#!/usr/bin/env bash
# Proves that crafted documents make LibreOffice fetch nothing and read no
# local files: remote images, linked sections, WEBSERVICE(), external cell
# links and file:// references, in DOCX, PPTX, XLSX, ODP, FODT, FODS, FODP,
# FODG, HTML, XHTML, SVG and RTF, through /v1/convert with and without
# landscape. A control request from the same container must reach the listener, or the test
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
docker run -d --name "$NAME" --add-host host.docker.internal:host-gateway -p 127.0.0.1:3155:9027 "$IMAGE" >/dev/null
for _ in $(seq 1 60); do curl -sf -m 2 localhost:3155/health >/dev/null && break; sleep 1; done

docker exec "$NAME" python3 -c 'import urllib.request as u; u.urlopen("http://host.docker.internal:3199/control")' >/dev/null
if ! grep -q '^/control$' hits.log; then
    echo "FAIL: the control request did not arrive, so this run proves nothing"
    exit 2
fi
: > hits.log
status=0
convert() { # file, label, path, extra curl arguments
    f=$1; label=$2; path=$3; shift 3
    code=$(curl -s -m 90 -o "out-$label-$f.pdf" -w '%{http_code}' -F "files=@$f" "$@" "localhost:3155$path")
    echo "$label $f http=$code"
    [ "$code" = 200 ] || status=1
    if grep -aq 'root:x:0' "out-$label-$f.pdf"; then
        echo "FAIL: $label $f leaked a local file"
        status=1
    fi
}
for f in ext.docx ext.html ext.fodt ext.fods ext.pptx ext.odp ext.fodp ext.fodg ext.svg ext.xhtml ext.rtf ext.xlsx web.html web.xhtml frames.html objects.fodt linked.docx; do
    convert "$f" v1 /v1/convert
    convert "$f" landscape /v1/convert -F landscape=true
done
# The text export opens a document for editing, so it gets its own pass.
for f in ext.docx ext.html ext.fodt ext.rtf linked.docx; do
    convert "$f" txt /v1/convert -F to=txt
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
