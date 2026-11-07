#!/usr/bin/env bash
# Proves that one conversion cannot fill the disk. LibreOffice's text export never
# finishes a document whose last element is a table. The bridge stops that, and
# the pool's disk limit stops whatever else writes without end, so both are
# tested: the second with the first taken out of the bridge, against the real
# runaway export. Usage: test/security/disk.sh [image]
set -u
IMAGE=${1:-rusticdoc:local}
WORK=$(mktemp -d)
NAME=rusticdoc-disk-$$
PORT=3156
cleanup() { docker rm -f "$NAME" >/dev/null 2>&1; rm -rf "$WORK"; }
trap cleanup EXIT
cd "$WORK" || exit 2
status=0
fail() { echo "FAIL: $1"; status=1; }

python3 -I - <<'EOF'
import zipfile
BODY = '<w:p><w:r><w:t>before</w:t></w:r></w:p><w:tbl><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>in the table</w:t></w:r></w:p></w:tc></w:tr></w:tbl>'
with zipfile.ZipFile("ends-in-table.docx", "w") as z:
    z.writestr("[Content_Types].xml", '<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>')
    z.writestr("_rels/.rels", '<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>')
    z.writestr("word/document.xml", '<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>%s</w:body></w:document>' % BODY)
open("ends-in-table.html", "w").write("<html><body><p>before</p><table><tr><td>in the table</td></tr></table></body></html>")
open("ends-in-table.rtf", "w").write(r"{\rtf1\ansi before\par\trowd\cellx2000 in the table\cell\row}")
open("ends-in-table.fodt", "w").write('<?xml version="1.0"?><office:document xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" office:version="1.2" office:mimetype="application/vnd.oasis.opendocument.text"><office:body><office:text><text:p>before</text:p><table:table table:name="T"><table:table-column/><table:table-row><table:table-cell><text:p>in the table</text:p></table:table-cell></table:table-row></table:table></office:text></office:body></office:document>')
EOF
cat > bridge-without-fix.sh <<'EOF'
#!/bin/sh
# Runs the bridge with the paragraph fix taken out, to reach the disk limit.
script=$1; shift
copy=/tmp/unobridge-unfixed-$$.py
sed 's/^\( *\)end_with_paragraph(doc)$/\1pass/' "$script" > "$copy"
exec python3 "$copy" "$@"
EOF
chmod 755 bridge-without-fix.sh

start() { # extra docker arguments
    docker rm -f "$NAME" >/dev/null 2>&1
    docker run -d --name "$NAME" --tmpfs /tmp:rw,size=512m,mode=1777 -p "127.0.0.1:$PORT:9027" "$@" "$IMAGE" >/dev/null
    for _ in $(seq 1 60); do curl -sf -m 2 "localhost:$PORT/health" >/dev/null && return; sleep 1; done
    fail "the service did not start"; exit 2
}
used_mb() { docker exec "$NAME" df -m /tmp | awk 'END {print $3}'; }

echo "Documents that end in a table export to text"
start
for f in ends-in-table.docx ends-in-table.html ends-in-table.rtf ends-in-table.fodt; do
    code=$(curl -s -m 25 -o "out-$f.txt" -w '%{http_code}' -F "files=@$f" -F to=txt "localhost:$PORT/v1/convert")
    size=$(wc -c < "out-$f.txt" | tr -d ' ')
    echo "  $f http=$code bytes=$size"
    [ "$code" = 200 ] || fail "$f answered $code"
    grep -q 'in the table' "out-$f.txt" || fail "$f lost its table"
    [ "$size" -lt 200 ] || fail "$f wrote its text more than once"
done
[ "$(used_mb)" -lt 50 ] || fail "/tmp holds $(used_mb) MB after those"

echo "The limit stops the real runaway export, and other conversions carry on"
cp ends-in-table.docx small.docx
start -v "$WORK/bridge-without-fix.sh:/bridge-without-fix.sh:ro" \
    -e RUSTICDOC_PYTHON=/bridge-without-fix.sh -e RUSTICDOC_LIBREOFFICE_MAX_DISK=64MB
started=$(date +%s)
curl -s -m 60 -o runaway.json -w '%{http_code}' -F files=@ends-in-table.docx -F to=txt "localhost:$PORT/v1/convert" > runaway.code &
runaway=$!
code=$(curl -s -m 25 -o other.pdf -w '%{http_code}' -F files=@small.docx "localhost:$PORT/v1/convert")
echo "  a conversion alongside it http=$code"
[ "$code" = 200 ] || fail "a conversion alongside the runaway answered $code"
wait "$runaway"
took=$(($(date +%s) - started))
echo "  runaway http=$(cat runaway.code) after ${took}s: $(cat runaway.json)"
[ "$(cat runaway.code)" = 400 ] || fail "the runaway answered $(cat runaway.code)"
grep -q 'wrote more than the limit' runaway.json || fail "the refusal does not say why"
[ "$took" -lt 25 ] || fail "the runaway ran to the time limit, ${took}s"
sleep 1
[ "$(used_mb)" -lt 50 ] || fail "/tmp still holds $(used_mb) MB"
for _ in $(seq 1 30); do [ "$(curl -s -m 2 "localhost:$PORT/health" | grep -c '"status":"up"')" = 1 ] && break; sleep 1; done
code=$(curl -s -m 25 -o /dev/null -w '%{http_code}' -F files=@small.docx "localhost:$PORT/v1/convert")
echo "  the next conversion http=$code"
[ "$code" = 200 ] || fail "the next conversion answered $code"

if [ "$status" = 0 ]; then echo "PASS"; fi
exit "$status"
