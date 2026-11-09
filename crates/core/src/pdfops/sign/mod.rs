//! Cryptographic PDF signatures, PAdES baseline B-B to B-LTA.
//!
//! A signature is an incremental update: the original bytes stay as they are,
//! and after them come a signature field, its widget, the AcroForm that lists
//! it, and a `/Sig` dictionary with `/Filter /Adobe.PPKLite` and `/SubFilter
//! /ETSI.CAdES.detached`. `/Contents` holds a detached CMS `SignedData` over
//! the SHA-256 of the two byte ranges around it, with the signer's
//! certificate and any chain certificates. That is B-B.
//!
//! - B-T adds an RFC 3161 timestamp over the signature value, from the
//!   configured timestamp authority, as the `id-aa-signatureTimeStampToken`
//!   unsigned attribute.
//! - B-LT appends a second update with a `/DSS`: the certificates of the
//!   signer's and the authority's chains, and an OCSP response or CRL for each
//!   (see [`revocation`]), with a `/VRI` entry for the signature.
//! - B-LTA appends a third: a document timestamp (`/Type /DocTimeStamp`,
//!   `/SubFilter /ETSI.RFC3161`) over the whole file, DSS included.
//!
//! A signature can certify instead of approve (DocMDP, permission 1 to 3).
//! Keys come from PEM files or a PKCS#11 token; besides the default key there
//! can be named keys, each with the callers allowed to use it. A timestamp
//! authority or revocation source that fails makes the request fail: no file
//! is returned at a lower level than asked for.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use sha2::Digest as _;

use super::{blocking, lopdf::DEFAULT_MAX_BYTES, require_pdf};
use crate::error::{Error, Result};
use crate::job::Document;

mod cms;
mod dss;
mod incremental;
mod keys;
#[cfg(test)]
mod level_tests;
pub mod net;
pub mod pkcs11;
pub mod revocation;
mod settings;
#[cfg(test)]
mod tests;
pub mod tsa;
mod verify;

pub use keys::Signer;
pub use net::{Outbound, OutboundRequest};
pub use pkcs11::{Pkcs11Settings, TokenKeySpec};
pub use revocation::Limits;
pub use settings::{NamedKey, Settings, check_tsa_url, load_key};
pub use tsa::Tsa;

/// Longest `reason`, `location`, `contactInfo` or `signerName`, in bytes.
pub const MAX_SIGN_TEXT: usize = 1024;

/// Headroom over the longest signature this signer can produce, so the
/// placeholder never runs short.
const PLACEHOLDER_HEADROOM: usize = 8192;

/// How far a signature goes, after ETSI EN 319 142-1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Level {
    /// The signature alone.
    #[default]
    BB,
    /// With a signature timestamp.
    BT,
    /// With validation data in a DSS.
    BLt,
    /// With a document timestamp over the DSS.
    BLta,
}

impl Level {
    /// `b-b`, `b-t`, `b-lt` or `b-lta`, in any case, with or without `pades-`.
    pub fn parse(s: &str) -> Result<Self> {
        let lower = s.trim().to_ascii_lowercase();
        Ok(match lower.trim_start_matches("pades-") {
            "b-b" | "bb" => Level::BB,
            "b-t" | "bt" => Level::BT,
            "b-lt" | "blt" => Level::BLt,
            "b-lta" | "blta" => Level::BLta,
            _ => {
                return Err(Error::InvalidRequest(format!(
                    "'level' must be b-b, b-t, b-lt or b-lta, not '{}'",
                    s.trim()
                )));
            }
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Level::BB => "b-b",
            Level::BT => "b-t",
            Level::BLt => "b-lt",
            Level::BLta => "b-lta",
        }
    }
}

/// What a certification signature lets others do afterwards (ISO 32000
/// DocMDP). Every level still allows DSS updates and document timestamps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocMdp {
    /// 1: no changes.
    NoChanges,
    /// 2: filling forms and signing.
    FormFilling,
    /// 3: that, and annotations.
    Annotations,
}

impl DocMdp {
    /// `1`, `2` or `3`.
    pub fn parse(s: &str) -> Result<Self> {
        match s.trim() {
            "1" => Ok(DocMdp::NoChanges),
            "2" => Ok(DocMdp::FormFilling),
            "3" => Ok(DocMdp::Annotations),
            other => Err(Error::InvalidRequest(format!(
                "'certify' must be 1 (no changes), 2 (form filling and signing) or 3 (also annotations), not '{other}'"
            ))),
        }
    }

    pub fn level(self) -> u8 {
        match self {
            DocMdp::NoChanges => 1,
            DocMdp::FormFilling => 2,
            DocMdp::Annotations => 3,
        }
    }
}

/// What a signature says about itself, where it is seen, how far it goes and
/// which key makes it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SignOptions {
    pub reason: Option<String>,
    pub location: Option<String>,
    pub contact_info: Option<String>,
    /// `/Name`: who signed, as the signer wants it shown. Validators take the
    /// signer's identity from the certificate, not from here.
    pub signer_name: Option<String>,
    /// Where the signature is shown, with an image or as text. Without one
    /// the signature is invisible.
    pub appearance: Option<Appearance>,
    /// The PAdES level; the service's default when absent.
    pub level: Option<Level>,
    /// Certify with this DocMDP permission instead of approving.
    pub certify: Option<DocMdp>,
    /// A named key instead of the default one.
    pub key: Option<String>,
}

/// A visible signature: an image, or text naming the signer, the date and the
/// reason, placed on a page as the widget's appearance.
#[derive(Clone, PartialEq)]
pub struct Appearance {
    /// 1-based.
    pub page: u32,
    /// Points from the page's left edge to the appearance's left edge.
    pub x: f32,
    /// Points from the page's bottom edge to the appearance's bottom edge.
    pub y: f32,
    /// Width in points; the image's pixel width, or 200 for text, when absent.
    pub width: Option<f32>,
    /// PNG or JPEG bytes. Text is drawn when absent.
    pub image: Option<Vec<u8>>,
    /// The image's file name, for messages.
    pub name: String,
}

impl std::fmt::Debug for Appearance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Appearance")
            .field("page", &self.page)
            .field("x", &self.x)
            .field("y", &self.y)
            .field("width", &self.width)
            .field("image", &self.image.as_ref().map(Vec::len))
            .field("name", &self.name)
            .finish()
    }
}

impl SignOptions {
    /// The options a form or a command line names, read through `field`:
    /// `reason`, `location`, `contactInfo`, `signerName`, `level`, `certify`
    /// and `signingKey`. The appearance is left to the caller, which has the
    /// files.
    pub fn from_fields(field: &dyn Fn(&str) -> Option<String>) -> Result<Self> {
        let text = |name: &str| {
            field(name)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let options = Self {
            reason: text("reason"),
            location: text("location"),
            contact_info: text("contactInfo"),
            signer_name: text("signerName"),
            appearance: None,
            level: text("level").map(|v| Level::parse(&v)).transpose()?,
            certify: text("certify").map(|v| DocMdp::parse(&v)).transpose()?,
            key: text("signingKey"),
        };
        options.validate()?;
        Ok(options)
    }

    /// Refuses what should not reach a signature dictionary.
    pub fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("reason", &self.reason),
            ("location", &self.location),
            ("contactInfo", &self.contact_info),
            ("signerName", &self.signer_name),
        ] {
            if value.as_ref().is_some_and(|v| v.len() > MAX_SIGN_TEXT) {
                return Err(Error::InvalidRequest(format!(
                    "'{name}' is longer than {MAX_SIGN_TEXT} bytes"
                )));
            }
        }
        if let Some(key) = &self.key
            && !settings::valid_key_name(key)
        {
            return Err(Error::InvalidRequest(format!(
                "'signingKey' must be a key name of letters, digits, '-' and '_', not '{key}'"
            )));
        }
        if let Some(a) = &self.appearance {
            if a.page == 0 {
                return Err(Error::InvalidRequest(
                    "'page' must be a page number from 1".into(),
                ));
            }
            for (name, value) in [("x", Some(a.x)), ("y", Some(a.y)), ("width", a.width)] {
                if value.is_some_and(|v| !v.is_finite() || v < 0.0) {
                    return Err(Error::InvalidRequest(format!(
                        "'{name}' must be a non-negative number"
                    )));
                }
            }
            if a.width == Some(0.0) {
                return Err(Error::InvalidRequest("'width' must be above 0".into()));
            }
            if let Some(image) = &a.image {
                let png = image.starts_with(b"\x89PNG\r\n\x1a\n");
                let jpeg = image.starts_with(b"\xff\xd8\xff");
                if !(png || jpeg) {
                    return Err(Error::InvalidRequest(format!(
                        "'{}' is not a PNG or JPEG image",
                        a.name
                    )));
                }
            }
        }
        Ok(())
    }
}

/// The network a signature beyond B-B uses.
#[derive(Clone, Copy)]
pub struct Network<'a> {
    pub tsa: Option<&'a Tsa>,
    pub outbound: Option<&'a dyn Outbound>,
    pub limits: &'a Limits,
}

/// Every key the service signs with, who may use each, and what levels
/// beyond B-B need: the timestamp authority, the outbound client and the
/// limits on fetching validation data.
#[derive(Debug, Default)]
pub struct Signing {
    default: Option<Arc<Signer>>,
    /// Who may use the default key; every caller when `None`.
    default_allow: Option<Vec<String>>,
    named: BTreeMap<String, (Arc<Signer>, Vec<String>)>,
    tsa: Option<Tsa>,
    level: Level,
    limits: Limits,
    outbound: Option<Arc<dyn Outbound>>,
}

impl Signing {
    /// A service with one key that every caller may use, at B-B by default.
    pub fn with_default(signer: Signer) -> Self {
        Self {
            default: Some(Arc::new(signer)),
            ..Self::default()
        }
    }

    /// Reads every key the settings name, and keeps the rest. Errors name the
    /// variable and the file, never what a key file holds.
    pub fn load(
        default_key: Option<(&str, &std::path::Path)>,
        settings: &Settings,
    ) -> std::result::Result<Self, String> {
        let default = match default_key {
            Some((key, cert)) => Some(Arc::new(
                settings::load_key(key, cert, &settings.pkcs11).map_err(|e| {
                    format!("RUSTICDOC_SIGNING_KEY and RUSTICDOC_SIGNING_CERT: {e}")
                })?,
            )),
            None => None,
        };
        if default.is_none() && settings.default_allow.is_some() {
            return Err(
                "RUSTICDOC_SIGNING_ALLOW is set, but there is no default key (RUSTICDOC_SIGNING_KEY)"
                    .into(),
            );
        }
        let mut named = BTreeMap::new();
        for key in &settings.named {
            let signer = settings::load_key(&key.key, &key.cert, &settings.pkcs11)
                .map_err(|e| format!("signing key '{}': {e}", key.name))?;
            named.insert(key.name.clone(), (Arc::new(signer), key.allow.clone()));
        }
        let tsa = match &settings.tsa {
            Some(tsa) => {
                let mut tsa = tsa.clone();
                tsa.load_anchors()?;
                Some(tsa)
            }
            None => None,
        };
        Ok(Self {
            default,
            default_allow: settings.default_allow.clone(),
            named,
            tsa,
            level: settings.level,
            limits: settings.revocation.clone(),
            outbound: None,
        })
    }

    /// Adds a named key with its allow list.
    pub fn with_named(mut self, name: &str, signer: Signer, allow: Vec<String>) -> Self {
        self.named
            .insert(name.to_string(), (Arc::new(signer), allow));
        self
    }

    pub fn with_tsa(mut self, tsa: Tsa) -> Self {
        self.tsa = Some(tsa);
        self
    }

    pub fn with_level(mut self, level: Level) -> Self {
        self.level = level;
        self
    }

    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// The client for the timestamp authority and validation data.
    pub fn set_outbound(&mut self, outbound: Arc<dyn Outbound>) {
        self.outbound = Some(outbound);
    }

    pub fn with_outbound(mut self, outbound: Arc<dyn Outbound>) -> Self {
        self.set_outbound(outbound);
        self
    }

    /// True when some key is loaded.
    pub fn is_configured(&self) -> bool {
        self.default.is_some() || !self.named.is_empty()
    }

    /// The loaded keys: `None` for the default, then the named ones.
    pub fn keys(&self) -> impl Iterator<Item = (Option<&str>, &Arc<Signer>)> {
        self.default
            .iter()
            .map(|s| (None, s))
            .chain(self.named.iter().map(|(n, (s, _))| (Some(n.as_str()), s)))
    }

    /// The timestamp authority, when one is configured.
    pub fn tsa(&self) -> Option<&Tsa> {
        self.tsa.as_ref()
    }

    /// The level a request gets when it names none.
    pub fn default_level(&self) -> Level {
        self.level
    }

    /// The key `name` names, or the default one. A 503 when signing is not
    /// configured at all, a 400 for a name that is not configured.
    pub fn key(&self, name: Option<&str>) -> Result<&Arc<Signer>> {
        match name {
            None => self.default.as_ref().ok_or_else(|| {
                Error::Status(
                    503,
                    if self.named.is_empty() {
                        "cryptographic signing is not configured on this service; its operator sets RUSTICDOC_SIGNING_KEY and RUSTICDOC_SIGNING_CERT"
                    } else {
                        "this service has no default signing key; name one with 'signingKey'"
                    }
                    .into(),
                )
            }),
            Some(name) => self.named.get(name).map(|(s, _)| s).ok_or_else(|| {
                if self.is_configured() {
                    Error::InvalidRequest(format!("there is no signing key named '{name}'"))
                } else {
                    Error::Status(
                        503,
                        "cryptographic signing is not configured on this service; its operator sets RUSTICDOC_SIGNING_KEY and RUSTICDOC_SIGNING_CERT".into(),
                    )
                }
            }),
        }
    }

    /// Who may use the key `name` names: `None` when every caller may. The
    /// server matches the caller against it.
    pub fn allow_list(&self, name: Option<&str>) -> Result<Option<&[String]>> {
        self.key(name)?;
        Ok(match name {
            None => self.default_allow.as_deref(),
            Some(name) => self.named.get(name).map(|(_, allow)| allow.as_slice()),
        })
    }

    /// Refuses a signature this service cannot make, before any work: no
    /// key, or a level whose timestamp authority or network is missing.
    pub fn check(&self, options: &SignOptions) -> Result<()> {
        self.key(options.key.as_deref())?;
        options.validate()?;
        let level = options.level.unwrap_or(self.level);
        if level >= Level::BT {
            match &self.tsa {
                None => {
                    return Err(Error::Status(
                        503,
                        format!(
                            "level {} needs a timestamp authority; this service's operator sets RUSTICDOC_SIGNING_TSA_URL",
                            level.name()
                        ),
                    ));
                }
                Some(tsa) if !tsa.has_anchors() => {
                    return Err(Error::Status(
                        503,
                        "the timestamp authority has no trusted certificate to check its tokens against; this service's operator sets RUSTICDOC_SIGNING_TSA_CERT".into(),
                    ));
                }
                Some(_) => {}
            }
            if self.outbound.is_none() {
                return Err(Error::Status(
                    503,
                    format!(
                        "level {} needs outbound requests, which this build of the service cannot make",
                        level.name()
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Signs `doc` as `options` say, within `timeout` for the whole of it,
    /// timestamp and validation data included.
    pub async fn sign(
        &self,
        doc: &Document,
        options: &SignOptions,
        timeout: Duration,
    ) -> Result<Document> {
        self.check(options)?;
        let signer = self.key(options.key.as_deref())?.clone();
        let mut options = options.clone();
        options.level = Some(options.level.unwrap_or(self.level));
        let network = Network {
            tsa: self.tsa.as_ref(),
            outbound: self.outbound.as_deref(),
            limits: &self.limits,
        };
        sign_with(doc, signer, &options, network, SystemTime::now(), timeout).await
    }
}

/// Signs `doc` with `signer` at B-B, off the async threads and under
/// `timeout`.
pub async fn sign(
    doc: &Document,
    signer: Arc<Signer>,
    options: &SignOptions,
    timeout: Duration,
) -> Result<Document> {
    let limits = Limits::default();
    let network = Network {
        tsa: None,
        outbound: None,
        limits: &limits,
    };
    let mut options = options.clone();
    options.level.get_or_insert(Level::BB);
    sign_with(doc, signer, &options, network, SystemTime::now(), timeout).await
}

fn check_size(doc: &Document) -> Result<()> {
    require_pdf(doc)?;
    if doc.bytes.len() > DEFAULT_MAX_BYTES {
        return Err(Error::InvalidRequest(format!(
            "'{}' is {} bytes, over the {DEFAULT_MAX_BYTES} byte limit",
            doc.name,
            doc.bytes.len()
        )));
    }
    Ok(())
}

fn time_left(deadline: Instant, timeout: Duration) -> Result<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(Error::Timeout(timeout));
    }
    Ok(left)
}

/// The prepared update and the CMS signature over it, timestamp not yet added.
struct Prepared {
    update: incremental::Update,
    signed_data: ::cms::signed_data::SignedData,
}

fn prepare_signature(
    doc: &Document,
    signer: &Signer,
    options: &SignOptions,
    now: SystemTime,
    token_reserve: usize,
) -> Result<Prepared> {
    require_pdf(doc)?;
    options.validate()?;
    signer.usable_at(now)?;
    let pdf = super::lopdf::load(doc)?;
    let reserve = cms::max_len(signer, token_reserve)? + PLACEHOLDER_HEADROOM;
    let shown_name = options
        .signer_name
        .clone()
        .unwrap_or_else(|| signer.common_name());
    let update = incremental::prepare(
        &doc.bytes,
        &pdf,
        &doc.name,
        incremental::Kind::Signature {
            options,
            now,
            shown_name: &shown_name,
        },
        reserve,
    )?;
    drop(pdf);
    let signed_data = cms::signed_data(signer, &update.digest())?;
    Ok(Prepared {
        update,
        signed_data,
    })
}

/// Signs `doc` at `options.level` (B-B when absent) as of `now`, which becomes
/// the signature's `/M`, within `timeout`.
pub async fn sign_with(
    doc: &Document,
    signer: Arc<Signer>,
    options: &SignOptions,
    network: Network<'_>,
    now: SystemTime,
    timeout: Duration,
) -> Result<Document> {
    check_size(doc)?;
    options.validate()?;
    let deadline = Instant::now() + timeout;
    let level = options.level.unwrap_or_default();
    let stamping = if level >= Level::BT {
        let tsa = network
            .tsa
            .ok_or_else(|| Error::Status(503, "no timestamp authority is configured".into()))?;
        let outbound = network
            .outbound
            .ok_or_else(|| Error::Status(503, "no outbound client is configured".into()))?;
        Some((tsa, outbound))
    } else {
        None
    };

    // The signature, and for B-T its timestamp. A token larger than the room
    // left for it means one more try with room for that token.
    let mut token_reserve = stamping.map_or(0, |(tsa, _)| tsa.reserve());
    let mut signature_token = None;
    let mut attempt = 0;
    let (mut bytes, contents) = loop {
        attempt += 1;
        let (d, s, o) = (doc.clone(), signer.clone(), options.clone());
        let Prepared {
            mut update,
            mut signed_data,
        } = blocking(time_left(deadline, timeout)?, move |_| {
            prepare_signature(&d, &s, &o, now, token_reserve)
        })
        .await?;
        if let Some((tsa, outbound)) = stamping {
            let value = cms::signature_value(&signed_data);
            let digest: [u8; 32] = sha2::Sha256::digest(&value).into();
            let token = tsa
                .stamp(outbound, &digest, time_left(deadline, timeout)?)
                .await?;
            cms::add_timestamp(&mut signed_data, &token.der)?;
            signature_token = Some(token);
        }
        let der = cms::encode(&signed_data)?;
        if 2 * der.len() > update.contents.len() && attempt == 1 && stamping.is_some() {
            token_reserve = signature_token.as_ref().map_or(0, |t| t.der.len()) + 1024;
            continue;
        }
        update.fill(&der)?;
        break (update.bytes, update.contents);
    };

    if level >= Level::BLt {
        let (tsa, outbound) = stamping.expect("B-LT implies B-T");
        let mut certs = signer.chain().to_vec();
        certs.extend(
            signature_token
                .as_ref()
                .map(|t| t.certificates.clone())
                .unwrap_or_default(),
        );
        let validation = revocation::collect(
            &certs,
            outbound,
            network.limits,
            deadline,
            SystemTime::now(),
        )
        .await?;
        let signature = hex::decode(&bytes[contents.clone()]).map_err(incremental::writing)?;
        let name = doc.name.clone();
        bytes = blocking(time_left(deadline, timeout)?, move |_| {
            dss::append(&bytes, &name, &validation, &[signature])
        })
        .await?;

        if level >= Level::BLta {
            let reserve = signature_token
                .as_ref()
                .map_or(0, |t| t.der.len())
                .max(tsa.reserve())
                + PLACEHOLDER_HEADROOM;
            let name = doc.name.clone();
            let mut update = blocking(time_left(deadline, timeout)?, move |_| {
                let pdf = ::lopdf::Document::load_mem(&bytes).map_err(incremental::writing)?;
                incremental::prepare(
                    &bytes,
                    &pdf,
                    &name,
                    incremental::Kind::DocTimeStamp,
                    reserve,
                )
            })
            .await?;
            let token = tsa
                .stamp(outbound, &update.digest(), time_left(deadline, timeout)?)
                .await?;
            update.fill(&token.der)?;
            bytes = update.bytes;
        }
    }
    Ok(Document::new(doc.name.clone(), bytes))
}

/// Signs `doc` at B-B as of `now`, which becomes the signature's `/M`.
pub fn sign_at(
    doc: &Document,
    signer: &Signer,
    options: &SignOptions,
    now: SystemTime,
) -> Result<Document> {
    if options.level.is_some_and(|l| l > Level::BB) {
        return Err(Error::InvalidRequest(
            "sign_at makes B-B signatures; use sign_with for higher levels".into(),
        ));
    }
    let Prepared {
        mut update,
        signed_data,
    } = prepare_signature(doc, signer, options, now, 0)?;
    update.fill(&cms::encode(&signed_data)?)?;
    Ok(Document::new(doc.name.clone(), update.bytes))
}
