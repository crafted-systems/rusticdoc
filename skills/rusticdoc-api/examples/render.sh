#!/usr/bin/env bash
# The common calls, start to finish, against a running service. Each response's
# status is checked, so a JSON error is never saved as a document.
#
#   RUSTICDOC_URL=http://localhost:9027 RUSTICDOC_API_KEY=... bash render.sh TEMPLATE DATA
#
# TEMPLATE is any template (for example ../../rusticdoc-templates/examples/invoice.docx)
# and DATA its JSON. Writes out.pdf, out-pdfa.pdf and, when the service has a store,
# out-stored.pdf and out-job.pdf, in the current folder.
set -euo pipefail
template="${1:?usage: render.sh TEMPLATE DATA}"
data="${2:?usage: render.sh TEMPLATE DATA}"
url="${RUSTICDOC_URL:-http://localhost:9027}"
url="${url%/}"

# curl with the key from the environment, sent on stdin so it stays out of the
# process list. Prints the HTTP status; the body goes where -o says.
rd() {
  if [ -n "${RUSTICDOC_API_KEY:-}" ]; then
    printf 'X-Api-Key: %s\n' "$RUSTICDOC_API_KEY" | curl -sS -H @- -w '%{http_code}' "$@"
  else
    curl -sS -w '%{http_code}' "$@"
  fi
}
expect() { # expect STATUS WHAT FILE
  if [ "$1" != "$2" ]; then
    echo "$3: HTTP $1, wanted $2: $(head -c 300 "$4")" >&2
    exit 1
  fi
}

# 1. Validate first: always 200, so read "ok".
status=$(rd -o validate.json -F "template=@$template" -F "data=<$data" "$url/v1/templates/validate")
expect "$status" 200 validate validate.json
if ! grep -q '"ok":true' validate.json; then
  echo "the template has errors:" >&2
  cat validate.json >&2
  exit 1
fi
echo "validated"

# 2. Render to PDF.
status=$(rd -o out.pdf -F "template=@$template" -F "data=<$data" -F steps=convert "$url/v1/render")
expect "$status" 200 render out.pdf
echo "wrote out.pdf"

# 3. PDF/A-2b with a title. On an Office file, pdfa converts directly.
status=$(rd -o out-pdfa.pdf -F "template=@$template" -F "data=<$data" \
  -F steps=pdfa,metadata -F 'metadata={"Title":"Rendered by rusticdoc"}' "$url/v1/render")
expect "$status" 200 pdfa out-pdfa.pdf
echo "wrote out-pdfa.pdf"

# 4. Store the template and render it by id with a JSON body. Needs RUSTICDOC_STORE.
status=$(rd -o stored.json -F "template=@$template" "$url/v1/templates")
if [ "$status" = 501 ]; then
  echo "the service has no store, so stored templates and jobs are skipped"
  exit 0
fi
expect "$status" 201 store stored.json
id=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["id"])' stored.json)
python3 - "$id" "$data" >body.json <<'EOF'
import json, sys
print(json.dumps({"templateId": sys.argv[1], "data": json.load(open(sys.argv[2])), "steps": ["convert"]}))
EOF
status=$(rd -o out-stored.pdf -H 'Content-Type: application/json' -d @body.json "$url/v1/render")
expect "$status" 200 "render by id" out-stored.pdf
echo "wrote out-stored.pdf from template $id"

# 5. The same as a background job: 202, then poll, then fetch.
status=$(rd -o job.json -H 'Prefer: respond-async' -H 'Content-Type: application/json' -d @body.json "$url/v1/render")
expect "$status" 202 "start job" job.json
job=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["id"])' job.json)
for _ in $(seq 1 120); do
  rd -o state.json "$url/v1/jobs/$job" >/dev/null
  state=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["status"])' state.json)
  [ "$state" = done ] || [ "$state" = failed ] && break
  sleep 1
done
if [ "$state" != done ]; then
  echo "job $job ended as $state: $(cat state.json)" >&2
  exit 1
fi
status=$(rd -o out-job.pdf "$url/v1/jobs/$job/result")
expect "$status" 200 "job result" out-job.pdf
echo "wrote out-job.pdf from job $job"

# 6. Tidy up: delete the stored template. Ask before doing this to a user's templates.
status=$(rd -o /dev/null -X DELETE "$url/v1/templates/$id")
expect "$status" 204 delete /dev/null
rm -f validate.json stored.json body.json job.json state.json
echo "deleted template $id"
