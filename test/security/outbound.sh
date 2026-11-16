#!/usr/bin/env bash
# Proves that crafted documents make the service fetch nothing and read no
# local files: remote images, linked sections, WEBSERVICE(), external cell
# links and file:// references, in DOCX, PPTX, XLSX, ODP, FODT, FODS, FODP,
# FODG, HTML, XHTML, SVG and RTF, through /v1/convert with and without
# landscape and with both HTML engines; the same as templates through
# /v1/render, with and without `keep`; a picture in a template's data; and a
# PDF naming files and URLs through the PDF routes. A control request from the
# same container must reach the listener, and the check for a leaked file must
# find a line it was given inside a PDF, or the test proves nothing.
# Usage: test/security/outbound.sh [image]
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
# True when an output holds a line of /etc/passwd. A PDF's text is compressed, so
# a grep of the file sees none of it: the image's own pdftotext reads it first.
# Anything else, such as the text export or an error body, is read as it is.
leaked() { # output file
    if [ "$(head -c 5 "$1")" != "%PDF-" ]; then
        grep -aq 'root:x:0' "$1"
        return
    fi
    if ! docker exec -i "$NAME" pdftotext - - < "$1" > "$1.text" 2>/dev/null; then
        echo "FAIL: the text of $1 could not be read"
        status=1
        return 1
    fi
    grep -q 'root:x:0' "$1.text" || grep -aq 'root:x:0' "$1"
}
printf 'root:x:0:0:control\n' > control.txt
curl -s -m 90 -o control.pdf -F files=@control.txt localhost:3155/v1/convert
if [ "$(head -c 5 control.pdf)" != "%PDF-" ] || ! leaked control.pdf; then
    echo "FAIL: the leak check did not find a line it was given, so this run proves nothing"
    exit 2
fi
ask() { # label, the file it is about, path, curl arguments
    label=$1; f=$2; path=$3; shift 3
    code=$(curl -s -m 90 -o "out-$label-$f.pdf" -w '%{http_code}' "$@" "localhost:3155$path")
    echo "$label $f http=$code"
    [ "$code" = 200 ] || status=1
    if leaked "out-$label-$f.pdf"; then
        echo "FAIL: $label $f leaked a local file"
        status=1
    fi
}
convert() { # file, label, path, extra curl arguments
    f=$1; label=$2; path=$3; shift 3
    ask "$label" "$f" "$path" -F "files=@$f" "$@"
}
for f in ext.docx ext.html ext.fodt ext.fods ext.pptx ext.odp ext.fodp ext.fodg ext.svg ext.xhtml ext.rtf ext.xlsx web.html web.xhtml frames.html objects.fodt linked.docx; do
    convert "$f" v1 /v1/convert
    convert "$f" landscape /v1/convert -F landscape=true
done
# The text export opens a document for editing, so it gets its own pass.
for f in ext.docx ext.html ext.fodt ext.rtf linked.docx; do
    convert "$f" txt /v1/convert -F to=txt
done
# HTML goes to Fulgur unless a request asks for LibreOffice, which loads a page
# its own way.
for f in ext.html ext.xhtml web.html web.xhtml frames.html; do
    convert "$f" libreoffice /v1/convert -F htmlEngine=libreoffice
done
# Rendering takes what names something outside out of a template, and `keep`
# leaves it in: the conversion that follows must fetch nothing either way.
for f in ext.docx linked.docx ext.xlsx ext.pptx ext.odp; do
    ask render "$f" /v1/render -F "template=@$f" -F 'data={}' -F steps=convert
    ask keep "$f" /v1/render -F "template=@$f" -F 'data={}' -F steps=convert -F keep=all
done
for f in ext.html web.html; do
    ask render "$f" /v1/render -F "template=@$f" -F 'data={}' -F steps=convert
done
# A picture that arrives as data is drawn by the service itself.
ask picture picture.docx /v1/render -F template=@picture.docx -F data=@picture.json -F steps=convert
# The PDF routes read a PDF that names files and URLs, and one of them hands it
# to LibreOffice.
ask merge ext.pdf /v1/pdf/merge -F files=@ext.pdf -F files=@ext.pdf
ask flatten ext.pdf /v1/pdf/flatten -F files=@ext.pdf
ask optimize ext.pdf /v1/pdf/optimize -F files=@ext.pdf
ask rotate ext.pdf /v1/pdf/rotate -F files=@ext.pdf -F rotation=90
ask pdfa ext.pdf /v1/pdf/convert -F files=@ext.pdf -F pdfa=PDF/A-2b
ask metadata ext.pdf /v1/pdf/metadata/read -F files=@ext.pdf
ask fields ext.pdf /v1/pdf/forms/fields -F files=@ext.pdf
sleep 2
if [ -s hits.log ]; then
    echo "FAIL: documents reached the network:"
    cat hits.log
    status=1
else
    echo "no outbound requests from any document"
fi
exit $status
