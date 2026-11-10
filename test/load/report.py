#!/usr/bin/env python3
"""Turns one load run into a Markdown table.

    report.py OUT_DIR

Reads OUT_DIR/runs.txt, which run.sh writes as it goes, and for each run the
ab report (.ab), the docker stats samples (.mem) and the service's request
counts and pool state before and after (.before, .after).
"""
import pathlib
import re
import sys

LABELS = {"text": "Text", "docx": "DOCX"}
UNITS = {"KiB": 1 / 1024, "MiB": 1, "GiB": 1024}


def field(pattern: str, text: str) -> str | None:
    found = re.search(pattern, text, re.M)
    return found.group(1) if found else None


def statuses(text: str) -> dict[str, int]:
    return {s: int(n) for s, n in re.findall(r'route="/v1/render",status="(\d+)"\} (\d+)', text)}


def restarts(text: str) -> int:
    return int(field(r'"restarts":(\d+)', text) or 0)


def peak_memory(text: str) -> str:
    samples = [float(n) * UNITS[u] for n, u in re.findall(r"([\d.]+)(KiB|MiB|GiB) /", text)]
    return f"{max(samples):.0f} MiB" if samples else "not sampled"


def row(out: pathlib.Path, name: str, steps: str, connections: str, requests: str) -> list[str]:
    stem = out / f"{name}-c{connections}"
    ab = stem.with_suffix(".ab").read_text()
    before, after = (stem.with_suffix(s).read_text() for s in (".before", ".after"))
    kind = LABELS.get(name.split("-")[0], "DOCX")
    label = f"{kind}, no steps" if steps == "-" else f"{kind}, `{steps}`"
    cells = [label, connections]

    rate = field(r"^Requests per second:\s+([\d.]+)", ab)
    if rate is None:
        # ab gives up without a summary when a response takes longer than -s.
        last = ab.strip().splitlines()[-1] if ab.strip() else "no output"
        return cells + ["ab stopped", "", "", "", "", f"{last}", peak_memory(stem.with_suffix(".mem").read_text()), ""]

    was, now = statuses(before), statuses(after)
    answered = {s: now[s] - was.get(s, 0) for s in now if now[s] - was.get(s, 0)}
    bad = {s: n for s, n in answered.items() if s != "200"}
    failed = int(field(r"^Failed requests:\s+(\d+)", ab) or 0)
    non_2xx = int(field(r"^Non-2xx responses:\s+(\d+)", ab) or 0)
    # The service's own count has to agree with ab's before a run counts as clean.
    clean = not failed and not non_2xx and not bad and answered.get("200") == int(requests)
    if clean:
        failures = f"0 of {requests}"
    else:
        codes = ", ".join(f"{n} × {s}" for s, n in sorted(bad.items())) or "none by status"
        failures = f"{failed + non_2xx} of {requests} ({codes}; {answered.get('200', 0)} × 200)"

    mean = field(r"^Time per request:\s+([\d.]+) \[ms\] \(mean\)", ab)
    cells += [f"{float(rate):.1f}" if clean else "run failed", f"{float(mean):.1f}"]
    cells += [field(rf"^\s+{p}%\s+(\d+)", ab) or "" for p in (50, 95, 99)]
    cells += [failures, peak_memory(stem.with_suffix(".mem").read_text()), str(restarts(after) - restarts(before))]
    return cells


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    out = pathlib.Path(sys.argv[1])
    print("| Template and steps | Connections | Requests/s | Mean, ms | p50, ms | p95, ms | p99, ms | Failed | Peak memory | LibreOffice restarts |")
    print("|---|---|---|---|---|---|---|---|---|---|")
    for line in (out / "runs.txt").read_text().splitlines():
        print("| " + " | ".join(row(out, *line.split())) + " |")


if __name__ == "__main__":
    main()
