//! RFC 3161 timestamps: the signature timestamp that makes a signature
//! PAdES B-T, and the document timestamp of B-LTA.
//!
//! A request carries the SHA-256 of what is stamped and a fresh nonce, and
//! asks for the authority's certificate. The answer is checked before it is
//! used: granted, a `SignedData` over a `TSTInfo` whose message imprint and
//! nonce are the ones sent, a signature that verifies with the certificate
//! the token names, and that certificate trusted: it is one of the operator's
//! `RUSTICDOC_SIGNING_TSA_CERT` certificates or chains to one, carries a
//! critical `id-kp-timeStamping` extended key usage, and is valid at the
//! token's time. Anything else is an error, never a file without its
//! timestamp.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use cms::cert::CertificateChoices;
use cms::content_info::ContentInfo;
use cms::signed_data::{SignedData, SignerIdentifier};
use der::asn1::{Int, ObjectIdentifier, OctetString};
use der::{Decode, Encode};
use x509_cert::Certificate;
use x509_cert::spki::AlgorithmIdentifier;
use x509_tsp::{MessageImprint, TimeStampReq, TimeStampResp, TspVersion, TstInfo};

use super::cms::{ID_MESSAGE_DIGEST, ID_SHA256, ID_SIGNED_DATA};
use super::net::{Outbound, OutboundRequest, upstream};
use super::verify;
use crate::error::{Error, Result};

const ID_CT_TST_INFO: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.1.4");
const ID_KP_TIME_STAMPING: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.3.8");

/// Longest chain from the authority's certificate to an anchor.
const MAX_PATH: usize = 8;

/// Largest timestamp response read. Real ones are a few kilobytes.
const MAX_RESPONSE_BYTES: usize = 256 * 1024;

/// What a token is expected to need until one has been seen.
const DEFAULT_TOKEN_RESERVE: usize = 8 * 1024;

/// A timestamp authority, from `RUSTICDOC_SIGNING_TSA_*`.
pub struct Tsa {
    pub url: String,
    /// Basic credentials.
    pub username: Option<String>,
    pub password: Option<String>,
    /// Further headers, such as an API key.
    pub headers: Vec<(String, String)>,
    pub timeout: Duration,
    /// `RUSTICDOC_SIGNING_TSA_CERT`: a PEM file of the certificates the
    /// authority's must be, or chain to. Read by [`Tsa::load_anchors`].
    pub cert_file: Option<std::path::PathBuf>,
    /// The trust anchors or pinned certificates. Without any, no token is
    /// accepted.
    anchors: Vec<Certificate>,
    /// The largest token seen, to size the next placeholder.
    largest: AtomicUsize,
}

impl std::fmt::Debug for Tsa {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tsa")
            .field("url", &self.url)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl Clone for Tsa {
    fn clone(&self) -> Self {
        Self {
            url: self.url.clone(),
            username: self.username.clone(),
            password: self.password.clone(),
            headers: self.headers.clone(),
            timeout: self.timeout,
            cert_file: self.cert_file.clone(),
            anchors: self.anchors.clone(),
            largest: AtomicUsize::new(self.largest.load(Ordering::Relaxed)),
        }
    }
}

impl PartialEq for Tsa {
    fn eq(&self, other: &Self) -> bool {
        self.url == other.url
            && self.username == other.username
            && self.password == other.password
            && self.headers == other.headers
            && self.timeout == other.timeout
            && self.cert_file == other.cert_file
            && self.anchors == other.anchors
    }
}

/// A checked timestamp token.
#[derive(Debug, Clone)]
pub struct Token {
    /// The DER `ContentInfo`.
    pub der: Vec<u8>,
    /// The authority's certificate and the chain from it to a trusted one,
    /// each checked. Whatever else the token carries is left out.
    pub certificates: Vec<Certificate>,
    /// When the authority says it stamped.
    pub time: SystemTime,
}

impl Tsa {
    pub fn new(url: impl Into<String>, timeout: Duration) -> Self {
        Self {
            url: url.into(),
            username: None,
            password: None,
            headers: Vec::new(),
            timeout,
            cert_file: None,
            anchors: Vec::new(),
            largest: AtomicUsize::new(0),
        }
    }

    /// Trusts the certificates in `pem`: the authority's own, pinned, or the
    /// CAs it must chain to.
    pub fn with_anchors_pem(mut self, pem: &[u8]) -> std::result::Result<Self, String> {
        let anchors = super::keys::parse_chain(pem)?;
        if anchors.is_empty() {
            return Err("it holds no certificate".into());
        }
        self.anchors = anchors;
        Ok(self)
    }

    /// Reads [`Tsa::cert_file`] into the anchors. Errors name the variable.
    /// Anchors given in code with [`Tsa::with_anchors_pem`] and no file stay.
    pub fn load_anchors(&mut self) -> std::result::Result<(), String> {
        if self.cert_file.is_none() && self.has_anchors() {
            return Ok(());
        }
        let path = self.cert_file.clone().ok_or(
            "RUSTICDOC_SIGNING_TSA_CERT is needed with RUSTICDOC_SIGNING_TSA_URL: the certificate the timestamp authority signs with, or the CA it chains to",
        )?;
        let pem = std::fs::read(&path).map_err(|e| {
            format!(
                "RUSTICDOC_SIGNING_TSA_CERT: cannot read {}: {e}",
                path.display()
            )
        })?;
        *self = std::mem::replace(self, Tsa::new("", Duration::ZERO))
            .with_anchors_pem(&pem)
            .map_err(|e| format!("RUSTICDOC_SIGNING_TSA_CERT ({}): {e}", path.display()))?;
        Ok(())
    }

    /// True when some certificate is trusted for tokens.
    pub fn has_anchors(&self) -> bool {
        !self.anchors.is_empty()
    }

    /// The certificates tokens are trusted through.
    pub(super) fn anchors(&self) -> &[Certificate] {
        &self.anchors
    }

    /// Bytes to reserve for a token in a placeholder.
    pub(super) fn reserve(&self) -> usize {
        self.largest
            .load(Ordering::Relaxed)
            .max(DEFAULT_TOKEN_RESERVE)
    }

    /// Asks for a timestamp over `digest`, a SHA-256, within `budget`.
    pub(super) async fn stamp(
        &self,
        outbound: &dyn Outbound,
        digest: &[u8; 32],
        budget: Duration,
    ) -> Result<Token> {
        let mut nonce = [0u8; 8];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| upstream("the timestamp authority", "no random nonce"))?;
        // Positive, and with no leading zero byte to drop.
        nonce[0] = (nonce[0] & 0x7f) | 0x01;
        let request = request(digest, &nonce)?;
        let headers = self.headers.clone();
        let basic_auth = self
            .username
            .clone()
            .map(|user| (user, self.password.clone().unwrap_or_default()));
        let response = outbound
            .fetch(OutboundRequest {
                url: self.url.clone(),
                content_type: Some("application/timestamp-query"),
                body: Some(request),
                headers,
                basic_auth,
                max_bytes: MAX_RESPONSE_BYTES,
                timeout: self.timeout.min(budget),
                purpose: "the timestamp authority",
            })
            .await?;
        let token = check(&response, digest, &nonce, &self.anchors, SystemTime::now())?;
        self.largest.fetch_max(token.der.len(), Ordering::Relaxed);
        Ok(token)
    }
}

/// A DER `TimeStampReq` for `digest`.
pub(super) fn request(digest: &[u8; 32], nonce: &[u8]) -> Result<Vec<u8>> {
    let encoding = |e: der::Error| upstream("the timestamp request", e);
    TimeStampReq {
        version: TspVersion::V1,
        message_imprint: MessageImprint {
            hash_algorithm: AlgorithmIdentifier {
                oid: ID_SHA256,
                parameters: None,
            },
            hashed_message: OctetString::new(digest.to_vec()).map_err(encoding)?,
        },
        req_policy: None,
        nonce: Some(Int::new(nonce).map_err(encoding)?),
        cert_req: true,
        extensions: None,
    }
    .to_der()
    .map_err(encoding)
}

fn refused(what: impl std::fmt::Display) -> Error {
    upstream("the timestamp authority", what)
}

/// Checks a `TimeStampResp` against what was asked for, and its signer
/// against `anchors`.
pub(super) fn check(
    response: &[u8],
    digest: &[u8; 32],
    nonce: &[u8],
    anchors: &[Certificate],
    now: SystemTime,
) -> Result<Token> {
    let response = TimeStampResp::from_der(response)
        .map_err(|e| refused(format!("its answer is not a timestamp response: {e}")))?;
    use cmpv2::status::PkiStatus;
    if !matches!(
        response.status.status,
        PkiStatus::Accepted | PkiStatus::GrantedWithMods
    ) {
        let text = response
            .status
            .status_string
            .iter()
            .flatten()
            .map(|s| s.as_str().to_string())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(refused(format!(
            "it refused the request ({:?}{}{})",
            response.status.status,
            if text.is_empty() { "" } else { ": " },
            text
        )));
    }
    let token = response
        .time_stamp_token
        .ok_or_else(|| refused("it granted the request but sent no token"))?;
    let der = token
        .to_der()
        .map_err(|e| refused(format!("its token cannot be encoded: {e}")))?;
    let parsed = parse(&der)?;
    if parsed.imprint_algorithm != ID_SHA256 || parsed.imprint != digest {
        return Err(refused(
            "its token stamps something other than what was sent",
        ));
    }
    if parsed.nonce.as_deref() != Some(nonce) {
        return Err(refused("its token does not carry the nonce that was sent"));
    }
    // The token was asked for just now, so its time must be now: a token
    // dated elsewhere is not the one asked for, or the authority's clock is
    // wrong, and either way its certificate cannot be judged at that time.
    let skew = match parsed.time.duration_since(now) {
        Ok(ahead) => ahead,
        Err(behind) => behind.duration(),
    };
    if skew > verify::CLOCK_SKEW {
        return Err(refused(format!(
            "its token is dated {}, more than {} minutes from this service's clock",
            super::keys::timestamp(parsed.time),
            verify::CLOCK_SKEW.as_secs() / 60
        )));
    }
    let path = signer_path(
        &parsed.signer,
        &parsed.certificates,
        anchors,
        parsed.time,
        now,
    )
    .map_err(|e| refused(format!("its token is not trusted: {e}")))?;
    Ok(Token {
        der,
        // Not everything the token carries: that set is not signed, and these
        // are asked about, at the addresses they name, to prove the signature
        // later.
        certificates: path,
        time: parsed.time,
    })
}

/// What a token says, once its signature has been checked.
pub(super) struct Parsed {
    pub imprint_algorithm: ObjectIdentifier,
    pub imprint: Vec<u8>,
    pub nonce: Option<Vec<u8>>,
    pub time: SystemTime,
    pub certificates: Vec<Certificate>,
    /// The certificate whose key signed the token.
    pub signer: Certificate,
}

/// Parses a DER `TimeStampToken` and verifies its signature.
pub(super) fn parse(der: &[u8]) -> Result<Parsed> {
    let info = ContentInfo::from_der(der)
        .map_err(|e| refused(format!("its token is not a CMS ContentInfo: {e}")))?;
    if info.content_type != ID_SIGNED_DATA {
        return Err(refused("its token is not SignedData"));
    }
    let signed: SignedData = info
        .content
        .decode_as()
        .map_err(|e| refused(format!("its token is not SignedData: {e}")))?;
    if signed.encap_content_info.econtent_type != ID_CT_TST_INFO {
        return Err(refused("its token does not hold a TSTInfo"));
    }
    let econtent = signed
        .encap_content_info
        .econtent
        .as_ref()
        .ok_or_else(|| refused("its token holds no TSTInfo"))?;
    let tst_der = OctetString::from_der(
        &econtent
            .to_der()
            .map_err(|e| refused(format!("its TSTInfo cannot be read: {e}")))?,
    )
    .map_err(|e| refused(format!("its TSTInfo is not an OCTET STRING: {e}")))?;
    let tst = TstInfo::from_der(tst_der.as_bytes())
        .map_err(|e| refused(format!("its TSTInfo cannot be parsed: {e}")))?;
    let certificates: Vec<Certificate> = signed
        .certificates
        .iter()
        .flat_map(|set| set.0.iter())
        .filter_map(|c| match c {
            CertificateChoices::Certificate(c) => Some(c.clone()),
            _ => None,
        })
        .collect();

    // The signer, its message digest over the TSTInfo, and its signature.
    let signer_info = signed
        .signer_infos
        .0
        .as_slice()
        .first()
        .ok_or_else(|| refused("its token has no signer"))?;
    let cert = certificates
        .iter()
        .find(|c| match &signer_info.sid {
            SignerIdentifier::IssuerAndSerialNumber(id) => {
                c.tbs_certificate.issuer == id.issuer
                    && c.tbs_certificate.serial_number == id.serial_number
            }
            SignerIdentifier::SubjectKeyIdentifier(ski) => {
                verify::subject_key_id(c).as_deref() == Some(ski.0.as_bytes())
            }
        })
        .ok_or_else(|| {
            refused("its token does not include the authority's certificate, which was asked for")
        })?;
    let attrs = signer_info
        .signed_attrs
        .as_ref()
        .ok_or_else(|| refused("its token has no signed attributes"))?;
    let message_digest: OctetString = attrs
        .iter()
        .find(|a| a.oid == ID_MESSAGE_DIGEST)
        .and_then(|a| a.values.get(0))
        .and_then(|v| v.decode_as().ok())
        .ok_or_else(|| refused("its token has no message digest"))?;
    let computed = verify::digest(&signer_info.digest_alg.oid, tst_der.as_bytes())
        .ok_or_else(|| refused("its token uses a digest this service does not know"))?;
    if computed != message_digest.as_bytes() {
        return Err(refused(
            "its token's message digest does not match its TSTInfo",
        ));
    }
    let signed_attrs = attrs
        .to_der()
        .map_err(|e| refused(format!("its signed attributes cannot be encoded: {e}")))?;
    verify::signature(
        cert,
        &signer_info.digest_alg.oid,
        &signer_info.signature_algorithm.oid,
        &signed_attrs,
        signer_info.signature.as_bytes(),
    )
    .map_err(|e| refused(format!("its token's signature {e}")))?;

    Ok(Parsed {
        imprint_algorithm: tst.message_imprint.hash_algorithm.oid,
        imprint: tst.message_imprint.hashed_message.as_bytes().to_vec(),
        nonce: tst.nonce.map(|n| n.as_bytes().to_vec()),
        time: UNIX_EPOCH + tst.gen_time.to_unix_duration(),
        signer: cert.clone(),
        certificates,
    })
}

/// Adds to `path` the root that issued its last certificate, when `pool` has
/// it. A pool is not signed, so only one kind of certificate is taken from
/// it: a self-signed one whose key made the signature on the trusted
/// certificate. Its own signature shows that the holder of that key wrote
/// all of it, which no certificate further from the trusted one can show.
fn with_root(
    mut path: Vec<Certificate>,
    pool: &[Certificate],
    valid: &dyn Fn(&Certificate) -> bool,
) -> Vec<Certificate> {
    let Some(last) = path.last() else { return path };
    if verify::issued_by(last, last) {
        return path;
    }
    if let Some(root) = pool.iter().find(|c| {
        verify::is_ca(c) && valid(c) && verify::issued_by(c, c) && verify::issued_by(last, c)
    }) {
        path.push(root.clone());
    }
    path
}

/// [`check_signer`], giving the certificates it trusted: the signer, the CA
/// certificates between, and the anchor. Where the anchor is not a root, the
/// root that issued it follows when `pool` has it; see [`with_root`]. Nothing
/// else a token carries has been checked, since a token's certificate set is
/// not signed.
pub(super) fn signer_path(
    signer: &Certificate,
    pool: &[Certificate],
    anchors: &[Certificate],
    at: SystemTime,
    now: SystemTime,
) -> std::result::Result<Vec<Certificate>, String> {
    let subject = signer.tbs_certificate.subject.to_string();
    if anchors.is_empty() {
        return Err("no certificate is trusted for timestamps (RUSTICDOC_SIGNING_TSA_CERT)".into());
    }
    match verify::extended_key_usage(signer) {
        Some((true, usages)) if usages.contains(&ID_KP_TIME_STAMPING) => {}
        Some((false, _)) => {
            return Err(format!(
                "the certificate {subject} does not mark its extended key usage critical"
            ));
        }
        _ => {
            return Err(format!(
                "the certificate {subject} is not for timestamping (no id-kp-timeStamping)"
            ));
        }
    }
    let valid = |c: &Certificate| verify::valid_at(c, at) && verify::valid_at(c, now);
    if !valid(signer) {
        return Err(format!(
            "the certificate {subject} is not valid now or was not at the token's time"
        ));
    }
    let mut path = vec![signer.clone()];
    if anchors.iter().any(|a| a == signer) {
        return Ok(with_root(path, pool, &valid));
    }
    let mut current = signer.clone();
    let mut expired_anchor = None;
    for _ in 0..MAX_PATH {
        if let Some(anchor) = anchors.iter().find(|a| verify::issued_by(&current, a)) {
            if valid(anchor) {
                path.push(anchor.clone());
                return Ok(with_root(path, pool, &valid));
            }
            expired_anchor = Some(anchor.tbs_certificate.subject.to_string());
            break;
        }
        let Some(next) = pool.iter().find(|c| {
            *c != &current && verify::is_ca(c) && valid(c) && verify::issued_by(&current, c)
        }) else {
            break;
        };
        path.push(next.clone());
        current = next.clone();
    }
    Err(match expired_anchor {
        Some(anchor) => format!(
            "the certificate {subject} chains to {anchor}, which is not valid now or was not at the token's time"
        ),
        None => format!(
            "the certificate {subject} neither is nor chains to a certificate in RUSTICDOC_SIGNING_TSA_CERT"
        ),
    })
}
