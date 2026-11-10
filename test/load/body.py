#!/usr/bin/env python3
"""Writes the multipart/form-data body that ab posts to /v1/render, and prints
its content type.

    body.py TEMPLATE DATA STEPS OUT

STEPS is the text of the `steps` field, or - for none.
"""
import pathlib
import sys

BOUNDARY = "rusticdoc-load-7f3a9c"


def part(name: str, body: bytes, filename: str = "") -> bytes:
    head = f'--{BOUNDARY}\r\nContent-Disposition: form-data; name="{name}"'
    if filename:
        head += f'; filename="{filename}"\r\nContent-Type: application/octet-stream'
    return head.encode() + b"\r\n\r\n" + body + b"\r\n"


def main() -> None:
    if len(sys.argv) != 5:
        sys.exit(__doc__)
    template, data, steps, out = sys.argv[1:]
    body = part("template", pathlib.Path(template).read_bytes(), pathlib.Path(template).name)
    body += part("data", pathlib.Path(data).read_bytes())
    if steps != "-":
        body += part("steps", steps.encode())
    body += f"--{BOUNDARY}--\r\n".encode()
    pathlib.Path(out).write_bytes(body)
    print(f"multipart/form-data; boundary={BOUNDARY}")


if __name__ == "__main__":
    main()
