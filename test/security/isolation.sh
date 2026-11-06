#!/usr/bin/env bash
# Proves that what runs beside the service cannot read its secrets: the environment
# of soffice, its bridge and a render worker hold none of them, and the service's
# own /proc/<pid>/environ and mem are closed to that user.
# Usage: test/security/isolation.sh [image]
set -u
IMAGE=${1:-rusticdoc:local}
NAME=rusticdoc-isolation-$$
HERE=$(cd "$(dirname "$0")" && pwd)
cleanup() { docker rm -f "$NAME" >/dev/null 2>&1; }
trap cleanup EXIT
docker run -d --name "$NAME" -e RUSTICDOC_API_KEYS=marker-api-key -e RUSTICDOC_WEBHOOK_SECRET=marker-secret \
    -p 127.0.0.1:3156:3000 "$IMAGE" >/dev/null
for _ in $(seq 1 60); do curl -sf -m 2 localhost:3156/health >/dev/null && break; sleep 1; done
# Start the workers.
printf 'hello' > /tmp/isolation-$$.txt
curl -s -m 60 -o /dev/null -H 'X-Api-Key: marker-api-key' -F files=@/tmp/isolation-$$.txt localhost:3156/v1/convert
rm -f /tmp/isolation-$$.txt
docker cp "$HERE/isolation_probe.py" "$NAME:/tmp/isolation_probe.py"
docker exec "$NAME" python3 -I /tmp/isolation_probe.py
