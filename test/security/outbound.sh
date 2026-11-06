#!/usr/bin/env bash
# Proves that crafted documents make LibreOffice fetch nothing and read no
# local files: remote images, linked sections, WEBSERVICE(), external cell
# links and file:// references, in DOCX, FODT, FODS and HTML. A control
# request from the same container must reach the listener, or the test
# proves nothing. Usage: test/security/outbound.sh [image]
set -u
IMAGE=${1:-rusticdoc:local}
HERE=$(cd "$(dirname "$0")" && pwd)
WORK=$(mktemp -d)
NAME=rusticdoc-outbound-$$
cd "$WORK" || exit 2
python3 -I "$HERE/gen_documents.py"
touch hits.log
python3 -I "$HERE/listener.py" &
LISTENER=$!
cleanup() { kill "$LISTENER" 2>/dev/null; docker rm -f "$NAME" >/dev/null 2>&1; rm -rf "$WORK"; }
trap cleanup EXIT
sleep 1
docker run -d --name "$NAME" --add-host host.docker.internal:host-gateway -p 127.0.0.1:3155:3000 "$IMAGE" >/dev/null
for _ in $(seq 1 60); do curl -sf -m 2 localhost:3155/health >/dev/null && break; sleep 1; done

docker exec "$NAME" python3 -c 'import urllib.request as u; u.urlopen("http://host.docker.internal:3199/control")' >/dev/null
if ! grep -q '^/control$' hits.log; then
    echo "FAIL: the control request did not arrive, so this run proves nothing"
    exit 2
fi
: > hits.log
status=0
for f in ext.docx ext.html ext.fodt ext.fods; do
    code=$(curl -s -m 60 -o "out-$f.pdf" -w '%{http_code}' -F "files=@$f" localhost:3155/forms/libreoffice/convert)
    echo "$f http=$code"
    [ "$code" = 200 ] || status=1
    if grep -aq 'root:x:0' "out-$f.pdf"; then
        echo "FAIL: $f leaked a local file"
        status=1
    fi
done
sleep 2
if [ -s hits.log ]; then
    echo "FAIL: documents reached the network:"
    cat hits.log
    status=1
else
    echo "no outbound requests from any document"
fi
exit $status
