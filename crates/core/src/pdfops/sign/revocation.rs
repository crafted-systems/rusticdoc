//! Validation data for PAdES B-LT: every certificate a validator needs, and an
//! OCSP response or CRL for each one that is not a trust anchor.
//!
//! The certificates to cover are the signer's chain and the timestamp
//! authority's. Each one's issuer is found among them or fetched from its AIA
//! `caIssuers` URL; then its revocation status is asked of the OCSP responders
//! its AIA names, and failing that fetched from its CRL distribution points.
//! An OCSP responder's own certificate is covered in turn unless it carries
//! `id-pkix-ocsp-nocheck`. Nothing is taken on trust: an OCSP response must
//! answer for the exact certificate identifier asked about, be signed by the
//! issuer or by a responder the issuer authorised for OCSP, and be current; a
//! CRL must be signed with the issuer's key and be current. One that fails is
//! set aside for the next source, OCSP then CRLs. Every request goes through the [`Outbound`] and its
//! rules, with a time and size limit, and the whole collection keeps to a
//! deadline and a number of requests. A certificate found revoked, or one with
//! no reachable status, is an error: the file would not be B-LT.

use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use der::asn1::{Null, ObjectIdentifier, OctetString};
use der::{Decode, DecodePem, Encode};
use x509_cert::Certificate;
use x509_cert::crl::CertificateList;
use x509_cert::ext::pkix::name::{DistributionPointName, GeneralName};
use x509_cert::ext::pkix::{AuthorityInfoAccessSyntax, CrlDistributionPoints};
use x509_cert::spki::AlgorithmIdentifierOwned;
use x509_ocsp::{
    BasicOcspResponse, CertId, CertStatus, OcspRequest, OcspResponse, OcspResponseStatus, Request,
    ResponderId, TbsRequest, Version,
};

use super::keys::{cert_der, timestamp};
use super::net::{Outbound, OutboundRequest, upstream};
use super::verify::{self, CLOCK_SKEW, sha1};
use crate::error::{Error, Result};

const ID_AD_OCSP: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.48.1");
const ID_AD_CA_ISSUERS: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.48.2");
const ID_PKIX_OCSP_BASIC: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.48.1.1");
const ID_PKIX_OCSP_NOCHECK: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.48.1.5");
const AUTHORITY_INFO_ACCESS: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.1.1");
const CRL_DISTRIBUTION_POINTS: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.29.31");
const ID_KP_OCSP_SIGNING: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.3.9");
const ID_SHA1: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.14.3.2.26");

/// Most certificates one collection covers.
const MAX_CERTIFICATES: usize = 32;

/// How long and how much the requests for validation data may take.
#[derive(Debug, Clone, PartialEq)]
pub struct Limits {
    /// Per request.
    pub timeout: Duration,
    /// Per response: a CRL can be large.
    pub max_bytes: usize,
    /// Requests in one collection.
    pub max_requests: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            max_bytes: 10 * 1024 * 1024,
            max_requests: 32,
        }
    }
}

/// What goes in the DSS, as DER.
#[derive(Debug, Default, Clone)]
pub(super) struct Validation {
    pub certs: Vec<Vec<u8>>,
    pub ocsps: Vec<Vec<u8>>,
    pub crls: Vec<Vec<u8>>,
}

fn subject(cert: &Certificate) -> String {
    cert.tbs_certificate.subject.to_string()
}

fn extension<T: for<'a> Decode<'a>>(cert: &Certificate, oid: ObjectIdentifier) -> Option<T> {
    cert.tbs_certificate
        .extensions
        .iter()
        .flatten()
        .find(|e| e.extn_id == oid)
        .and_then(|e| T::from_der(e.extn_value.as_bytes()).ok())
}

fn has_extension(cert: &Certificate, oid: ObjectIdentifier) -> bool {
    cert.tbs_certificate
        .extensions
        .iter()
        .flatten()
        .any(|e| e.extn_id == oid)
}

/// The http(s) URLs of one AIA access method.
fn aia_urls(cert: &Certificate, method: ObjectIdentifier) -> Vec<String> {
    extension::<AuthorityInfoAccessSyntax>(cert, AUTHORITY_INFO_ACCESS)
        .map(|aia| {
            aia.0
                .into_iter()
                .filter(|d| d.access_method == method)
                .filter_map(|d| match d.access_location {
                    GeneralName::UniformResourceIdentifier(uri) => Some(uri.to_string()),
                    _ => None,
                })
                .filter(|u| u.starts_with("http://") || u.starts_with("https://"))
                .collect()
        })
        .unwrap_or_default()
}

/// The http(s) URLs of the CRL distribution points.
fn crl_urls(cert: &Certificate) -> Vec<String> {
    extension::<CrlDistributionPoints>(cert, CRL_DISTRIBUTION_POINTS)
        .map(|dps| {
            dps.0
                .into_iter()
                .filter_map(|dp| match dp.distribution_point {
                    Some(DistributionPointName::FullName(names)) => Some(names),
                    _ => None,
                })
                .flatten()
                .filter_map(|n| match n {
                    GeneralName::UniformResourceIdentifier(uri) => Some(uri.to_string()),
                    _ => None,
                })
                .filter(|u| u.starts_with("http://") || u.starts_with("https://"))
                .collect()
        })
        .unwrap_or_default()
}

fn self_issued(cert: &Certificate) -> bool {
    cert.tbs_certificate.subject == cert.tbs_certificate.issuer
}

/// A certificate as DER, or as PEM.
fn parse_certificate(bytes: &[u8]) -> Option<Certificate> {
    Certificate::from_der(bytes)
        .ok()
        .or_else(|| Certificate::from_pem(bytes).ok())
}

/// The OCSP certificate identifier of `cert`, SHA-1 as responders expect.
fn cert_id(cert: &Certificate, issuer: &Certificate) -> Result<CertId> {
    let encoding = |e: der::Error| upstream("the OCSP request", e);
    let issuer_name = issuer.tbs_certificate.subject.to_der().map_err(encoding)?;
    let issuer_key = issuer
        .tbs_certificate
        .subject_public_key_info
        .subject_public_key
        .raw_bytes();
    Ok(CertId {
        hash_algorithm: AlgorithmIdentifierOwned {
            oid: ID_SHA1,
            parameters: Some(der::Any::encode_from(&Null).map_err(encoding)?),
        },
        issuer_name_hash: OctetString::new(sha1(&issuer_name)).map_err(encoding)?,
        issuer_key_hash: OctetString::new(sha1(issuer_key)).map_err(encoding)?,
        serial_number: cert.tbs_certificate.serial_number.clone(),
    })
}

/// A DER OCSP request for `cert`.
pub(super) fn ocsp_request(cert: &Certificate, issuer: &Certificate) -> Result<Vec<u8>> {
    OcspRequest {
        tbs_request: TbsRequest {
            version: Version::V1,
            requestor_name: None,
            request_list: vec![Request {
                req_cert: cert_id(cert, issuer)?,
                single_request_extensions: None,
            }],
            request_extensions: None,
        },
        optional_signature: None,
    }
    .to_der()
    .map_err(|e| upstream("the OCSP request", e))
}

/// Whether `this_update <= now < next_update`, with [`CLOCK_SKEW`] either side.
/// Without a next update, the information has to be from the last hour.
fn current(
    this_update: SystemTime,
    next_update: Option<SystemTime>,
    now: SystemTime,
) -> std::result::Result<(), String> {
    if this_update > now + CLOCK_SKEW {
        return Err(format!(
            "it is dated {}, in the future",
            timestamp(this_update)
        ));
    }
    match next_update {
        Some(next) if next + CLOCK_SKEW <= now => Err(format!("it expired at {}", timestamp(next))),
        Some(_) => Ok(()),
        None if this_update + Duration::from_secs(3600) + CLOCK_SKEW < now => Err(format!(
            "it has no next update and is from {}, over an hour ago",
            timestamp(this_update)
        )),
        None => Ok(()),
    }
}

/// The certificate that signed a basic OCSP response, if it may: the issuer
/// itself, or a certificate the issuer signed for OCSP (`id-kp-OCSPSigning`)
/// that is valid now, found among the response's certificates.
fn ocsp_signer<'a>(
    basic: &'a BasicOcspResponse,
    issuer: &'a Certificate,
    now: SystemTime,
) -> std::result::Result<&'a Certificate, String> {
    let names = |c: &Certificate| match &basic.tbs_response_data.responder_id {
        ResponderId::ByName(name) => c.tbs_certificate.subject == *name,
        ResponderId::ByKey(hash) => {
            sha1(
                c.tbs_certificate
                    .subject_public_key_info
                    .subject_public_key
                    .raw_bytes(),
            ) == hash.as_bytes()
        }
    };
    let issuer_key = &issuer.tbs_certificate.subject_public_key_info;
    if names(issuer) {
        return Ok(issuer);
    }
    let delegate = basic
        .certs
        .iter()
        .flatten()
        .find(|c| names(c))
        .ok_or("the responder is neither the issuer nor a certificate the response carries")?;
    if delegate.tbs_certificate.subject_public_key_info == *issuer_key {
        return Ok(issuer);
    }
    if !verify::issued_by(delegate, issuer) {
        return Err(format!(
            "the responder's certificate ({}) was not issued by the certificate's issuer",
            delegate.tbs_certificate.subject
        ));
    }
    if !verify::extended_key_usage(delegate).is_some_and(|(_, u)| u.contains(&ID_KP_OCSP_SIGNING)) {
        return Err(format!(
            "the responder's certificate ({}) is not for OCSP signing",
            delegate.tbs_certificate.subject
        ));
    }
    if !verify::valid_at(delegate, now) {
        return Err(format!(
            "the responder's certificate ({}) is not valid now",
            delegate.tbs_certificate.subject
        ));
    }
    Ok(delegate)
}

/// What an OCSP response says of `cert`: good, with the delegated responder's
/// certificate when one signed it, or why it cannot be used. It must answer for exactly the
/// certificate identifier asked about, be signed by the issuer or a
/// responder the issuer authorised, and be current. `REVOKED` comes only
/// from a response that passed all of that.
pub(super) fn read_ocsp(
    response: &[u8],
    cert: &Certificate,
    issuer: &Certificate,
    now: SystemTime,
) -> std::result::Result<Option<Certificate>, String> {
    let response = OcspResponse::from_der(response)
        .map_err(|e| format!("the answer is not an OCSP response: {e}"))?;
    if response.response_status != OcspResponseStatus::Successful {
        return Err(format!(
            "the responder answered {:?}",
            response.response_status
        ));
    }
    let bytes = response
        .response_bytes
        .ok_or("the responder sent no response")?;
    if bytes.response_type != ID_PKIX_OCSP_BASIC {
        return Err("the response is not a basic OCSP response".into());
    }
    let basic = BasicOcspResponse::from_der(bytes.response.as_bytes())
        .map_err(|e| format!("the basic OCSP response cannot be parsed: {e}"))?;
    let wanted = cert_id(cert, issuer).map_err(|e| e.to_string())?;
    let single = basic
        .tbs_response_data
        .responses
        .iter()
        .find(|r| {
            r.cert_id.hash_algorithm.oid == wanted.hash_algorithm.oid
                && r.cert_id.issuer_name_hash == wanted.issuer_name_hash
                && r.cert_id.issuer_key_hash == wanted.issuer_key_hash
                && r.cert_id.serial_number == wanted.serial_number
        })
        .ok_or("the response is about another certificate")?;

    let signer = ocsp_signer(&basic, issuer, now)?;
    let tbs = basic
        .tbs_response_data
        .to_der()
        .map_err(|e| format!("the response cannot be encoded: {e}"))?;
    let oid = &basic.signature_algorithm.oid;
    verify::signature(signer, oid, oid, &tbs, basic.signature.raw_bytes())
        .map_err(|e| format!("the response's signature {e}"))?;

    let next = single
        .next_update
        .as_ref()
        .map(|t| UNIX_EPOCH + t.0.to_unix_duration());
    current(
        UNIX_EPOCH + single.this_update.0.to_unix_duration(),
        next,
        now,
    )
    .map_err(|e| format!("the response is not current: {e}"))?;

    match &single.cert_status {
        // Only the verified signer leaves here: other certificates the
        // response carries prove nothing and never reach the pool.
        CertStatus::Good(_) => Ok((signer != issuer).then(|| signer.clone())),
        CertStatus::Revoked(info) => Err(format!(
            "REVOKED since {}",
            timestamp(UNIX_EPOCH + info.revocation_time.0.to_unix_duration())
        )),
        CertStatus::Unknown(_) => Err("the responder does not know the certificate".into()),
    }
}

/// What a CRL says of `cert`: its DER when `cert` is not listed. It must be
/// from `cert`'s issuer, signed with the issuer's key, and current.
fn read_crl(
    crl: &[u8],
    cert: &Certificate,
    issuer: Option<&Certificate>,
    now: SystemTime,
) -> std::result::Result<Vec<u8>, String> {
    let list = CertificateList::from_der(crl)
        .or_else(|_| {
            // Some distribution points serve PEM.
            let text = std::str::from_utf8(crl).map_err(|_| der::Tag::Sequence.value_error())?;
            let (_, der) = der::pem::decode_vec(text.as_bytes())?;
            CertificateList::from_der(&der)
        })
        .map_err(|e| format!("the answer is not a CRL: {e}"))?;
    if list.tbs_cert_list.issuer != cert.tbs_certificate.issuer {
        return Err("the CRL is from another issuer".into());
    }
    let issuer = issuer.ok_or(
        "the issuer's certificate is not in the chain and cannot be fetched, so the CRL's signature cannot be checked",
    )?;
    let tbs = list
        .tbs_cert_list
        .to_der()
        .map_err(|e| format!("the CRL cannot be encoded: {e}"))?;
    let signature = list
        .signature
        .as_bytes()
        .ok_or("the CRL's signature is not whole bytes")?;
    let oid = &list.signature_algorithm.oid;
    verify::signature(issuer, oid, oid, &tbs, signature)
        .map_err(|e| format!("the CRL's signature {e}"))?;
    current(
        UNIX_EPOCH + list.tbs_cert_list.this_update.to_unix_duration(),
        list.tbs_cert_list
            .next_update
            .map(|t| UNIX_EPOCH + t.to_unix_duration()),
        now,
    )
    .map_err(|e| format!("the CRL is not current: {e}"))?;
    if let Some(revoked) = list
        .tbs_cert_list
        .revoked_certificates
        .iter()
        .flatten()
        .find(|r| r.serial_number == cert.tbs_certificate.serial_number)
    {
        return Err(format!(
            "REVOKED since {}",
            timestamp(UNIX_EPOCH + revoked.revocation_date.to_unix_duration())
        ));
    }
    list.to_der().map_err(|e| e.to_string())
}

/// The requests of one collection, under its limits.
struct Fetcher<'a> {
    outbound: &'a dyn Outbound,
    limits: &'a Limits,
    deadline: Instant,
    made: usize,
}

impl Fetcher<'_> {
    async fn fetch(
        &mut self,
        url: &str,
        body: Option<(&'static str, Vec<u8>)>,
        purpose: &'static str,
    ) -> Result<Vec<u8>> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(Error::Status(
                503,
                "the time for fetching validation data ran out".into(),
            ));
        }
        if self.made >= self.limits.max_requests {
            return Err(Error::Status(
                503,
                format!(
                    "validation data needs more than {} requests",
                    self.limits.max_requests
                ),
            ));
        }
        self.made += 1;
        let (content_type, body) = match body {
            Some((t, b)) => (Some(t), Some(b)),
            None => (None, None),
        };
        self.outbound
            .fetch(OutboundRequest {
                url: url.to_string(),
                content_type,
                body,
                headers: Vec::new(),
                basic_auth: None,
                max_bytes: self.limits.max_bytes,
                timeout: self.limits.timeout.min(left),
                purpose,
            })
            .await
    }
}

/// The certificates found so far, and those still to cover.
#[derive(Default)]
struct Found {
    pool: Vec<Certificate>,
    seen: HashSet<Vec<u8>>,
    queue: VecDeque<Certificate>,
    out: Validation,
}

impl Found {
    /// Keeps `cert` for the DSS, and queues it for a revocation status when
    /// `cover` is set.
    fn add(&mut self, cert: &Certificate, cover: bool) -> Result<()> {
        let der = cert_der(cert)?;
        if !self.seen.insert(der.clone()) {
            return Ok(());
        }
        if self.pool.len() >= MAX_CERTIFICATES {
            return Err(Error::Status(
                503,
                format!("validation data needs more than {MAX_CERTIFICATES} certificates"),
            ));
        }
        self.pool.push(cert.clone());
        if cover {
            self.queue.push_back(cert.clone());
        }
        self.out.certs.push(der);
        Ok(())
    }
}

/// Collects validation data for `start` and whatever its validation needs.
pub(super) async fn collect(
    start: &[Certificate],
    outbound: &dyn Outbound,
    limits: &Limits,
    deadline: Instant,
    now: SystemTime,
) -> Result<Validation> {
    let mut fetcher = Fetcher {
        outbound,
        limits,
        deadline,
        made: 0,
    };
    let mut found = Found::default();
    for cert in start {
        found.add(cert, true)?;
    }

    while let Some(cert) = found.queue.pop_front() {
        if self_issued(&cert) {
            // A trust anchor: validators take it from their own store.
            continue;
        }
        let name = subject(&cert);
        // The issuer is the certificate whose key verifies this one's
        // signature, not merely one with the right name: an impostor with the
        // issuer's name must not vouch for revocation data.
        let mut issuer = found
            .pool
            .iter()
            .find(|c| verify::issued_by(&cert, c))
            .cloned();
        if issuer.is_none() {
            for url in aia_urls(&cert, ID_AD_CA_ISSUERS) {
                if let Ok(bytes) = fetcher
                    .fetch(&url, None, "the certificate issuer download")
                    .await
                    && let Some(fetched) = parse_certificate(&bytes)
                    && verify::issued_by(&cert, &fetched)
                {
                    found.add(&fetched, true)?;
                    issuer = Some(fetched);
                    break;
                }
            }
        }

        let mut problems: Vec<String> = Vec::new();
        let mut covered = false;
        if let Some(issuer) = &issuer {
            for url in aia_urls(&cert, ID_AD_OCSP) {
                let request = ocsp_request(&cert, issuer)?;
                let response = match fetcher
                    .fetch(
                        &url,
                        Some(("application/ocsp-request", request)),
                        "the OCSP responder",
                    )
                    .await
                {
                    Ok(r) => r,
                    Err(e @ Error::Status(503, _)) => return Err(e),
                    Err(e) => {
                        problems.push(format!("{url}: {e}"));
                        continue;
                    }
                };
                match read_ocsp(&response, &cert, issuer, now) {
                    Ok(delegate) => {
                        found.out.ocsps.push(response);
                        // A delegated responder, checked as issued by the
                        // verified issuer: covered in turn unless it says
                        // it needs no checking.
                        if let Some(responder) = delegate {
                            let needs_status = !has_extension(&responder, ID_PKIX_OCSP_NOCHECK);
                            found.add(&responder, needs_status)?;
                        }
                        covered = true;
                        break;
                    }
                    Err(why) if why.starts_with("REVOKED") => {
                        return Err(Error::Status(
                            503,
                            format!("the certificate {name} is {why}, says its OCSP responder"),
                        ));
                    }
                    Err(why) => problems.push(format!("{url}: {why}")),
                }
            }
        } else {
            problems.push("its issuer's certificate is not in the chain and cannot be fetched, so OCSP cannot be asked".into());
        }
        if !covered {
            for url in crl_urls(&cert) {
                let crl = match fetcher.fetch(&url, None, "the CRL download").await {
                    Ok(c) => c,
                    Err(e @ Error::Status(503, _)) => return Err(e),
                    Err(e) => {
                        problems.push(format!("{url}: {e}"));
                        continue;
                    }
                };
                match read_crl(&crl, &cert, issuer.as_ref(), now) {
                    Ok(der) => {
                        if !found.out.crls.contains(&der) {
                            found.out.crls.push(der);
                        }
                        covered = true;
                        break;
                    }
                    Err(why) if why.starts_with("REVOKED") => {
                        return Err(Error::Status(
                            503,
                            format!("the certificate {name} is {why}, says its CRL"),
                        ));
                    }
                    Err(why) => problems.push(format!("{url}: {why}")),
                }
            }
        }
        if !covered {
            return Err(upstream(
                "collecting validation data",
                if problems.is_empty() {
                    format!(
                        "the certificate {name} names no OCSP responder and no CRL distribution point"
                    )
                } else {
                    format!(
                        "no revocation status for the certificate {name} ({})",
                        problems.join("; ")
                    )
                },
            ));
        }
    }
    Ok(found.out)
}
