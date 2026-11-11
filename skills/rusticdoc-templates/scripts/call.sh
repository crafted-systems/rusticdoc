#!/usr/bin/env bash
# Calls the rusticdoc service. The response body goes to stdout (or where -o
# says), and the status line to stderr, so a failure is never mistaken for a
# document.
#
#   bash call.sh /v1/templates/validate -F template=@invoice.docx -F data=@invoice.json
#   bash call.sh /v1/render -F template=@invoice.docx -F data=@invoice.json -F steps=convert -o invoice.pdf
#
# Reads RUSTICDOC_URL (default http://localhost:9027) and RUSTICDOC_API_KEY.
# The key goes to curl on stdin, so it never appears in the process list; for
# the same reason, don't pass curl a body to read from stdin (`@-`).
set -u
if [ $# -lt 1 ]; then
  echo "usage: call.sh /v1/<path> [curl arguments...]" >&2
  exit 2
fi
url="${RUSTICDOC_URL:-http://localhost:9027}"
path="$1"
shift
status='%{stderr}\nHTTP %{http_code} %{content_type}\n'
if [ -n "${RUSTICDOC_API_KEY:-}" ]; then
  printf 'X-Api-Key: %s\n' "$RUSTICDOC_API_KEY" | curl -sS -H @- -w "$status" "$@" "${url%/}$path"
else
  curl -sS -w "$status" "$@" "${url%/}$path"
fi
