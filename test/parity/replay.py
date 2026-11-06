#!/usr/bin/env python3
"""Replays Gotenberg's published test scenarios against two deployments.

Gotenberg's integration suite describes each scenario in a .feature file. This
script reads those files as data, sends every request to the official image and
to gotenberg-slim, and checks the same expectations on both, using the same
pdfinfo, pdftotext and veraPDF commands from gotenberg/integration-tools.

It does not run upstream's Go test code, and it is not a substitute for it: run
`make conformance-browser` and `make conformance-office` too.

A scenario is compared on both sides and ends up as one of:
  same-pass   both met every expectation
  regression  official met them and slim did not
  both-fail   neither did, so it says nothing about slim (usually this harness)
  slim-only   slim met them and official did not

Usage: replay.py --features DIR --testdata DIR --out DIR [--workers N] [--tags a,b]
"""
import argparse
import concurrent.futures as futures
import email.parser
import email.policy
import http.server
import io
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
import traceback
import urllib.error
import urllib.request
import uuid
import zipfile
from pathlib import Path

ALLOW_ALL = {"CHROMIUM_ALLOW_LIST": ".+", "API_DOWNLOAD_FROM_ALLOW_LIST": ".+", "WEBHOOK_ALLOW_LIST": ".+"}
REMOVED_ENGINES = ("pdftk", "exiftool")
TOOLS_IMAGE = "gotenberg/integration-tools:latest"


class StepFailed(Exception):
    pass


class Unsupported(Exception):
    """A step this harness does not implement."""


# ---------------------------------------------------------------------------
# Feature file parsing
# ---------------------------------------------------------------------------
class Step:
    def __init__(self, text):
        self.text = text
        self.table = []
        self.doc = None


class Scenario:
    def __init__(self, feature, name, tags):
        self.feature = feature
        self.name = name
        self.tags = tags
        self.steps = []

    @property
    def key(self):
        return f"{self.feature}: {self.name}"


def parse_feature(path):
    scenarios, tags, feature_tags, current, step = [], [], [], None, None
    in_doc, doc_lines, doc_indent = False, [], 0
    seen_feature = False
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if in_doc:
            if line == '"""':
                step.doc = "\n".join(doc_lines)
                in_doc = False
            else:
                doc_lines.append(raw[doc_indent:] if len(raw) >= doc_indent else raw.lstrip())
            continue
        if line == '"""':
            in_doc, doc_lines, doc_indent = True, [], len(raw) - len(raw.lstrip())
            continue
        if not line or line.startswith("#"):
            continue
        if line.startswith("@"):
            tags.extend(tag[1:] for tag in line.split())
        elif line.startswith("Feature:"):
            feature_tags, tags, seen_feature = tags, [], True
        elif line.startswith("Scenario"):
            current = Scenario(path.name, line.split(":", 1)[1].strip(), set(feature_tags + tags))
            tags, step = [], None
            scenarios.append(current)
        elif line.startswith("|"):
            # Gherkin cell escapes: \\ for a backslash, \n for a newline, \| for a pipe.
            cells = [re.sub(r"\\(\\|n|\|)", lambda e: {"\\": "\\", "n": "\n", "|": "|"}[e[1]], cell.strip())
                     for cell in re.split(r"(?<!\\)\|", line)[1:-1]]
            step.table.append(cells)
        elif re.match(r"(Given|When|Then|And|But)\s", line):
            step = Step(line.split(None, 1)[1])
            current.steps.append(step)
    return scenarios


def scenario_reads_logs(scenario):
    return any("should log the following" in s.text or "should NOT log the following" in s.text for s in scenario.steps)


def needs_office(scenario):
    for step in scenario.steps:
        if "/forms/libreoffice" in step.text or "/forms/pdfengines/convert" in step.text:
            return True
        if any(row and row[0] in ("pdfa", "pdfua") for row in step.table):
            return True
    return False


# ---------------------------------------------------------------------------
# Multipart requests
# ---------------------------------------------------------------------------
def encode_form(fields, files):
    boundary = uuid.uuid4().hex
    out = io.BytesIO()
    for name, value in fields:
        out.write(f'--{boundary}\r\nContent-Disposition: form-data; name="{name}"\r\n\r\n'.encode())
        out.write(value.encode())
        out.write(b"\r\n")
    for name, path in files:
        filename = os.path.basename(path)
        out.write(f'--{boundary}\r\nContent-Disposition: form-data; name="{name}"; filename="{filename}"\r\n'.encode())
        out.write(b"Content-Type: application/octet-stream\r\n\r\n")
        out.write(Path(path).read_bytes())
        out.write(b"\r\n")
    out.write(f"--{boundary}--\r\n".encode())
    return out.getvalue(), f"multipart/form-data; boundary={boundary}"


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


OPENER = urllib.request.build_opener(NoRedirect, urllib.request.ProxyHandler({}))


def http_call(method, url, data=None, headers=None, timeout=240):
    request = urllib.request.Request(url, data=data, method=method, headers=headers or {})
    try:
        with OPENER.open(request, timeout=timeout) as resp:
            return resp.status, resp.headers, resp.read()
    except urllib.error.HTTPError as err:
        return err.code, err.headers, err.read()


# ---------------------------------------------------------------------------
# Host side webhook and static server, one per scenario and deployment
# ---------------------------------------------------------------------------
class HostServer:
    def __init__(self, workdir, testdata, bind="127.0.0.1"):
        self.workdir, self.testdata = workdir, testdata
        self.req_headers = None
        self.cookies = {}
        self.body = None
        self.event_body = None
        self.done = threading.Event()
        outer = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def _record(self):
                outer.req_headers = self.headers
                outer.cookies = {}
                for part in (self.headers.get("Cookie") or "").split(";"):
                    if "=" in part:
                        name, value = part.strip().split("=", 1)
                        outer.cookies[name] = value

            def _send(self, status, body=b"OK", ctype="text/plain", extra=None):
                self.send_response(status)
                self.send_header("Content-Type", ctype)
                self.send_header("Content-Length", str(len(body)))
                for key, value in (extra or {}).items():
                    self.send_header(key, value)
                self.end_headers()
                if self.command != "HEAD":
                    self.wfile.write(body)

            def do_GET(self):
                path = self.path.split("?")[0]
                from urllib.parse import unquote
                if path.startswith("/static/"):
                    self._record()
                    rel = unquote(path[len("/static/"):])
                    if "teststore" in rel:
                        target = confined(outer.workdir, safe_trace(self.headers.get("Gotenberg-Trace")), os.path.basename(rel))
                    else:
                        target = confined(outer.testdata, relative_to_testdata(rel))
                    if not target or not target.is_file():
                        return self._send(404, b"not found")
                    return self._send(200, target.read_bytes(), "application/octet-stream",
                                      {"Content-Disposition": f'attachment; filename="{target.name}"'})
                if path.startswith("/html/"):
                    self._record()
                    target = confined(outer.testdata, relative_to_testdata(unquote(path[len("/html/"):])))
                    if not target or not target.is_file():
                        return self._send(500, b"open file failed")
                    return self._send(200, target.read_bytes(), "text/html; charset=UTF-8")
                if path == "/redirect-to-private":
                    self._record()
                    return self._send(302, b"", "text/plain", {"Location": "http://127.0.0.1:9999/redirected"})
                self._send(404, b"not found")

            do_HEAD = do_GET

            def _webhook(self):
                length = int(self.headers.get("Content-Length") or 0)
                body = self.rfile.read(length)
                path = self.path.split("?")[0]
                if path == "/webhook/events":
                    outer.event_body = body
                    return self._send(200)
                self._record()
                outer.body = body
                if path == "/webhook":
                    outer.store_webhook(self.headers, body)
                self._send(200)
                outer.done.set()

            do_POST = do_PUT = do_PATCH = _webhook

        self.httpd = http.server.ThreadingHTTPServer((bind, 0), Handler)
        self.port = self.httpd.server_address[1]
        threading.Thread(target=self.httpd.serve_forever, daemon=True).start()

    def store_webhook(self, headers, body):
        directory = Path(self.workdir) / safe_trace(headers.get("Gotenberg-Trace"))
        directory.mkdir(parents=True, exist_ok=True)
        filename = content_disposition_filename(headers.get("Content-Disposition"))
        ctype = headers.get("Content-Type", "")
        if not filename:
            filename = uuid.uuid4().hex + (".zip" if ctype == "application/zip" else ".pdf")
        (directory / filename).write_bytes(body)
        if ctype == "application/zip":
            with zipfile.ZipFile(io.BytesIO(body)) as archive:
                archive.extractall(directory)

    def stop(self):
        self.httpd.shutdown()
        self.httpd.server_close()


def safe_trace(value):
    """A trace ID becomes a directory name, so it must not carry a path."""
    return value if value and re.fullmatch(r"[A-Za-z0-9_.-]+", value) and value.strip(".") else "unknown"


def relative_to_testdata(path):
    return path[len("testdata/"):] if path.startswith("testdata/") else path


def confined(base, *parts):
    """Returns the path only if it stays inside base. The server sees unauthenticated requests."""
    root = Path(base).resolve()
    target = root.joinpath(*parts).resolve()
    return target if target.is_relative_to(root) else None


def content_disposition_filename(value):
    if not value:
        return None
    msg = email.parser.Parser(policy=email.policy.default).parsestr(f"Content-Disposition: {value}\n\n")
    name = msg.get_filename()
    return os.path.basename(name) if name else None


# ---------------------------------------------------------------------------
# Containers
# ---------------------------------------------------------------------------
def docker(*args, check=True, timeout=180):
    return subprocess.run(["docker", *args], capture_output=True, text=True, check=check, timeout=timeout)


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def start_container(image, name, env, platform, network=None):
    port = free_port()
    args = ["run", "-d", "--name", name, "--platform", platform, "-p", f"127.0.0.1:{port}:3000",
            "--add-host", "host.docker.internal:host-gateway"]
    if network:
        args += ["--network", network, "--network-alias", name]
    for key, value in env.items():
        args += ["-e", f"{key}={value}"]
    docker(*args, image)
    return port


def wait_healthy(port, root, deadline=150):
    url = f"http://127.0.0.1:{port}{root}health"
    end = time.time() + deadline
    while time.time() < end:
        try:
            status, _, _ = http_call("GET", url, timeout=3)
            if status == 200:
                return True
        except Exception:
            pass
        time.sleep(0.5)
    return False


class Deployment:
    """One or two containers plus the rule for which one serves a path."""

    def __init__(self, routes, names, root="/"):
        self.routes, self.names, self.root = routes, names, root

    def url(self, endpoint):
        office = "/forms/libreoffice" in endpoint or endpoint.endswith("/forms/pdfengines/convert")
        port = self.routes.get("office") if office else None
        port = port or self.routes.get("main") or self.routes.get("office")
        return f"http://127.0.0.1:{port}{endpoint}"

    def logs(self):
        out = []
        for name in self.names:
            result = docker("logs", name, check=False, timeout=60)
            out.append(result.stdout + result.stderr)
        return "\n".join(out)

    def stop(self):
        for name in self.names:
            docker("rm", "-f", name, check=False, timeout=90)


class Lab:
    def __init__(self, args):
        self.args = args
        self.network = f"parity-{uuid.uuid4().hex[:8]}"
        docker("network", "create", self.network)
        self.shared = {}
        self.lock = threading.Lock()

    def official(self, env):
        name = f"parity-off-{uuid.uuid4().hex[:8]}"
        port = start_container(self.args.official, name, env, self.args.platform)
        root = env.get("API_ROOT_PATH") or "/"
        dep = Deployment({"main": port}, [name], root)
        if not wait_healthy(port, root):
            logs = dep.logs()
            dep.stop()
            raise StepFailed(f"official container did not start: {logs[-400:]}")
        return dep

    def slim(self, env, office_needed=True):
        # Extra variables for the slim image only; a scenario's own win.
        env = dict(env)
        for pair in self.args.slim_env:
            key, _, value = pair.partition("=")
            env.setdefault(key, value)
        token = uuid.uuid4().hex[:8]
        platform = self.args.slim_platform or self.args.platform
        if self.args.office_only:
            name = f"parity-office-{token}"
            root = env.get("API_ROOT_PATH") or "/"
            port = start_container(self.args.slim_office, name, env, platform)
            dep = Deployment({"main": port}, [name], root)
            if not wait_healthy(port, root):
                logs = dep.logs()
                dep.stop()
                raise StepFailed(f"slim container did not start: {logs[-400:]}")
            return dep
        office_name, browser_name = f"parity-office-{token}", f"parity-browser-{token}"
        root = env.get("API_ROOT_PATH") or "/"
        browser_env = dict(env)
        browser_env["REMOTE_CONVERT_URL"] = f"http://{office_name}:3000{root.rstrip('/')}"
        if env.get("GOTENBERG_API_BASIC_AUTH_USERNAME"):
            browser_env["REMOTE_CONVERT_USERNAME"] = env["GOTENBERG_API_BASIC_AUTH_USERNAME"]
            browser_env["REMOTE_CONVERT_PASSWORD"] = env.get("GOTENBERG_API_BASIC_AUTH_PASSWORD", "")
        routes, names = {}, []
        if office_needed:
            routes["office"] = start_container(self.args.slim_office, office_name, env, platform, self.network)
            names.append(office_name)
        routes["main"] = start_container(self.args.slim_browser, browser_name, browser_env, platform, self.network)
        names.append(browser_name)
        dep = Deployment(routes, names, root)
        for port in routes.values():
            if not wait_healthy(port, root):
                logs = dep.logs()
                dep.stop()
                raise StepFailed(f"slim container did not start: {logs[-400:]}")
        return dep

    def default(self, side):
        with self.lock:
            if side not in self.shared:
                env = dict(ALLOW_ALL)
                self.shared[side] = self.official(env) if side == "official" else self.slim(env)
            return self.shared[side]

    def close(self):
        for dep in self.shared.values():
            dep.stop()
        docker("network", "rm", self.network, check=False)


# ---------------------------------------------------------------------------
# PDF tools: the same commands upstream's steps run.
# ---------------------------------------------------------------------------
class Tools:
    def __init__(self, platform):
        self.name = f"parity-tools-{uuid.uuid4().hex[:8]}"
        self.root = tempfile.mkdtemp(prefix="parity-tools-")
        docker("run", "-d", "--name", self.name, "--platform", platform, "-v", f"{self.root}:/work:ro",
               TOOLS_IMAGE, "tail", "-f", "/dev/null")
        self.limit = threading.Semaphore(4)

    def run(self, path, *cmd, after=()):
        token = uuid.uuid4().hex
        directory = Path(self.root) / token
        directory.mkdir()
        os.chmod(directory, 0o755)
        target = directory / os.path.basename(path)
        shutil.copy(path, target)
        os.chmod(target, 0o644)
        try:
            with self.limit:
                result = docker("exec", "-w", f"/work/{token}", self.name, *cmd, target.name, *after, check=False, timeout=240)
            return result.stdout + result.stderr
        finally:
            shutil.rmtree(directory, ignore_errors=True)

    def close(self):
        docker("rm", "-f", self.name, check=False)
        shutil.rmtree(self.root, ignore_errors=True)


def compare_json(expected, actual, path="$"):
    if isinstance(expected, dict):
        if not isinstance(actual, dict):
            raise StepFailed(f"{path}: expected an object")
        for key, value in expected.items():
            if value == "ignore":
                continue
            if key not in actual:
                raise StepFailed(f"{path}: missing key {key!r}")
            compare_json(value, actual[key], f"{path}.{key}")
    elif isinstance(expected, list):
        if not isinstance(actual, list) or len(actual) != len(expected):
            raise StepFailed(f"{path}: expected a list of {len(expected)}, got {actual!r}")
        for i, value in enumerate(expected):
            compare_json(value, actual[i], f"{path}[{i}]")
    elif expected != actual:
        raise StepFailed(f"{path}: expected {expected!r}, got {actual!r}")


# ---------------------------------------------------------------------------
# Running one scenario on one deployment
# ---------------------------------------------------------------------------
class Run:
    def __init__(self, lab, tools, side, scenario, testdata):
        self.lab, self.tools, self.side, self.scenario = lab, tools, side, scenario
        self.testdata = testdata
        self.workdir = tempfile.mkdtemp(prefix="parity-run-")
        self.server = None
        self.dep = None
        self.owned = None
        self.status, self.headers, self.body = None, {}, b""
        self.store = None
        self.statuses, self.probe_statuses, self.seq_statuses, self.conc_counts = [], [], [], []
        self.skipped = []

    def close(self):
        if self.owned:
            self.owned.stop()
        if self.server:
            self.server.stop()
        shutil.rmtree(self.workdir, ignore_errors=True)

    # --- requests ---
    def substitute(self, value):
        port = self.server.port if self.server else 0
        return value.replace("%d", str(port))

    def build(self, table):
        fields, files, headers = [], [], {}
        for row in table:
            if len(row) < 3:
                continue
            name, value, kind = row[0], row[1], row[2]
            if kind == "field":
                fields.append((name, self.substitute(value) if name in ("downloadFrom", "url", "cookies", "cookie") else value))
            elif kind == "file":
                if "teststore" in value:
                    if not self.store:
                        raise StepFailed("no teststore from a previous request")
                    files.append((name, value.replace("teststore", self.store)))
                else:
                    files.append((name, str(Path(self.testdata).parent / value)))
            elif kind == "header":
                headers[name] = self.substitute(value) if name.startswith("Gotenberg-Webhook") and "Url" in name else value
        return fields, files, headers

    def post(self, method, endpoint, table):
        fields, files, headers = self.build(table)
        data, ctype = encode_form(fields, files)
        return http_call(method, self.dep.url(endpoint), data, {**headers, "Content-Type": ctype})

    def record(self, status, headers, body):
        self.status, self.headers, self.body = status, headers, body
        trace = headers.get("Gotenberg-Trace") or ""
        if status == 204:
            self.store = str(Path(self.workdir) / trace)
            return
        if status != 200:
            return
        filename = content_disposition_filename(headers.get("Content-Disposition"))
        if not filename:
            return
        directory = Path(self.workdir) / trace
        directory.mkdir(parents=True, exist_ok=True)
        self.store = str(directory)
        (directory / filename).write_bytes(body)
        if headers.get("Content-Type") == "application/zip":
            with zipfile.ZipFile(io.BytesIO(body)) as archive:
                archive.extractall(directory)

    # --- files ---
    def pdfs(self):
        if not self.store or not os.path.isdir(self.store):
            raise StepFailed("no output directory")
        return sorted(str(p) for p in Path(self.store).rglob("*") if p.suffix.lower() == ".pdf")

    def named(self, name):
        if not self.store:
            raise StepFailed("no output directory")
        for path in Path(self.store).rglob("*"):
            if path.is_file() and (path.name == name or (name.startswith("*_") and name[2:] in path.name)):
                return str(path)
        raise StepFailed(f"file {name!r} not found")

    def start_own(self, env):
        if self.owned:
            self.owned.stop()
        if self.side == "official":
            self.owned = self.lab.official(env)
        else:
            self.owned = self.lab.slim(env, needs_office(self.scenario))
        self.dep = self.owned

    # --- steps ---
    def step(self, step):
        text = step.text
        m = re.fullmatch(r'I have a default Gotenberg container', text)
        if m:
            # Log expectations need logs of this scenario alone.
            if scenario_reads_logs(self.scenario):
                self.start_own(dict(ALLOW_ALL))
            else:
                self.dep = self.lab.default(self.side)
            return
        if text.startswith("I have a Gotenberg container with the following environment"):
            env = dict(ALLOW_ALL)
            env.update({row[0]: row[1] if len(row) > 1 else "" for row in step.table})
            self.start_own(env)
            return
        if re.fullmatch(r"I have a (webhook|static) server", text):
            if not self.server:
                self.server = HostServer(self.workdir, self.testdata, self.lab.args.bind)
            return
        m = re.fullmatch(r'I make a "(GET|HEAD)" request to Gotenberg at the "([^"]*)" endpoint', text)
        if m:
            return self.record(*http_call(m[1], self.dep.url(m[2])))
        m = re.fullmatch(r'I make a "(GET|HEAD)" request to Gotenberg at the "([^"]*)" endpoint with the following header\(s\):', text)
        if m:
            return self.record(*http_call(m[1], self.dep.url(m[2]), headers={row[0]: row[1] for row in step.table}))
        m = re.fullmatch(r'I make a "(POST)" request to Gotenberg at the "([^"]*)" endpoint with the following form data and header\(s\):', text)
        if m:
            if self.server:
                self.server.done.clear()
            return self.record(*self.post(m[1], m[2], step.table))
        m = re.fullmatch(r'I make (\d+) concurrent "(POST)" requests to Gotenberg at the "([^"]*)" endpoint with the following form data and header\(s\):', text)
        if m:
            with futures.ThreadPoolExecutor(int(m[1])) as pool:
                results = list(pool.map(lambda _: self.post(m[2], m[3], step.table), range(int(m[1]))))
            self.statuses = [r[0] for r in results]
            self.conc_counts = [zip_pdf_count(r) for r in results]
            return
        m = re.fullmatch(r'I make (\d+) sequential "(POST)" requests to Gotenberg at the "([^"]*)" endpoint, probing "([^"]*)" after each, with the following form data and header\(s\):', text)
        if m:
            for _ in range(int(m[1])):
                self.seq_statuses.append(self.post(m[2], m[3], step.table)[0])
                self.probe_statuses.append(http_call("GET", self.dep.url(m[4]))[0])
            return
        if text == "I wait for the asynchronous request to the webhook":
            if not self.server.done.wait(180):
                raise StepFailed("no webhook request within 180 seconds")
            self.body = self.server.body
            return
        m = re.fullmatch(r'the Gotenberg container (should|should NOT) log the following entries:', text)
        if m:
            time.sleep(0.5)
            logs = self.dep.logs()
            for row in step.table:
                found = row[0] in logs
                if found != (m[1] == "should"):
                    raise StepFailed(f"log entry {row[0]!r}: found={found}")
            return
        m = re.fullmatch(r'the response status code should be (\d+)', text)
        if m:
            if self.status != int(m[1]):
                raise StepFailed(f"status {self.status}, want {m[1]}: {self.body[:200]!r}")
            return
        m = re.fullmatch(r'all concurrent response status codes should be (\d+)', text)
        if m:
            if any(s != int(m[1]) for s in self.statuses):
                raise StepFailed(f"statuses {self.statuses}")
            return
        m = re.fullmatch(r'all probe response status codes should be (\d+)', text)
        if m:
            if any(s != int(m[1]) for s in self.probe_statuses):
                raise StepFailed(f"statuses {self.probe_statuses}")
            return
        m = re.fullmatch(r'all sequential response status codes should be (\d+)', text)
        if m:
            if any(s != int(m[1]) for s in self.seq_statuses):
                raise StepFailed(f"statuses {self.seq_statuses}")
            return
        m = re.fullmatch(r'all concurrent responses should have (\d+) PDF\(s\)', text)
        if m:
            if any(c != int(m[1]) for c in self.conc_counts):
                raise StepFailed(f"counts {self.conc_counts}")
            return
        m = re.fullmatch(r'the (response|webhook request|file request|server request) header "([^"]*)" should be "([^"]*)"', text)
        if m:
            headers = self.headers if m[1] == "response" else (self.server.req_headers or {})
            value = headers.get(m[2])
            if m[3] == "":
                ok = not value
            else:
                expected = m[3].replace("{version}", self.version())
                ok = value == expected
            if not ok:
                raise StepFailed(f"header {m[2]} = {value!r}, want {m[3]!r}")
            return
        m = re.fullmatch(r'the webhook request header "([^"]*)" should carry trace id "([^"]*)"', text)
        if m:
            value = (self.server.req_headers or {}).get(m[1]) or ""
            if m[2] not in value:
                raise StepFailed(f"header {m[1]} = {value!r}")
            return
        m = re.fullmatch(r'the (response|webhook request|file request|server request) cookie "([^"]*)" should be "([^"]*)"', text)
        if m:
            value = self.server.cookies.get(m[2], "")
            if value != m[3]:
                raise StepFailed(f"cookie {m[2]} = {value!r}")
            return
        m = re.fullmatch(r'the (response|webhook request) body should (match|contain) string:', text)
        if m:
            body = (self.body if m[1] == "response" else self.server.body) or b""
            actual, expected = body.decode("utf-8", "replace"), (step.doc or "").replace("{version}", self.version())
            if m[2] == "match" and actual.strip() != expected.strip():
                raise StepFailed(f"body {actual[:300]!r}, want {expected[:300]!r}")
            if m[2] == "contain" and expected not in actual:
                raise StepFailed(f"body {actual[:300]!r} lacks {expected[:300]!r}")
            return
        m = re.fullmatch(r'the (response|webhook request) body should match JSON:', text)
        if m:
            body = self.body if m[1] == "response" else self.server.body
            try:
                actual = json.loads(body)
            except ValueError as err:
                raise StepFailed(f"not JSON: {body[:200]!r}") from err
            compare_json(json.loads((step.doc or "").replace("{version}", self.version())), actual)
            return
        if text == "the webhook event should match JSON:":
            end = time.time() + 5
            while self.server.event_body is None and time.time() < end:
                time.sleep(0.1)
            if self.server.event_body is None:
                raise StepFailed("no webhook event")
            compare_json(json.loads(step.doc), json.loads(self.server.event_body))
            return
        m = re.fullmatch(r'there should be (\d+) PDF\(s\) in the (response|webhook request)', text)
        if m:
            count = len(self.pdfs()) if self.store and os.path.isdir(self.store) else 0
            if count != int(m[1]):
                raise StepFailed(f"{count} PDFs, want {m[1]}")
            return
        m = re.fullmatch(r'there should be the following file\(s\) in the (response|webhook request):', text)
        if m:
            for row in step.table:
                self.named(row[0])
            return
        m = re.fullmatch(r'the (response|webhook request) PDF\(s\) should be valid "([^"]*)" with a tolerance of (\d+) failed rule\(s\)', text)
        if m:
            flavor = {"PDF/A-1b": "1b", "PDF/A-2b": "2b", "PDF/A-3b": "3b", "PDF/A-1a": "1a", "PDF/A-2a": "2a", "PDF/A-2u": "2u",
                      "PDF/A-3a": "3a", "PDF/A-3u": "3u", "PDF/A-4": "4", "PDF/A-4e": "4e", "PDF/A-4f": "4f",
                      "PDF/UA-1": "ua1", "PDF/UA-2": "ua2"}.get(m[2])
            if not flavor:
                raise Unsupported(f"flavor {m[2]}")
            for path in self.pdfs():
                output = self.tools.run(path, "verapdf", "-f", flavor)
                found = re.search(r'failedRules="(\d+)"', output)
                if not found:
                    raise StepFailed(f"veraPDF gave no result: {output[:200]!r}")
                if int(found[1]) > int(m[3]):
                    raise StepFailed(f"{found[1]} failed rules, tolerance {m[3]}")
            return
        m = re.fullmatch(r'the (response|webhook request) PDF\(s\) (should|should NOT) be flatten', text)
        if m:
            for path in self.pdfs():
                output = self.tools.run(path, "verapdf", "-off", "--extract", "annotations")
                flat = "<featuresReport></featuresReport>" in output
                if flat != (m[2] == "should"):
                    raise StepFailed(f"flatten={flat}")
            return
        m = re.fullmatch(r'the (response|webhook request) PDF\(s\) (should|should NOT) be encrypted', text)
        if m:
            for path in self.pdfs():
                output = self.tools.run(path, "pdfinfo").replace(" ", "").replace("\n", "")
                encrypted = "CommandLineError:Incorrectpassword" in output
                if encrypted != (m[2] == "should"):
                    raise StepFailed(f"encrypted={encrypted}")
            return
        m = re.fullmatch(r'the (response|webhook request) PDF\(s\) (should|should NOT) allow "([^"]*)"', text)
        if m:
            flag = {"printing": "print", "copying": "copy", "modifying": "change", "annotating": "addNotes"}[m[3]]
            for path in self.pdfs():
                output = self.tools.run(path, "pdfinfo").replace(" ", "").replace("\n", "")
                state = flag + (":yes" if m[2] == "should" else ":no")
                if state not in output:
                    raise StepFailed(f"want {state}: {output[:300]}")
            return
        m = re.fullmatch(r'the (response|webhook request) PDF\(s\) (should|should NOT) have the "([^"]*)" file embedded', text)
        if m:
            for path in self.pdfs():
                output = self.tools.run(path, "verapdf", "--off", "--loglevel", "0", "--extract", "embeddedFile")
                found = f"<fileName>{m[3]}</fileName>" in output
                if found != (m[2] == "should"):
                    raise StepFailed(f"embed {m[3]} found={found}")
            return
        m = re.fullmatch(r'the (response|webhook request) PDF\(s\) should have the "([^"]*)" file embedded with relationship "([^"]*)"', text)
        if m:
            for path in self.pdfs():
                output = self.tools.run(path, "verapdf", "--off", "--loglevel", "0", "--extract", "embeddedFile")
                blocks = [b for b in output.split("</embeddedFile>") if f"<fileName>{m[2]}</fileName>" in b]
                if not blocks or f"<afRelationship>{m[3]}</afRelationship>" not in blocks[0]:
                    raise StepFailed(f"embed {m[2]} relationship {m[3]} missing")
            return
        m = re.fullmatch(r'the (response|webhook request) PDF\(s\) should declare Factur-X XMP with conformance level "([^"]*)"', text)
        if m:
            for path in self.pdfs():
                output = self.tools.run(path, "pdfinfo", "-meta")
                for needle in ("urn:factur-x:pdfa:CrossIndustryDocument:invoice:1p0#", "pdfaExtension:schemas",
                               f"<fx:ConformanceLevel>{m[2]}</fx:ConformanceLevel>"):
                    if needle not in output:
                        raise StepFailed(f"XMP lacks {needle}")
            return
        m = re.fullmatch(r'the "([^"]*)" PDF should have (\d+) page\(s\)', text)
        if m:
            output = self.tools.run(self.named(m[1]), "pdfinfo").replace(" ", "")
            found = re.search(r"Pages:(\d+)", output)
            if not found or int(found[1]) != int(m[2]):
                raise StepFailed(f"pages {found[1] if found else None}, want {m[2]}")
            return
        m = re.fullmatch(r'the "([^"]*)" PDF (should|should NOT) be set to landscape orientation', text)
        if m:
            output = self.tools.run(self.named(m[1]), "pdfinfo").replace(" ", "")
            found = re.search(r"Pagesize:([\d.]+)x([\d.]+)", output)
            if not found:
                raise StepFailed("no page size")
            landscape = float(found[2]) < float(found[1])
            if landscape != (m[2] == "should"):
                raise StepFailed(f"landscape={landscape}")
            return
        m = re.fullmatch(r'the "([^"]*)" PDF (should|should NOT) be tagged', text)
        if m:
            output = self.tools.run(self.named(m[1]), "pdfinfo")
            tagged = re.search(r"Tagged:\s+yes", output) is not None
            if tagged != (m[2] == "should"):
                raise StepFailed(f"tagged={tagged}")
            return
        m = re.fullmatch(r'the "([^"]*)" PDF (should|should NOT) have a document outline', text)
        if m:
            output = self.tools.run(self.named(m[1]), "verapdf", "-off", "--extract", "outlines")
            has = "<featuresReport></featuresReport>" not in output
            if has != (m[2] == "should"):
                raise StepFailed(f"outline={has}")
            return
        m = re.fullmatch(r'the "([^"]*)" PDF (should|should NOT) have the following content at page (\d+):', text)
        if m:
            found = (step.doc or "") in self.page_text(m[1], m[3])
            if found != (m[2] == "should"):
                raise StepFailed(f"content {step.doc!r} at page {m[3]}: found={found}")
            return
        m = re.fullmatch(r'the "([^"]*)" PDF (should|should NOT) have content matching "([^"]*)" at page (\d+)', text)
        if m:
            found = re.search(m[3], self.page_text(m[1], m[4])) is not None
            if found != (m[2] == "should"):
                raise StepFailed(f"pattern {m[3]!r} at page {m[4]}: found={found}")
            return
        m = re.fullmatch(r'the "([^"]*)" file size should be (less|greater) than (\d+) KB', text)
        if m:
            size = os.path.getsize(self.named(m[1]))
            limit = int(m[3]) * 1024
            if (m[2] == "less" and size >= limit) or (m[2] == "greater" and size <= limit):
                raise StepFailed(f"size {size} vs {limit}")
            return
        m = re.fullmatch(r'the "([^"]*)" PDF should have (\d+) image\(s\)', text)
        if m:
            output = self.tools.run(self.named(m[1]), "pdfimages", "-list")
            count = max(0, len([l for l in output.splitlines() if l.strip()]) - 2)
            if count != int(m[2]):
                raise StepFailed(f"{count} images, want {m[2]}")
            return
        # Pixel and XObject-sharing checks need image decoding this harness lacks.
        raise Unsupported(text)

    def page_text(self, name, page):
        return self.tools.run(self.named(name), "pdftotext", "-f", str(page), "-l", str(page), after=("-",))

    def version(self):
        status, _, body = http_call("GET", self.dep.url(f"{self.dep.root}version"))
        return body.decode().strip() if status == 200 else ""

    def execute(self):
        for index, step in enumerate(self.scenario.steps, 1):
            try:
                self.step(step)
            except Unsupported as err:
                self.skipped.append(str(err)[:120])
            except StepFailed as err:
                return {"ok": False, "step": index, "text": step.text[:160], "reason": str(err)[:600], "skipped": self.skipped}
            except Exception as err:
                return {"ok": False, "step": index, "text": step.text[:160], "reason": f"{type(err).__name__}: {err}"[:600], "skipped": self.skipped}
        return {"ok": True, "skipped": self.skipped}


def zip_pdf_count(result):
    status, headers, body = result
    if status != 200:
        return 0
    if headers.get("Content-Type") == "application/zip":
        with zipfile.ZipFile(io.BytesIO(body)) as archive:
            return sum(1 for n in archive.namelist() if n.lower().endswith(".pdf"))
    return 1


def scenario_env_mentions_removed(scenario):
    for step in scenario.steps:
        if step.text.startswith("I have a Gotenberg container with"):
            for row in step.table:
                if len(row) > 1 and any(engine in row[1].lower() for engine in REMOVED_ENGINES):
                    return True
    return False


def run_side(lab, tools, side, scenario, testdata):
    run = Run(lab, tools, side, scenario, testdata)
    try:
        return run.execute()
    finally:
        run.close()


def run_scenario(lab, tools, scenario, testdata):
    with futures.ThreadPoolExecutor(2) as pool:
        official = pool.submit(run_side, lab, tools, "official", scenario, testdata)
        slim = pool.submit(run_side, lab, tools, "slim", scenario, testdata)
        return official.result(), slim.result()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--features", required=True)
    parser.add_argument("--testdata", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--official", default="gotenberg/gotenberg:8.39.0")
    parser.add_argument("--slim-browser", default="gotenberg-slim-browser:arm64")
    parser.add_argument("--slim-office", default="gotenberg-slim-office:arm64")
    parser.add_argument("--platform", default="linux/arm64")
    parser.add_argument("--workers", type=int, default=4)
    parser.add_argument("--office-only", action="store_true",
                        help="run slim as the office image alone, with no browser service")
    parser.add_argument("--slim-env", action="append", default=[],
                        help="KEY=VALUE set on the slim containers only; repeatable")
    parser.add_argument("--slim-platform", default="",
                        help="platform of the slim images when it differs from --platform")
    parser.add_argument("--bind", default="127.0.0.1",
                        help="address of the helper server. Loopback works on Docker Desktop; on Linux use the docker0 address")
    parser.add_argument("--tags", default="")
    parser.add_argument("--only", default="", help="regex over 'file: scenario name'")
    parser.add_argument("--resume", action="store_true")
    parser.add_argument("--retry-failed", action="store_true",
                        help="rerun what did not pass on both sides; the last result for a scenario wins")
    args = parser.parse_args()

    scenarios = []
    for path in sorted(Path(args.features).glob("*.feature")):
        scenarios.extend(parse_feature(path))
    if args.tags:
        wanted = set(args.tags.split(","))
        scenarios = [s for s in scenarios if s.tags & wanted]
    if args.only:
        scenarios = [s for s in scenarios if re.search(args.only, s.key)]

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    results_path = out / "results.jsonl"
    done = {}
    if (args.resume or args.retry_failed) and results_path.exists():
        for line in results_path.read_text().splitlines():
            record = json.loads(line)
            done[record["key"]] = record
    if args.retry_failed:
        done = {key: record for key, record in done.items() if record["official"]["ok"] and record["slim"]["ok"]}
    pending = [s for s in scenarios if s.key not in done]
    print(f"{len(scenarios)} scenarios, {len(pending)} to run", flush=True)

    lab, tools = Lab(args), Tools(args.platform)
    lock = threading.Lock()
    finished = [len(scenarios) - len(pending)]

    def work(scenario):
        try:
            official, slim = run_scenario(lab, tools, scenario, args.testdata)
        except Exception:
            failure = {"ok": False, "reason": traceback.format_exc()[-500:], "skipped": []}
            official, slim = failure, failure
        record = {"key": scenario.key, "feature": scenario.feature, "name": scenario.name,
                  "removed_engine": scenario_env_mentions_removed(scenario),
                  "needs_chromium": any("/forms/chromium" in s.text for s in scenario.steps), "official": official, "slim": slim}
        with lock:
            finished[0] += 1
            with results_path.open("a") as handle:
                handle.write(json.dumps(record) + "\n")
            if finished[0] % 10 == 0:
                print(f"{finished[0]}/{len(scenarios)}", flush=True)
        return record

    try:
        with futures.ThreadPoolExecutor(args.workers) as pool:
            list(pool.map(work, pending))
    finally:
        lab.close()
        tools.close()
    print("done", flush=True)


if __name__ == "__main__":
    sys.exit(main())
