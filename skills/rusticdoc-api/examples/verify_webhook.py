#!/usr/bin/env python3
"""A rusticdoc webhook receiver that checks each delivery's signature.

    RUSTICDOC_SIGNING_KEY=<hex> PORT=8000 python3 verify_webhook.py

RUSTICDOC_SIGNING_KEY is the hex that `rusticdoc webhook-secret` prints for
the calling key, run where RUSTICDOC_WEBHOOK_SECRET is set. Standard library
only. A verified result is saved under its file name in ./received.
"""
import hashlib
import hmac
import os
import re
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

KEY = bytes.fromhex(os.environ["RUSTICDOC_SIGNING_KEY"])
MAX_AGE_SECONDS = 300
seen = set()  # request ids already handled; deliveries can repeat


def verified(headers, body):
    timestamp = headers.get("X-Rusticdoc-Timestamp", "")
    event = headers.get("X-Rusticdoc-Event", "")
    request_id = headers.get("X-Request-Id", "")
    signature = headers.get("X-Rusticdoc-Signature", "")
    if not (timestamp.isdigit() and event and request_id and signature.startswith("v1=")):
        return False
    if abs(time.time() - int(timestamp)) > MAX_AGE_SECONDS:
        return False
    # Over the raw body bytes: a parsed or re-encoded body won't verify.
    message = f"{timestamp}.{event}.{request_id}.".encode() + body
    expected = hmac.new(KEY, message, hashlib.sha256).hexdigest()
    return hmac.compare_digest(signature[3:].encode(), expected.encode())


class Receiver(BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        ok = verified(self.headers, body)
        event = self.headers.get("X-Rusticdoc-Event")
        request_id = self.headers.get("X-Request-Id")
        print("verified" if ok else "REJECTED", event, request_id, len(body), flush=True)
        if ok and request_id not in seen:
            seen.add(request_id)
            if event == "render.completed":
                name = re.search(r'filename="([^"/\\]+)"', self.headers.get("Content-Disposition", ""))
                os.makedirs("received", exist_ok=True)
                with open(os.path.join("received", name[1] if name else request_id), "wb") as f:
                    f.write(body)
            else:
                print("failure:", body.decode("utf-8", "replace"), flush=True)
        # Any 2xx stops the retries; anything else asks for another attempt.
        self.send_response(204 if ok else 401)
        self.end_headers()

    def log_message(self, *args):
        pass


ThreadingHTTPServer(("0.0.0.0", int(os.environ.get("PORT", 8000))), Receiver).serve_forever()
