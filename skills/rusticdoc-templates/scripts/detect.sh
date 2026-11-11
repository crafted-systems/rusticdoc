#!/usr/bin/env bash
# What this machine can render rusticdoc templates with. It only looks: it sends
# GET /health and GET /v1/version, runs `rusticdoc --version` and `rusticdoc
# engines`, and asks Docker which images it already has. It never pulls an
# image, and it never prints the API key, only whether one is set.
#
#   bash detect.sh
#
# Reads RUSTICDOC_URL (default http://localhost:9027) and RUSTICDOC_API_KEY.
# The last line is `use=service`, `use=cli`, `use=image`, `use=offer-image` or
# `use=none`.
set -u

url="${RUSTICDOC_URL:-http://localhost:9027}"
url="${url%/}"
image="ghcr.io/crafted-systems/rusticdoc:0.1.2"
service=no
cli=no
docker_ok=no
have_image=no

echo "== service"
echo "url=$url"
if [ -n "${RUSTICDOC_API_KEY:-}" ]; then echo "api_key=set"; else echo "api_key=unset"; fi
if command -v curl >/dev/null 2>&1; then
  health=$(curl -s --max-time 5 "$url/health" 2>/dev/null)
  if printf '%s' "$health" | grep -q '"status"'; then
    echo "health=$(printf '%s' "$health" | tr -d '\n' | cut -c1-300)"
    # The key goes to curl on stdin, so it never appears in the process list.
    if [ -n "${RUSTICDOC_API_KEY:-}" ]; then
      out=$(printf 'X-Api-Key: %s\n' "$RUSTICDOC_API_KEY" |
        curl -s --max-time 5 -w '\n%{http_code}' -H @- "$url/v1/version" 2>/dev/null)
    else
      out=$(curl -s --max-time 5 -w '\n%{http_code}' "$url/v1/version" 2>/dev/null)
    fi
    code=$(printf '%s' "$out" | tail -n1)
    body=$(printf '%s' "$out" | sed '$d' | tr -d '\n')
    case "$code" in
      200)
        echo "version=$(printf '%s' "$body" | sed -n 's/.*"version":"\([^"]*\)".*/\1/p')"
        service=yes ;;
      401 | 403)
        echo "auth=refused (HTTP $code): set RUSTICDOC_API_KEY to a key this service accepts" ;;
      *)
        echo "version=unknown (HTTP $code)" ;;
    esac
  else
    echo "health=unreachable"
  fi
else
  echo "curl=missing"
fi

echo "== cli"
if command -v rusticdoc >/dev/null 2>&1; then
  echo "path=$(command -v rusticdoc)"
  echo "version=$(rusticdoc --version 2>/dev/null)"
  rusticdoc engines 2>/dev/null | sed 's/^/engines: /'
  cli=yes
else
  echo "path=none"
fi

echo "== docker"
if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
  docker_ok=yes
  echo "docker=running"
  if docker image inspect "$image" >/dev/null 2>&1; then
    have_image=yes
    echo "image=$image (already pulled)"
  else
    echo "image=$image (not pulled; pulling it downloads the whole image, LibreOffice included)"
  fi
else
  echo "docker=unavailable"
fi

echo "=="
if [ "$service" = yes ]; then
  echo "use=service"
elif [ "$cli" = yes ]; then
  echo "use=cli"
elif [ "$have_image" = yes ]; then
  echo "use=image"
elif [ "$docker_ok" = yes ]; then
  echo "use=offer-image"
else
  echo "use=none"
fi
