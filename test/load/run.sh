#!/usr/bin/env bash
# Posts the sample invoice to /v1/render with ApacheBench, at 1, 4, 8 and 16
# connections, and writes requests per second, latency, failures and container
# memory to out/results.md. The container runs with the flags and limits of
# deploy/compose.yaml. Needs docker, curl, unzip, python3 and ab.
#   test/load/run.sh [image]   (default rusticdoc:local)
#   RUSTICDOC_WORKERS and the other variables below change the container.
set -euo pipefail
cd "$(dirname "$0")"
image=${1:-rusticdoc:local}
python=${PYTHON:-python3}
ab=${AB:-ab}
samples=../../site/public/samples
name=rusticdoc-load
timeout=${RUSTICDOC_API_TIMEOUT:-60s}
workers=${RUSTICDOC_WORKERS:-2}
max_jobs=${RUSTICDOC_LIBREOFFICE_MAX_JOBS:-10}
render_workers=${RUSTICDOC_RENDER_WORKERS:-4}
render_memory=${RUSTICDOC_RENDER_MEMORY:-1GB}
mem_limit=${RUSTICDOC_MEM_LIMIT:-6g}
pids_limit=${RUSTICDOC_PIDS_LIMIT:-512}

# Name, template, steps, and the requests to send at 1, 4, 8 and 16 connections.
# The counts make each run last 15 to 30 seconds on the machine in the README.
scenarios="
text         invoice.txt                    -            10000 24000 30000 30000
docx         $samples/invoice-template.docx -            3000  10000 12000 12000
convert      $samples/invoice-template.docx convert      300   600   600   600
convert-pdfa $samples/invoice-template.docx convert,pdfa 180   300   300   300
pdfa         $samples/invoice-template.docx pdfa         350   600   600   600
"

# Whether a response is the filled document, not just a 200.
filled() {
  case $1 in
    text) grep -q 'Total due KES 36,300.00' "$2" ;;
    docx) unzip -p "$2" word/document.xml | grep -c 'Amani Otieno' >/dev/null ;;
    convert) [ "$(head -c 5 "$2")" = %PDF- ] ;;
    *pdfa) [ "$(head -c 5 "$2")" = %PDF- ] && grep -aq 'pdfaid:part' "$2" ;;
  esac
}

# The service's own request counts and LibreOffice pool, for report.py.
snapshot() {
  curl -sf "$url/metrics" | grep '^rusticdoc_requests_total{route="/v1/render"' || true
  curl -sf "$url/health"
}

rm -rf out && mkdir out
docker rm -f "$name" >/dev/null 2>&1 || true
docker run -d --name "$name" \
  --read-only --cap-drop ALL --security-opt no-new-privileges \
  --memory "$mem_limit" --pids-limit "$pids_limit" \
  --tmpfs /tmp:size=2g --tmpfs /home/rusticdoc:uid=1001,gid=1001,size=512m \
  -e RUSTICDOC_API_TIMEOUT="$timeout" -e RUSTICDOC_WORKERS="$workers" \
  -e RUSTICDOC_LIBREOFFICE_MAX_JOBS="$max_jobs" \
  -e RUSTICDOC_RENDER_WORKERS="$render_workers" -e RUSTICDOC_RENDER_MEMORY="$render_memory" \
  -p 127.0.0.1::9027 "$image" >/dev/null
trap 'kill "${stats:-}" 2>/dev/null || true; docker rm -f "$name" >/dev/null' EXIT
url=http://127.0.0.1:$(docker port "$name" 9027 | head -1 | cut -d: -f2)
for _ in $(seq 1 120); do curl -sf "$url/ready" >/dev/null && break; sleep 1; done
curl -sf "$url/ready" >/dev/null || { echo "$image did not become ready" >&2; docker logs "$name" >&2; exit 1; }

status=0
while read -r scenario template steps n1 n4 n8 n16 <&3; do
  [ -n "$scenario" ] || continue
  type=$("$python" -I body.py "$template" "$samples/invoice.json" "$steps" "out/$scenario.body")
  # One request by hand first. It also starts the LibreOffice workers.
  code=$(curl -s -o "out/$scenario.response" -w '%{http_code}' -H "Content-Type: $type" \
    --data-binary "@out/$scenario.body" "$url/v1/render")
  if [ "$code" != 200 ] || ! filled "$scenario" "out/$scenario.response"; then
    echo "$scenario: skipped, the check request got $code and no filled document" >&2
    status=1
    continue
  fi
  counts=("$n1" "$n4" "$n8" "$n16")
  i=0
  for c in 1 4 8 16; do
    n=${counts[$i]}; i=$((i + 1))
    run=out/$scenario-c$c
    sleep 2
    snapshot >"$run.before"
    docker stats --format '{{.MemUsage}}' "$name" >"$run.mem" </dev/null &
    stats=$!
    # -k keeps connections open, -l accepts responses of different lengths, and
    # -s is longer than the API timeout so the service answers before ab gives up.
    "$ab" -k -l -q -s 120 -n "$n" -c "$c" -p "out/$scenario.body" -T "$type" "$url/v1/render" >"$run.ab" 2>&1 || true
    kill "$stats" 2>/dev/null || true
    wait "$stats" 2>/dev/null || true
    snapshot >"$run.after"
    echo "$scenario $steps $c $n" >>out/runs.txt
    echo "$scenario at $c: $(grep -E '^(Requests per second|Non-2xx|Failed requests)' "$run.ab" | tr -s ' ' | paste -sd ';' -)"
  done
done 3<<<"$scenarios"

{
  echo "Run: $(date -u '+%Y-%m-%d %H:%M UTC')"
  echo "Image: $image, $(docker image inspect -f '{{.Id}}, {{.Architecture}}, created {{.Created}}' "$image")"
  echo "Host: $(sysctl -n machdep.cpu.brand_string 2>/dev/null || uname -m), $(getconf _NPROCESSORS_ONLN) cores, $(uname -sr)"
  echo "Docker: $(docker info -f '{{.ServerVersion}} on {{.OperatingSystem}}, {{.NCPU}} CPUs,') $(docker info -f '{{.MemTotal}}' | awk '{printf "%.2f GiB of memory", $1 / 1073741824}')"
  echo "Container: --memory $mem_limit, --pids-limit $pids_limit, /tmp 2g, home 512m;" \
    "docker stats shows a limit of $(grep -aoh '/ [0-9.]*[KMG]iB' out/*.mem | sort -u | tr -d '/ \n')"
  echo "Settings: RUSTICDOC_API_TIMEOUT=$timeout RUSTICDOC_WORKERS=$workers RUSTICDOC_LIBREOFFICE_MAX_JOBS=$max_jobs" \
    "RUSTICDOC_RENDER_WORKERS=$render_workers RUSTICDOC_RENDER_MEMORY=$render_memory"
  echo "Client: $("$ab" -V | sed -n '1s/^This is //p'), keep-alive, on the same machine"
  echo
  "$python" -I report.py out
} | tee out/results.md
exit $status
