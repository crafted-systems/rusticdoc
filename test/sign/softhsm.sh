#!/usr/bin/env bash
# Signs with keys held by SoftHSM2 through PKCS#11, in a Debian container, at
# every level against the test PKI's timestamp authority, OCSP responder and
# CRL (test/sign/pki.sh), all inside the container. The signed files are left
# in OUT for validators outside it.
#
# Usage: test/sign/softhsm.sh path/to/linux/rusticdoc OUT
# The binary must be built for the container's architecture, e.g. with
#   docker run --rm -v "$PWD:/src" -w /src rust:1.99.1-slim-trixie cargo build -p rusticdoc
# Needs docker.
set -eu
cd "$(dirname "$0")/../.."
BIN=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
OUT=$2
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
cargo run -q -p rusticdoc-core --example pdfops_fixtures -- write "$OUT/fx"

docker run --rm -i \
    -v "$BIN:/usr/local/bin/rusticdoc:ro" \
    -v "$PWD/test/sign:/sign:ro" \
    -v "$OUT:/out" \
    rust:1.99.1-slim-trixie /bin/bash -s <<'EOF'
set -eu
apt-get update -qq
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
    softhsm2 opensc openssl python3 curl >/dev/null
status=0
fail() { echo "FAIL: $1"; status=1; }
pass() { echo "ok: $1"; }

source /sign/pki.sh
pki_setup /work
trap pki_stop EXIT

# A token with the two signing keys imported, and nothing else.
export SOFTHSM2_CONF=/work/softhsm2.conf
mkdir -p /work/tokens
echo "directories.tokendir = /work/tokens" > "$SOFTHSM2_CONF"
MODULE=/usr/lib/softhsm/libsofthsm2.so
softhsm2-util --init-token --free --label rusticdoc --pin 4321 --so-pin 87654321 >/dev/null
softhsm2-util --import "$PKI/signer.key" --token rusticdoc --label signer --id 01 --pin 4321 >/dev/null
softhsm2-util --import "$PKI/ec-signer.key" --token rusticdoc --label ec-signer --id 02 --pin 4321 >/dev/null
pkcs11-tool --module "$MODULE" --token-label rusticdoc --login --pin 4321 --list-objects --type privkey

umask 077
printf '4321\n' > /work/pin
# The key files are gone from here on: only the token can sign.
rm -f "$PKI/signer.key" "$PKI/ec-signer.key"
export RUSTICDOC_PKCS11_MODULE=$MODULE RUSTICDOC_PKCS11_TOKEN=rusticdoc RUSTICDOC_PKCS11_PIN_FILE=/work/pin
export RUSTICDOC_SIGNING_TSA_URL=$PKI_TSA_URL RUSTICDOC_SIGNING_TSA_CERT=$PKI/root.pem
export RUSTICDOC_SIGNING_TSA_ALLOW_HTTP=true
export RUSTICDOC_SIGNING_OUTBOUND_ALLOW_PRIVATE=true

for level in b-b b-t b-lt b-lta; do
    if rusticdoc pdf sign /out/fx/three-pages.pdf --key pkcs11:signer --cert "$PKI/signer-chain.pem" \
        --level "$level" --reason "PKCS#11 $level" -o "/out/hsm-rsa-$level.pdf"; then
        pass "RSA key on SoftHSM signs at $level"
    else
        fail "RSA key on SoftHSM at $level"
    fi
done
if rusticdoc pdf sign /out/fx/form.pdf --key "pkcs11:object=ec-signer;token=rusticdoc" \
    --cert "$PKI/ec-signer-chain.pem" --level b-lta --x 72 --y 72 -o /out/hsm-ec-b-lta.pdf; then
    pass "ECDSA P-256 key on SoftHSM signs at b-lta, shown as text"
else
    fail "ECDSA key on SoftHSM"
fi
# A key that does not belong to the certificate is refused before signing.
if rusticdoc pdf sign /out/fx/three-pages.pdf --key pkcs11:ec-signer --cert "$PKI/signer-chain.pem" \
    -o /out/mismatch.pdf 2>/out/mismatch.log; then
    fail "a token key with someone else's certificate signed"
else
    grep -q "does not belong" /out/mismatch.log && pass "a token key with the wrong certificate is refused" \
        || { fail "wrong refusal"; cat /out/mismatch.log; }
fi
# A wrong PIN is refused, and says so without showing it.
printf '0000\n' > /work/bad-pin
if RUSTICDOC_PKCS11_PIN_FILE=/work/bad-pin rusticdoc pdf sign /out/fx/three-pages.pdf --key pkcs11:signer \
    --cert "$PKI/signer-chain.pem" -o /out/bad-pin.pdf 2>/out/bad-pin.log; then
    fail "a wrong PIN signed"
else
    grep -qi "log in" /out/bad-pin.log && ! grep -q 0000 /out/bad-pin.log \
        && pass "a wrong PIN is refused" || { fail "wrong PIN message"; cat /out/bad-pin.log; }
fi
cp "$PKI/root.pem" /out/root.pem
exit $status
EOF
