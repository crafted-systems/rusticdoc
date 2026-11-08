//! Cryptographic PDF signatures, PAdES baseline B-B style.
//!
//! The signature is an incremental update: the original bytes stay as they
//! are, and after them come a signature field, its widget, the AcroForm that
//! lists it, and a `/Sig` dictionary with `/Filter /Adobe.PPKLite` and
//! `/SubFilter /ETSI.CAdES.detached`. `/Contents` holds a detached CMS
//! `SignedData` over the SHA-256 of the two byte ranges around it, with the
//! signer's certificate and any chain certificates.
//!
//! What this is not: there is no RFC 3161 timestamp (B-T) and no revocation
//! data (B-LT, B-LTA), so a validator trusts the claimed signing time only as
//! far as it trusts the signer, and long-term validation is not possible.
//! There is one key per deployment.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use super::{blocking, lopdf::DEFAULT_MAX_BYTES, require_pdf};
use crate::error::{Error, Result};
use crate::job::Document;

mod cms;
mod incremental;
mod keys;
#[cfg(test)]
mod tests;

pub use keys::Signer;

/// Longest `reason`, `location`, `contactInfo` or `signerName`, in bytes.
pub const MAX_SIGN_TEXT: usize = 1024;

/// Headroom over the longest signature this signer can produce, so the
/// placeholder never runs short.
const PLACEHOLDER_HEADROOM: usize = 8192;

/// What a signature says about itself, and where it is seen.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SignOptions {
    pub reason: Option<String>,
    pub location: Option<String>,
    pub contact_info: Option<String>,
    /// `/Name`: who signed, as the signer wants it shown. Validators take the
    /// signer's identity from the certificate, not from here.
    pub signer_name: Option<String>,
    /// An image to show the signature with. Without one the signature is
    /// invisible.
    pub appearance: Option<Appearance>,
}

/// A visible signature: an image placed on a page, as the widget's appearance.
#[derive(Clone, PartialEq)]
pub struct Appearance {
    /// 1-based.
    pub page: u32,
    /// Points from the page's left edge to the image's left edge.
    pub x: f32,
    /// Points from the page's bottom edge to the image's bottom edge.
    pub y: f32,
    /// Width in points; the image's pixel width when absent.
    pub width: Option<f32>,
    /// PNG or JPEG bytes.
    pub image: Vec<u8>,
    pub name: String,
}

impl std::fmt::Debug for Appearance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Appearance")
            .field("page", &self.page)
            .field("x", &self.x)
            .field("y", &self.y)
            .field("width", &self.width)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl SignOptions {
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
            let png = a.image.starts_with(b"\x89PNG\r\n\x1a\n");
            let jpeg = a.image.starts_with(b"\xff\xd8\xff");
            if !(png || jpeg) {
                return Err(Error::InvalidRequest(format!(
                    "'{}' is not a PNG or JPEG image",
                    a.name
                )));
            }
        }
        Ok(())
    }
}

/// Signs `doc` with `signer`, off the async threads and under `timeout`.
pub async fn sign(
    doc: &Document,
    signer: Arc<Signer>,
    options: &SignOptions,
    timeout: Duration,
) -> Result<Document> {
    require_pdf(doc)?;
    if doc.bytes.len() > DEFAULT_MAX_BYTES {
        return Err(Error::InvalidRequest(format!(
            "'{}' is {} bytes, over the {DEFAULT_MAX_BYTES} byte limit",
            doc.name,
            doc.bytes.len()
        )));
    }
    options.validate()?;
    let doc = doc.clone();
    let options = options.clone();
    blocking(timeout, move |deadline| {
        let signed = sign_at(&doc, &signer, &options, SystemTime::now())?;
        deadline.check()?;
        Ok(signed)
    })
    .await
}

/// Signs `doc` as of `now`, which becomes the signature's `/M`.
pub fn sign_at(
    doc: &Document,
    signer: &Signer,
    options: &SignOptions,
    now: SystemTime,
) -> Result<Document> {
    require_pdf(doc)?;
    options.validate()?;
    signer.usable_at(now)?;
    let pdf = super::lopdf::load(doc)?;
    let reserve = cms::max_len(signer)? + PLACEHOLDER_HEADROOM;
    let mut update = incremental::prepare(&doc.bytes, &pdf, &doc.name, options, reserve, now)?;
    drop(pdf);
    let mut hasher = Sha256::new();
    for range in update.signed_ranges() {
        hasher.update(range);
    }
    let digest: [u8; 32] = hasher.finalize().into();
    let signature = hex::encode_upper(cms::signed_data(signer, &digest)?);
    if signature.len() > update.contents.len() {
        return Err(Error::Engine {
            engine: "signer",
            message: format!(
                "the signature is {} hex digits, more than the {} reserved for it",
                signature.len(),
                update.contents.len()
            ),
        });
    }
    let start = update.contents.start;
    update.bytes[start..start + signature.len()].copy_from_slice(signature.as_bytes());
    Ok(Document::new(doc.name.clone(), update.bytes))
}
