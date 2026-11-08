#!/usr/bin/env bash
# Signs PDFs with the CLI and has independent validators check them: poppler's
# pdfsig always, pyHanko when it is installed. Throwaway RSA and ECDSA P-256 keys
# with self-signed certificates are made with openssl for the run; they sign
# nothing else. pyHanko is given them as trust roots, so its verdict covers the
# certificate as well as the integrity pdfsig checks.
#
# Usage: test/sign/verify.sh [path/to/rusticdoc]
# Needs openssl and pdfsig. PDFSIG and PYHANKO name other binaries.
set -u
cd "$(dirname "$0")/../.." || exit 2
ROOT=$(pwd)
if [ $# -ge 1 ]; then
    BIN=$1
else
    cargo build -q -p rusticdoc || exit 2
    BIN=$ROOT/target/debug/rusticdoc
fi
PDFSIG=${PDFSIG:-$(command -v pdfsig)}
PYHANKO=${PYHANKO:-$(command -v pyhanko)}
[ -x "$PDFSIG" ] || { echo "pdfsig (poppler-utils) is required"; exit 2; }
command -v openssl >/dev/null || { echo "openssl is required"; exit 2; }
[ -x "$PYHANKO" ] || echo "note: pyHanko not found; set PYHANKO to check with it too"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
status=0
fail() { echo "FAIL: $1"; status=1; }
pass() { echo "ok: $1"; }

cargo run -q -p rusticdoc-core --example pdfops_fixtures -- write "$WORK/fx" || exit 2
umask 077
openssl req -x509 -newkey rsa:2048 -nodes -days 2 \
    -keyout "$WORK/rsa-key.pem" -out "$WORK/rsa-cert.pem" \
    -subj "/CN=rusticdoc test RSA signer/O=rusticdoc tests" \
    -addext "keyUsage=critical,digitalSignature,nonRepudiation" 2>/dev/null || exit 2
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 2 \
    -keyout "$WORK/ec-key.pem" -out "$WORK/ec-cert.pem" \
    -subj "/CN=rusticdoc test EC signer/O=rusticdoc tests" \
    -addext "keyUsage=critical,digitalSignature" 2>/dev/null || exit 2
# SEC1, the format `openssl ecparam -genkey` writes, to load that form too.
openssl ec -in "$WORK/ec-key.pem" -out "$WORK/ec-sec1.pem" 2>/dev/null || exit 2

# pdfsig checks integrity and coverage only: NSS will not take a self-signed
# leaf as a trust anchor, so its certificate verdict would only ever be
# "issuer unknown". pyHanko is given the certificates as trust roots instead.
NSS=(-nocert)

sign() { # name, then arguments for `rusticdoc pdf sign`
    local name=$1; shift
    if ! "$BIN" pdf sign "$@" -o "$WORK/$name.pdf" 2>"$WORK/$name.log"; then
        fail "$name: rusticdoc pdf sign failed: $(cat "$WORK/$name.log")"
        return 1
    fi
}

# What pdfsig says of signature N: valid, and covering the whole file or not.
check_pdfsig() { # name, signature number, total|partial
    local name=$1 n=$2 coverage=$3 report section
    report=$("$PDFSIG" "${NSS[@]}" "$WORK/$name.pdf" 2>/dev/null)
    section=$(printf '%s\n' "$report" | awk -v n="Signature #$n:" '$0==n{on=1;next} /^Signature #/{on=0} on')
    if [ -z "$section" ]; then
        fail "$name: pdfsig found no signature #$n"; printf '%s\n' "$report"; return
    fi
    printf '%s\n' "$section" | grep -q "Signature Validation: Signature is Valid." \
        || { fail "$name #$n: pdfsig does not find the signature valid"; printf '%s\n' "$section"; return; }
    printf '%s\n' "$section" | grep -q "Signature Type: ETSI.CAdES.detached" \
        || fail "$name #$n: not ETSI.CAdES.detached"
    if [ "$coverage" = total ]; then
        printf '%s\n' "$section" | grep -q "^  - Total document signed" \
            || fail "$name #$n: pdfsig says the signature does not cover the whole document"
    else
        printf '%s\n' "$section" | grep -q "Not total document signed" \
            || fail "$name #$n: an earlier signature should cover only its revision"
    fi
    pass "pdfsig $name #$n: valid, $coverage"
}

check_pyhanko() { # name, expected count of valid signatures
    [ -x "$PYHANKO" ] || return 0
    local name=$1 count=$2 out
    out=$("$PYHANKO" sign validate --executive-summary --trust-replace \
        --trust "$WORK/rsa-cert.pem" --trust "$WORK/ec-cert.pem" "$WORK/$name.pdf" 2>/dev/null)
    if [ "$(printf '%s\n' "$out" | grep -c ':VALID$')" -eq "$count" ]; then
        pass "pyHanko $name: $count valid"
    else
        fail "pyHanko $name: expected $count valid signatures"; printf '%s\n' "$out"
    fi
}

sign rsa-invisible "$WORK/fx/three-pages.pdf" --key "$WORK/rsa-key.pem" --cert "$WORK/rsa-cert.pem" \
    --reason "Approved" --location "Nairobi" --contact-info "ops@example.com" --signer-name "Zoë Test" \
    && { check_pdfsig rsa-invisible 1 total; check_pyhanko rsa-invisible 1; }

sign ec-visible "$WORK/fx/form.pdf" --key "$WORK/ec-sec1.pem" --cert "$WORK/ec-cert.pem" \
    --image "$WORK/fx/dot.png" --page 1 --x 100 --y 72 --width 120 \
    && { check_pdfsig ec-visible 1 total; check_pyhanko ec-visible 1; }

# A signed file signed again: the first signature still holds for its revision.
if [ -f "$WORK/ec-visible.pdf" ] && sign twice "$WORK/ec-visible.pdf" --key "$WORK/rsa-key.pem" --cert "$WORK/rsa-cert.pem"; then
    check_pdfsig twice 1 partial
    check_pdfsig twice 2 total
    check_pyhanko twice 2
fi

# The original bytes are untouched: the input is a prefix of the output.
if [ -f "$WORK/rsa-invisible.pdf" ]; then
    size=$(($(wc -c < "$WORK/fx/three-pages.pdf")))
    # Not `cmp -n`: on macOS it reports a difference at the shorter file's end.
    if head -c "$size" "$WORK/rsa-invisible.pdf" | cmp -s - "$WORK/fx/three-pages.pdf"; then
        pass "the original bytes are a prefix of the signed file"
    else
        fail "the signed file does not start with the original bytes"
    fi
fi

# A changed byte inside the signed range must break the signature.
if [ -f "$WORK/rsa-invisible.pdf" ]; then
    python3 -I - "$WORK/rsa-invisible.pdf" "$WORK/tampered.pdf" <<'EOF'
import sys
data = bytearray(open(sys.argv[1], "rb").read())
at = data.find(b"Page 2")
data[at + 5] = ord("9")
open(sys.argv[2], "wb").write(data)
EOF
    if "$PDFSIG" "${NSS[@]}" "$WORK/tampered.pdf" 2>/dev/null | grep -q "Signature is Valid."; then
        fail "pdfsig accepts a tampered file"
    else
        pass "pdfsig refuses a tampered file"
    fi
fi

[ $status -eq 0 ] && echo "all signature checks passed" || echo "signature checks FAILED"
exit $status
