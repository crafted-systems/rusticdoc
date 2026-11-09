//! Validation data for PAdES B-LT: every certificate a validator needs, and an
//! OCSP response or CRL for each one that is not a trust anchor.
//!
//! The certificates to cover are the signer's chain and the timestamp
//! authority's. Each one's issuer is found among them or fetched from its AIA
//! `caIssuers` URL; then its revocation status is asked of the OCSP responders
//! its AIA names, and failing that fetched from its CRL distribution points.
//! An OCSP responder's own certificate is covered in turn unless it carries
//! `id-pkix-ocsp-nocheck`. Every request goes through the [`Outbound`] and its
//! rules, with a time and size limit, and the whole collection keeps to a
//! deadline and a number of requests. A certificate found revoked, or one with
//! no reachable status, is an error: the file would not be B-LT.

use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

use der::asn1::{Null, ObjectIdentifier, OctetString};
use der::{Decode, DecodePem, Encode};
use x509_cert::Certificate;
use x509_cert::crl::CertificateList;
use x509_cert::ext::pkix::name::{DistributionPointName, GeneralName};
use x509_cert::ext::pkix::{AuthorityInfoAccessSyntax, CrlDistributionPoints};
use x509_cert::spki::AlgorithmIdentifierOwned;
use x509_ocsp::{
    BasicOcspResponse, CertId, CertStatus, OcspRequest, OcspResponse, OcspResponseStatus, Request,
    TbsRequest, Version,
};

use super::keys::{cert_der, timestamp};
use super::net::{Outbound, OutboundRequest, upstream};
use super::verify::sha1;
use crate::error::{Error, Result};

const ID_AD_OCSP: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.48.1");
const ID_AD_CA_ISSUERS: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.48.2");
const ID_PKIX_OCSP_BASIC: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.48.1.1");
const ID_PKIX_OCSP_NOCHECK: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.48.1.5");
const AUTHORITY_INFO_ACCESS: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.1.1");
const CRL_DISTRIBUTION_POINTS: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.29.31");
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

/// What an OCSP response says of `cert`: good, with the responder's
/// certificates, or why it cannot be used.
fn read_ocsp(
    response: &[u8],
    cert: &Certificate,
    issuer: &Certificate,
) -> std::result::Result<Vec<Certificate>, String> {
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
            r.cert_id.serial_number == wanted.serial_number
                && r.cert_id.issuer_key_hash == wanted.issuer_key_hash
        })
        .ok_or("the response is about another certificate")?;
    match &single.cert_status {
        CertStatus::Good(_) => Ok(basic.certs.unwrap_or_default()),
        CertStatus::Revoked(info) => Err(format!(
            "REVOKED since {}",
            timestamp(std::time::UNIX_EPOCH + info.revocation_time.0.to_unix_duration())
        )),
        CertStatus::Unknown(_) => Err("the responder does not know the certificate".into()),
    }
}

/// What a CRL says of `cert`: `Ok` when it is not listed.
fn read_crl(crl: &[u8], cert: &Certificate) -> std::result::Result<Vec<u8>, String> {
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
    if let Some(revoked) = list
        .tbs_cert_list
        .revoked_certificates
        .iter()
        .flatten()
        .find(|r| r.serial_number == cert.tbs_certificate.serial_number)
    {
        return Err(format!(
            "REVOKED since {}",
            timestamp(std::time::UNIX_EPOCH + revoked.revocation_date.to_unix_duration())
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
        let mut issuer = found
            .pool
            .iter()
            .find(|c| c.tbs_certificate.subject == cert.tbs_certificate.issuer)
            .cloned();
        if issuer.is_none() {
            for url in aia_urls(&cert, ID_AD_CA_ISSUERS) {
                if let Ok(bytes) = fetcher
                    .fetch(&url, None, "the certificate issuer download")
                    .await
                    && let Some(fetched) = parse_certificate(&bytes)
                    && fetched.tbs_certificate.subject == cert.tbs_certificate.issuer
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
                match read_ocsp(&response, &cert, issuer) {
                    Ok(responder_certs) => {
                        found.out.ocsps.push(response);
                        for responder in responder_certs {
                            let needs_status = !has_extension(&responder, ID_PKIX_OCSP_NOCHECK)
                                && responder.tbs_certificate.subject
                                    != issuer.tbs_certificate.subject;
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
                match read_crl(&crl, &cert) {
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
