//! RFC 3161 timestamps: the signature timestamp that makes a signature
//! PAdES B-T, and the document timestamp of B-LTA.
//!
//! A request carries the SHA-256 of what is stamped and a fresh nonce, and
//! asks for the authority's certificate. The answer is checked before it is
//! used: granted, a `SignedData` over a `TSTInfo` whose message imprint and
//! nonce are the ones sent, and a signature that verifies with the
//! certificate the token names. Anything else is an error, never a file
//! without its timestamp.

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
    }
}

/// A checked timestamp token.
#[derive(Debug, Clone)]
pub struct Token {
    /// The DER `ContentInfo`.
    pub der: Vec<u8>,
    /// The certificates the token carries: the authority's, and any chain.
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
            largest: AtomicUsize::new(0),
        }
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
        let token = check(&response, digest, &nonce)?;
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

/// Checks a `TimeStampResp` against what was asked for.
pub(super) fn check(response: &[u8], digest: &[u8; 32], nonce: &[u8]) -> Result<Token> {
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
    Ok(Token {
        der,
        certificates: parsed.certificates,
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
        certificates,
    })
}
