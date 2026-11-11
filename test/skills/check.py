#!/usr/bin/env python3
"""Checks what an agent left in a case folder: the template validates against
each data file, renders to PDF, and the output says what the case expects.

    python3 check.py cases.json CASE_ID FOLDER

Prints one JSON line: {"id", "pass", "failures"}. Calls the service at
RUSTICDOC_URL (default http://localhost:9027) with RUSTICDOC_API_KEY if set,
and needs curl, pdftotext and pdfinfo.
"""
import json
import os
import re
import subprocess
import sys
import tempfile
import zipfile

URL = os.environ.get("RUSTICDOC_URL", "http://localhost:9027").rstrip("/")
KEY = os.environ.get("RUSTICDOC_API_KEY", "")
OFFICE = (".docx", ".xlsx", ".pptx", ".odt", ".ods", ".odp")


def call(path, fields, folder):
    """POSTs a multipart form. Fields are (name, value) pairs; a value that
    starts with @ is a file in the folder. Returns (status, body)."""
    with tempfile.NamedTemporaryFile(delete=False) as out:
        target = out.name
    args = ["curl", "-sS", "-o", target, "-w", "%{http_code}"]
    for name, value in fields:
        args += ["-F", f"{name}={value}"]
    stdin = None
    if KEY:
        args += ["-H", "@-"]
        stdin = f"X-Api-Key: {KEY}\n"
    args.append(URL + path)
    done = subprocess.run(args, cwd=folder, input=stdin, capture_output=True, text=True)
    with open(target, "rb") as f:
        body = f.read()
    os.unlink(target)
    status = int(done.stdout.strip() or 0)
    return status, body


def normalise(text):
    return re.sub(r"\s+", " ", text.replace(" ", " ").replace(" ", " "))


def pdf_text(pdf):
    with tempfile.NamedTemporaryFile(suffix=".pdf", delete=False) as f:
        f.write(pdf)
        name = f.name
    text = subprocess.run(["pdftotext", name, "-"], capture_output=True, text=True).stdout
    info = subprocess.run(["pdfinfo", name], capture_output=True, text=True).stdout
    os.unlink(name)
    pages = int((re.search(r"^Pages:\s+(\d+)", info, re.M) or [0, 0])[1])
    return normalise(text), pages


def template_text(path):
    """The template's text, with an Office file's XML tags taken out, for
    checking which syntax it uses."""
    if path.lower().endswith(OFFICE):
        parts = []
        with zipfile.ZipFile(path) as z:
            for name in z.namelist():
                if name.endswith(".xml"):
                    xml = z.read(name).decode("utf-8", "replace")
                    parts.append(re.sub(r"<[^>]+>", "", xml))
        return "\n".join(parts)
    with open(path, encoding="utf-8", errors="replace") as f:
        return f.read()


def code_checks(c, folder):
    """Checks on code an agent wrote: what its source holds, and what it does
    when run. The snippets come from the case file, not from the agent."""
    failures = []
    for key, want in (("source_has", True), ("source_lacks", False)):
        if key not in c:
            continue
        for name in c[key]["files"]:
            path = os.path.join(folder, name)
            if not os.path.isfile(path):
                failures.append(f"no file {name}")
                continue
            with open(path, encoding="utf-8", errors="replace") as f:
                source = f.read()
            for pattern in c[key]["patterns"]:
                if bool(re.search(pattern, source)) != want:
                    failures.append(f"{name} {'lacks' if want else 'has'} /{pattern}/")
    if "python" in c:
        done = subprocess.run([sys.executable, "-c", c["python"]], cwd=folder, capture_output=True, text=True, timeout=300)
        if done.returncode != 0 or "ok" not in done.stdout:
            failures.append(f"python check: {(done.stderr or done.stdout).strip()[-300:]}")
    if "verify_suite" in c:
        failures += verify_suite(c["verify_suite"], folder)
    return failures


def verify_suite(spec, folder):
    """Runs a webhook verifier against real deliveries and altered copies of
    them: each real one must pass, and each altered one must fail."""
    sys.path.insert(0, folder)
    try:
        module = __import__(spec["module"])
        verify = getattr(module, spec["function"])
    except Exception as e:
        return [f"can't load {spec['module']}.{spec['function']}: {e}"]
    finally:
        sys.path.pop(0)
    with open(os.path.join(folder, spec["key"])) as f:
        key = bytes.fromhex(f.read().strip())
    failures = []
    root = os.path.join(folder, spec["deliveries"])
    for name in sorted(os.listdir(root)):
        with open(os.path.join(root, name, "headers.json")) as f:
            headers = json.load(f)
        with open(os.path.join(root, name, "body.bin"), "rb") as f:
            body = f.read()
        ts = int(headers["x-rusticdoc-timestamp"])
        cases = [
            ("real", headers, body, key, ts + 10, True),
            ("real, title-case headers", {k.title(): v for k, v in headers.items()}, body, key, ts + 10, True),
            ("changed body", headers, body + b" ", key, ts + 10, False),
            ("changed event", {**headers, "x-rusticdoc-event": "render.failed" if headers["x-rusticdoc-event"] == "render.completed" else "render.completed"}, body, key, ts + 10, False),
            ("changed request id", {**headers, "x-request-id": "someone-else"}, body, key, ts + 10, False),
            ("another key", headers, body, bytes(32), ts + 10, False),
            ("stale", headers, body, key, ts + 3600, False),
            ("no signature", {k: v for k, v in headers.items() if k != "x-rusticdoc-signature"}, body, key, ts + 10, False),
        ]
        for label, h, b, k, now, want in cases:
            try:
                got = bool(verify(h, b, k, now))
            except Exception as e:
                got = f"raised {type(e).__name__}"
                if want is False:
                    got = False  # refusing by raising is acceptable
            if got != want:
                failures.append(f"{name} {label}: want {want}, got {got}")
    return failures


def check(case, folder):
    failures = []
    template = case["template"]
    if template is None:
        for c in case["checks"]:
            if "output" in c:
                out = os.path.join(folder, c["output"])
                if not os.path.isfile(out):
                    failures.append(f"no output at {c['output']}")
                    continue
                with open(out, "rb") as f:
                    text, _ = pdf_text(f.read())
                for s in c.get("contains", []):
                    if normalise(s) not in text:
                        failures.append(f"{c['output']} lacks {s!r}")
            failures += code_checks(c, folder)
        return failures
    path = os.path.join(folder, template)
    if not case.get("output") and not os.path.isfile(path):
        return [f"no template at {template}"]
    if os.path.isfile(path):
        text = template_text(path)
        for pattern in case.get("template_uses", []):
            if not re.search(pattern, text):
                failures.append(f"template doesn't use /{pattern}/")
        for pattern in case.get("template_avoids", []):
            if re.search(pattern, text):
                failures.append(f"template still has /{pattern}/")

    for data in case["data"]:
        if not os.path.isfile(os.path.join(folder, data)):
            failures.append(f"no data file {data}")
            continue
        if not template.endswith(".pdf"):
            status, body = call("/v1/templates/validate", [("template", f"@{template}"), ("data", f"<{data}")], folder)
            try:
                result = json.loads(body)
            except ValueError:
                result = {}
            if status != 200 or not result.get("ok"):
                failures.append(f"validate with {data}: {result.get('error') or body[:200]!r}")

    for c in case["checks"]:
        if "output" in c:
            out = os.path.join(folder, c["output"])
            if not os.path.isfile(out):
                failures.append(f"no output at {c['output']}")
                continue
            with open(out, "rb") as f:
                pdf = f.read()
            text, pages = pdf_text(pdf)
            if c.get("no_fields"):
                status, body = call("/v1/pdf/forms/fields", [("files", f"@{c['output']}")], folder)
                if status != 200 or json.loads(body or b"[]"):
                    failures.append(f"{c['output']} still has form fields")
        else:
            data = c["data"]
            if not os.path.isfile(os.path.join(folder, data)):
                continue
            form = [("template", f"@{template}"), ("data", f"<{data}")]
            form += list(c.get("fields", {}).items())
            if "json" in c or "csv_rows" in c:
                status, body = call("/v1/render", form, folder)
                if status != 200:
                    failures.append(f"render with {data}: HTTP {status} {body[:200]!r}")
                    continue
                if "json" in c:
                    try:
                        got = json.loads(body)
                    except ValueError as e:
                        failures.append(f"output isn't JSON: {e}")
                        continue
                    for k, v in c["json"].items():
                        if got.get(k) != v:
                            failures.append(f"json {k}: want {v!r}, got {got.get(k)!r}")
                if "csv_rows" in c:
                    rows = [r for r in body.decode("utf-8", "replace").splitlines() if r.strip()]
                    if len(rows) != c["csv_rows"]:
                        failures.append(f"csv has {len(rows)} rows, want {c['csv_rows']}")
                continue
            status, body = call("/v1/render", form + [("steps", "convert")], folder)
            if status != 200 or not body.startswith(b"%PDF"):
                failures.append(f"render with {data}: HTTP {status} {body[:200]!r}")
                continue
            text, pages = pdf_text(body)
        for s in c.get("contains", []):
            if normalise(s) not in text:
                failures.append(f"output lacks {s!r}")
        for options in c.get("contains_any", []):
            if not any(normalise(s) in text for s in options):
                failures.append(f"output has none of {options!r}")
        for s in c.get("absent", []):
            if normalise(s) in text:
                failures.append(f"output has {s!r}")
        for pattern in c.get("contains_re", []):
            if not re.search(pattern, text):
                failures.append(f"output doesn't match /{pattern}/")
        for pattern in c.get("absent_re", []):
            if re.search(pattern, text):
                failures.append(f"output matches /{pattern}/")
        if pages < c.get("pages_min", 0):
            failures.append(f"{pages} pages, want at least {c['pages_min']}")
    return failures


def main():
    cases_file, case_id, folder = sys.argv[1:4]
    with open(cases_file) as f:
        cases = {c["id"]: c for c in json.load(f)["cases"]}
    failures = check(cases[case_id], folder)
    print(json.dumps({"id": case_id, "pass": not failures, "failures": failures}))


if __name__ == "__main__":
    main()
