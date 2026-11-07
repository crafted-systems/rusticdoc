#!/usr/bin/env bash
# Runs several real containers on one store, hardened as deploy/compose.yaml runs them,
# with nothing shared but that store, and checks that what is uploaded through one
# works through the others: first on an S3-compatible bucket, then on a shared volume.
#   make replicas
set -uo pipefail
IMAGE=${1:-rusticdoc:local}
HERE=$(cd "$(dirname "$0")" && pwd)
RUN=rdstore$$
NET=$RUN-net
VOLUME=$RUN-volume
WORK=$(mktemp -d)
S3_SERVER=versity/versitygw@sha256:89cad13cc981986ee284cbafea4613f9533ce6fd5f37a2c96c4c4fb8adb3a316
passed=0
failed=0

cleanup() {
    docker ps -aq --filter "name=$RUN" | xargs -r docker rm -f >/dev/null 2>&1
    docker volume rm "$VOLUME" "$VOLUME-s3" >/dev/null 2>&1
    docker network rm "$NET" >/dev/null 2>&1
    rm -rf "$WORK"
}
trap cleanup EXIT

check() { # name, expected, actual
    if [ "$2" = "$3" ]; then
        echo "  ok    $1"
        passed=$((passed + 1))
    else
        echo "  FAIL  $1: expected '$2', got '$3'"
        failed=$((failed + 1))
    fi
}
holds() { # name, needle, haystack
    case "$3" in
    *"$2"*) check "$1" yes yes ;;
    *) check "$1" "contains '$2'" "$3" ;;
    esac
}
lacks() { # name, needle, haystack
    case "$3" in
    *"$2"*) check "$1" "without '$2'" "$3" ;;
    *) check "$1" yes yes ;;
    esac
}

url() { docker port "$RUN-$1" 9027/tcp | head -1; }
# Starts a replica and prints nothing. Further arguments go to docker run.
replica() {
    local name=$1
    shift
    docker run -d --name "$RUN-$name" --network "$NET" -p 127.0.0.1::9027 \
        --read-only --cap-drop ALL --security-opt no-new-privileges:true \
        --tmpfs /tmp:size=2g --tmpfs /home/rusticdoc:uid=1001,gid=1001,size=512m \
        -e RUSTICDOC_API_KEYS=alice-key,bob-key -e RUSTICDOC_ADMIN_KEYS=admin-key \
        -e RUSTICDOC_STORE_SYNC_INTERVAL=3s "$@" "$IMAGE" >/dev/null
}
until_ready() {
    for _ in $(seq 1 90); do
        [ "$(curl -s -o /dev/null -m 2 -w '%{http_code}' "http://$(url "$1")/ready")" = 200 ] && return 0
        sleep 1
    done
    echo "  $1 never became ready"
    docker logs "$RUN-$1" 2>&1 | tail -20
    return 1
}
code() { curl -s -o /dev/null -w '%{http_code}' -m 60 "$@"; }
json() { python3 -c "import json,sys; print(json.load(sys.stdin)$1)"; }
# Polls until a replica's font list does or does not hold the probe font.
font_listed() { # replica, wanted (yes/no)
    for _ in $(seq 1 30); do
        local list
        list=$(curl -s -m 5 -H 'x-api-key: alice-key' "http://$(url "$1")/v1/fonts")
        case "$list" in
        *"Rusticdoc Probe"*) [ "$2" = yes ] && { echo yes; return; } ;;
        *) [ "$2" = no ] && { echo no; return; } ;;
        esac
        sleep 1
    done
    [ "$2" = yes ] && echo no || echo yes
}
# The fonts a rendered PDF used, by the service's own header.
fonts_used() { # replica, template id
    curl -s -D - -o "$WORK/out.pdf" -m 60 -H 'x-api-key: alice-key' \
        -F "templateId=$2" -F 'data={"name":"Ada"}' -F steps=convert \
        "http://$(url "$1")/v1/render" | tr -d '\r' | grep -i '^x-fonts-used:'
}

# The scenario both kinds of store have to pass.
exercise() {
    local a=$1 b=$2
    echo " templates"
    local out id
    out=$(curl -s -m 60 -H 'x-api-key: alice-key' -F "template=@$WORK/letter.docx" "http://$(url "$a")/v1/templates")
    id=$(echo "$out" | json "['id']")
    holds "upload returns an id" "-" "$id"
    check "the format is read from the file" docx "$(echo "$out" | json "['format']")"
    local status
    status=$(curl -s -o "$WORK/out.pdf" -w '%{http_code}' -m 60 -H 'x-api-key: alice-key' \
        -F "templateId=$id" -F 'data={"name":"Ada"}' -F steps=convert "http://$(url "$b")/v1/render")
    check "another replica renders it by id" 200 "$status"
    check "and the result is a PDF" %PDF "$(head -c 4 "$WORK/out.pdf")"
    check "another replica lists it" "$id" "$(curl -s -H 'x-api-key: alice-key' "http://$(url "$b")/v1/templates" | json "['templates'][-1]['id']")"
    check "the content comes back unchanged" "$(shasum -a 256 <"$WORK/letter.docx")" \
        "$(curl -s -H 'x-api-key: alice-key' "http://$(url "$b")/v1/templates/$id/content" | shasum -a 256)"
    check "another caller cannot see it" 404 "$(code -H 'x-api-key: bob-key' "http://$(url "$b")/v1/templates/$id")"
    check "no credential gets nothing" 401 "$(code "http://$(url "$b")/v1/templates/$id")"
    printf '{%% for %%}' >"$WORK/broken.txt"
    check "the render worker refuses bad syntax at upload" 422 \
        "$(code -H 'x-api-key: alice-key' -F "template=@$WORK/broken.txt" "http://$(url "$a")/v1/templates")"

    echo " fonts"
    holds "before the upload the fallback is used" "x-fonts-used" "$(fonts_used "$b" "$id")"
    lacks "and not the probe font" "Probe" "$(fonts_used "$b" "$id")"
    check "a caller's key cannot upload fonts" 403 \
        "$(code -H 'x-api-key: alice-key' -F "file=@$WORK/fonts.zip" "http://$(url "$a")/v1/fonts")"
    out=$(curl -s -m 60 -H 'x-api-key: admin-key' -F "file=@$WORK/fonts.zip" "http://$(url "$a")/v1/fonts")
    local font
    font=$(echo "$out" | json "['fonts'][0]['id']")
    check "an admin's key can, and it reads the family" "Rusticdoc Probe" "$(echo "$out" | json "['fonts'][0]['faces'][0]['family']")"
    check "the licence is reported as skipped" LICENSE.txt "$(echo "$out" | json "['skipped'][0]['file']")"
    check "nothing escaped the ZIP's paths" absent \
        "$(docker exec "$RUN-$a" sh -c 'test -e /tmp/escaped && echo present || echo absent')"
    check "the uploading replica lists it at once" yes "$(font_listed "$a" yes)"
    holds "and renders with it at once" "Probe" "$(fonts_used "$a" "$id")"
    check "the other replica picks it up unprompted" yes "$(font_listed "$b" yes)"
    holds "and renders with it" "Probe" "$(fonts_used "$b" "$id")"
    # shellcheck disable=SC2086
    replica late ${REPLICA_ARGS:-}
    until_ready late
    holds "a replica started later has it from its first document" "Probe" "$(fonts_used late "$id")"
    docker rm -f "$RUN-late" >/dev/null

    echo " deletes"
    check "deleting a template works" 204 "$(code -X DELETE -H 'x-api-key: alice-key' "http://$(url "$a")/v1/templates/$id")"
    check "another replica refuses it at once, though it had it cached" 404 \
        "$(code -H 'x-api-key: alice-key' -F "templateId=$id" -F 'data={"name":"A"}' "http://$(url "$b")/v1/render")"
    check "a caller's key cannot delete a font" 403 "$(code -X DELETE -H 'x-api-key: alice-key' "http://$(url "$a")/v1/fonts/$font")"
    check "an admin's key can" 204 "$(code -X DELETE -H 'x-api-key: admin-key' "http://$(url "$a")/v1/fonts/$font")"
    check "the other replica drops it unprompted" no "$(font_listed "$b" no)"
}

docker network create "$NET" >/dev/null
docker run --rm --entrypoint sh "$IMAGE" -c 'cat /usr/share/fonts/truetype/dejavu/DejaVuSans.ttf' >"$WORK/source.ttf"
docker run --rm -v "$WORK:/work" -v "$HERE:/script:ro" python:3.13-slim sh -c \
    'pip install -q --disable-pip-version-check --root-user-action=ignore fonttools && python /script/make_fixtures.py /work/source.ttf /work' \
    || { echo "could not make the fixtures"; exit 1; }

echo "== an S3-compatible bucket"
docker volume create "$VOLUME-s3" >/dev/null
# On this server a bucket is a directory.
docker run --rm -v "$VOLUME-s3:/data" --entrypoint sh --user root "$IMAGE" -c 'mkdir -p /data/docs && chmod -R 777 /data'
docker run -d --name "$RUN-s3" --network "$NET" --network-alias s3server -v "$VOLUME-s3:/data" \
    "$S3_SERVER" --access storekey --secret storesecret-marker --port :9000 posix /data >/dev/null
sleep 2
S3=(-e RUSTICDOC_STORE=s3://docs/rusticdoc -e AWS_ENDPOINT=http://s3server:9000 -e AWS_ALLOW_HTTP=true
    -e AWS_REGION=us-east-1 -e AWS_ACCESS_KEY_ID=storekey -e AWS_SECRET_ACCESS_KEY=storesecret-marker)
REPLICA_ARGS="${S3[*]}"
replica a "${S3[@]}"
replica b "${S3[@]}"
until_ready a && until_ready b || exit 1
exercise a b

echo " when the bucket goes away"
out=$(curl -s -m 60 -H 'x-api-key: alice-key' -F "template=@$WORK/letter.docx" "http://$(url a)/v1/templates")
id=$(echo "$out" | json "['id']")
docker stop "$RUN-s3" >/dev/null
out=$(curl -s -m 60 -H 'x-api-key: alice-key' -F "templateId=$id" "http://$(url b)/v1/render")
holds "a render by id says the store is unavailable" store_unavailable "$out"
lacks "and does not name it" s3server "$out"
check "health stays up" 200 "$(code "http://$(url b)/health")"
check "a replica in service stays ready on the fonts in hand" 200 "$(code "http://$(url b)/ready")"
replica nostore "${S3[@]}"
sleep 15
check "a new replica waits, not ready" 503 "$(code "http://$(url nostore)/ready")"
docker start "$RUN-s3" >/dev/null
until_ready nostore
check "and is ready when the bucket returns" 200 "$(code "http://$(url nostore)/ready")"
docker rm -f "$RUN-nostore" >/dev/null

echo " secrets"
check "the store's secret is closed to same-user processes" unreadable \
    "$(docker exec "$RUN-a" sh -c 'cat /proc/1/environ >/dev/null 2>&1 && echo readable || echo unreadable')"
lacks "and stays out of the logs" storesecret-marker "$(docker logs "$RUN-a" 2>&1)"
docker rm -f "$RUN-a" "$RUN-b" >/dev/null

echo "== a shared volume"
docker volume create "$VOLUME" >/dev/null
docker run --rm -v "$VOLUME:/data" --entrypoint sh --user root "$IMAGE" -c 'chown 1001:1001 /data'
FILE=(-v "$VOLUME:/data" -e RUSTICDOC_STORE=file:///data/store)
REPLICA_ARGS="${FILE[*]}"
replica a "${FILE[@]}"
replica b "${FILE[@]}"
until_ready a && until_ready b || exit 1
exercise a b

echo
echo "$passed passed, $failed failed"
[ "$failed" -eq 0 ]
