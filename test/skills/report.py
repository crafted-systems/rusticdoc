#!/usr/bin/env python3
"""Summarises one or more eval runs, side by side.

    python3 report.py test/skills/out/*/results.jsonl

Each file is one run (an agent, an arm and a model). Prints the pass count of
each run, then a row per case with pass or FAIL under each run.
"""
import json
import os
import sys


def main():
    runs = []
    for path in sys.argv[1:]:
        rows = [json.loads(line) for line in open(path) if line.strip()]
        label = os.path.basename(os.path.dirname(path)) or path
        runs.append((label, {r["id"]: r for r in rows}))
    if not runs:
        sys.exit("usage: report.py RESULTS.jsonl...")
    for label, rows in runs:
        passed = sum(r["pass"] for r in rows.values())
        print(f"{label}: {passed}/{len(rows)} passed")
    ids = sorted({i for _, rows in runs for i in rows})
    width = max(len(i) for i in ids)
    print()
    print(" " * width + "  " + "  ".join(label for label, _ in runs))
    for i in ids:
        cells = []
        for label, rows in runs:
            r = rows.get(i)
            mark = "-" if r is None else ("pass" if r["pass"] else "FAIL")
            cells.append(mark.ljust(len(label)))
        print(i.ljust(width) + "  " + "  ".join(cells))
    failures = [(label, i, r["failures"]) for label, rows in runs for i, r in sorted(rows.items()) if not r["pass"]]
    if failures:
        print("\nFailures:")
        for label, i, f in failures:
            print(f"  {label} {i}: {'; '.join(f)[:300]}")


if __name__ == "__main__":
    main()
