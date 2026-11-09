#!/usr/bin/env bash
# A throwaway PKI with a real timestamp authority, OCSP responder and CRL
# distribution point on loopback, for checking PAdES B-T, B-LT and B-LTA.
#
#   source test/sign/pki.sh
#   pki_setup "$WORK"    # makes the PKI under $WORK/pki, sets PKI_*, starts the servers
#   pki_stop             # stops them (trap it on EXIT)
#   pki_make "$WORK"     # the PKI alone, as the unit tests use it
#
# The root CA issues:
#   signer.pem     the signing certificate, RSA, with an AIA OCSP URL and a CRL
#                  distribution point
#   ec-signer.pem  the same with an ECDSA P-256 key
#   tsa.pem        the timestamp authority, critical timeStamping EKU, with a
#                  CRL distribution point only (so the CRL path is used too)
#   ocsp.pem       the OCSP responder, OCSPSigning EKU and id-pkix-ocsp-nocheck
# and rogue.pem, a self-signed impostor with the root's name, with rogue.crl.
# The timestamp authority is `openssl ts -reply` behind a small Python HTTP
# handler, which also serves the CRL and the root certificate; the OCSP
# responder is `openssl ocsp -port`.
#
# Needs openssl (3 or later), python3 and curl.

pki_setup() {
    pki_make "$1" && pki_start
}

# The certificates, keys, CRL and configuration, without the servers. Takes
# PKI_HTTP_PORT and PKI_OCSP_PORT from the environment when set, so the URLs in
# the certificates can name places that are not served (the unit tests answer
# them without a network).
pki_make() {
    local dir=$1
    PKI=$dir/pki
    mkdir -p "$PKI/newcerts" || return 1
    : > "$PKI/index.txt"
    echo 1000 > "$PKI/serial"
    echo 1000 > "$PKI/crlnumber"
    echo 01 > "$PKI/tsaserial"
    if [ -z "${PKI_HTTP_PORT:-}" ] || [ -z "${PKI_OCSP_PORT:-}" ]; then
        read -r PKI_HTTP_PORT PKI_OCSP_PORT < <(python3 -I -c '
import socket
socks = [socket.socket() for _ in range(2)]
for s in socks: s.bind(("127.0.0.1", 0))
print(*[s.getsockname()[1] for s in socks])
for s in socks: s.close()')
    fi
    export PKI PKI_HTTP_PORT PKI_OCSP_PORT
    export PKI_TSA_URL="http://127.0.0.1:$PKI_HTTP_PORT/tsa"

    cat > "$PKI/ca.cnf" <<EOF
[ ca ]
default_ca = root
[ root ]
dir = $PKI
database = \$dir/index.txt
new_certs_dir = \$dir/newcerts
certificate = \$dir/root.pem
private_key = \$dir/root.key
serial = \$dir/serial
crlnumber = \$dir/crlnumber
default_md = sha256
default_days = 30
default_crl_days = 7
policy = anything
unique_subject = no
copy_extensions = none
[ anything ]
commonName = supplied
organizationName = optional
[ req ]
distinguished_name = dn
prompt = no
[ dn ]
CN = unused
[ root_ext ]
basicConstraints = critical, CA:true
keyUsage = critical, keyCertSign, cRLSign
subjectKeyIdentifier = hash
[ signer_ext ]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature, nonRepudiation
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid
authorityInfoAccess = OCSP;URI:http://127.0.0.1:$PKI_OCSP_PORT/, caIssuers;URI:http://127.0.0.1:$PKI_HTTP_PORT/root.der
crlDistributionPoints = URI:http://127.0.0.1:$PKI_HTTP_PORT/root.crl
[ tsa_ext ]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature, nonRepudiation
extendedKeyUsage = critical, timeStamping
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid
crlDistributionPoints = URI:http://127.0.0.1:$PKI_HTTP_PORT/root.crl
[ ocsp_ext ]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, OCSPSigning
noCheck = ignored
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid
[ tsa ]
default_tsa = tsa_config
[ tsa_config ]
dir = $PKI
serial = \$dir/tsaserial
crypto_device = builtin
signer_cert = \$dir/tsa.pem
certs = \$dir/root.pem
signer_key = \$dir/tsa.key
signer_digest = sha256
default_policy = 1.3.6.1.4.1.99999.1
digests = sha256, sha384, sha512
accuracy = secs:1
ordering = no
tsa_name = yes
ess_cert_id_chain = no
ess_cert_id_alg = sha256
EOF

    local q=(-config "$PKI/ca.cnf")
    openssl req -x509 -newkey rsa:2048 -nodes -days 30 -keyout "$PKI/root.key" -out "$PKI/root.pem" \
        -subj "/CN=rusticdoc test root CA/O=rusticdoc tests" "${q[@]}" -extensions root_ext 2>/dev/null || return 1
    openssl x509 -in "$PKI/root.pem" -outform DER -out "$PKI/root.der" || return 1
    issue() { # name, key algorithm options, extensions, subject
        local name=$1 ext=$3 subj=$4
        # shellcheck disable=SC2086
        openssl req -new -newkey $2 -nodes -keyout "$PKI/$name.key" -out "$PKI/$name.csr" \
            -subj "$subj" "${q[@]}" 2>/dev/null || return 1
        openssl ca -batch "${q[@]}" -extensions "$ext" -in "$PKI/$name.csr" -out "$PKI/$name.pem" \
            -notext 2>/dev/null || return 1
    }
    issue signer "rsa:2048" signer_ext "/CN=rusticdoc test signer/O=rusticdoc tests" || return 1
    issue ec-signer "ec -pkeyopt ec_paramgen_curve:P-256" signer_ext "/CN=rusticdoc test EC signer/O=rusticdoc tests" || return 1
    issue tsa "rsa:2048" tsa_ext "/CN=rusticdoc test TSA/O=rusticdoc tests" || return 1
    issue ocsp "ec -pkeyopt ec_paramgen_curve:P-256" ocsp_ext "/CN=rusticdoc test OCSP responder/O=rusticdoc tests" || return 1
    chmod 600 "$PKI"/*.key
    # The chain files a signer is configured with: leaf, then the root.
    cat "$PKI/signer.pem" "$PKI/root.pem" > "$PKI/signer-chain.pem"
    cat "$PKI/ec-signer.pem" "$PKI/root.pem" > "$PKI/ec-signer-chain.pem"
    openssl ca -gencrl "${q[@]}" -out "$PKI/root.crl.pem" 2>/dev/null || return 1
    openssl crl -in "$PKI/root.crl.pem" -outform DER -out "$PKI/root.crl" || return 1
    # An impostor: the root's name with another key, for forged OCSP answers
    # and CRLs that a validator must refuse.
    openssl req -x509 -newkey rsa:2048 -nodes -days 30 -keyout "$PKI/rogue.key" -out "$PKI/rogue.pem" \
        -subj "/CN=rusticdoc test root CA/O=rusticdoc tests" "${q[@]}" -extensions root_ext 2>/dev/null || return 1
    openssl ca -gencrl "${q[@]}" -keyfile "$PKI/rogue.key" -cert "$PKI/rogue.pem" \
        -out "$PKI/rogue.crl.pem" 2>/dev/null || return 1
    openssl crl -in "$PKI/rogue.crl.pem" -outform DER -out "$PKI/rogue.crl" || return 1
}

# The timestamp authority and CRL server, and the OCSP responder.
pki_start() {
    cat > "$PKI/serve.py" <<'EOF'
import http.server, subprocess, sys
pki, port = sys.argv[1], int(sys.argv[2])
class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        with open(pki + "/http.log", "a") as log:
            log.write("%s %s\n" % (self.command, self.path))
    def send(self, body, kind):
        self.send_response(200)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def do_GET(self):
        files = {"/root.crl": ("root.crl", "application/pkix-crl"),
                 "/root.der": ("root.der", "application/pkix-cert")}
        if self.path not in files:
            self.send_error(404); return
        name, kind = files[self.path]
        self.send(open(pki + "/" + name, "rb").read(), kind)
    def do_POST(self):
        query = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        if self.path != "/tsa" or self.headers.get("Content-Type") != "application/timestamp-query":
            self.send_error(400); return
        with open(pki + "/query.tsq", "wb") as f:
            f.write(query)
        reply = subprocess.run(
            ["openssl", "ts", "-reply", "-config", pki + "/ca.cnf", "-queryfile", pki + "/query.tsq",
             "-signer", pki + "/tsa.pem", "-inkey", pki + "/tsa.key", "-chain", pki + "/root.pem"],
            capture_output=True)
        if reply.returncode != 0:
            sys.stderr.write(reply.stderr.decode()); self.send_error(500); return
        self.send(reply.stdout, "application/timestamp-reply")
http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
EOF
    python3 -I "$PKI/serve.py" "$PKI" "$PKI_HTTP_PORT" 2>"$PKI/http.err" &
    PKI_HTTP_PID=$!
    disown "$PKI_HTTP_PID"
    openssl ocsp -port "$PKI_OCSP_PORT" -index "$PKI/index.txt" -CA "$PKI/root.pem" \
        -rsigner "$PKI/ocsp.pem" -rkey "$PKI/ocsp.key" -nmin 60 >"$PKI/ocsp.log" 2>&1 &
    PKI_OCSP_PID=$!
    disown "$PKI_OCSP_PID"
    export PKI_HTTP_PID PKI_OCSP_PID
    # Both up before anything asks them.
    for _ in $(seq 50); do
        # Not a bare connection to the responder: `openssl ocsp -port` takes a
        # connection that sends no request badly and stops answering.
        if curl -sf "http://127.0.0.1:$PKI_HTTP_PORT/root.der" -o /dev/null \
            && grep -q "waiting for OCSP client connections" "$PKI/ocsp.log"; then
            return 0
        fi
        sleep 0.1
    done
    echo "the test TSA or OCSP responder did not start"
    return 1
}

pki_stop() {
    # Safe under `set -e`: nothing here may fail the caller.
    if [ -n "${PKI_HTTP_PID:-}" ]; then kill "$PKI_HTTP_PID" 2>/dev/null || true; fi
    if [ -n "${PKI_OCSP_PID:-}" ]; then kill "$PKI_OCSP_PID" 2>/dev/null || true; fi
    # python3 can be a wrapper that leaves the server running under it.
    if [ -n "${PKI:-}" ]; then pkill -f "$PKI/serve.py" 2>/dev/null || true; fi
    return 0
}

