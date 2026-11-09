//! The Document Security Store of PAdES B-LT (ETSI EN 319 142-1, 5.4.2): an
//! incremental update whose catalog gains `/DSS`, with the certificates, OCSP
//! responses and CRLs a validator needs as streams, and a `/VRI` entry per
//! signature, keyed by the uppercase hex SHA-1 of its `/Contents`, that lists
//! the same streams. A DSS already in the file is kept and added to.

use ::lopdf::{Dictionary, Document as Pdf, Object, ObjectId, Stream};

use super::incremental::{Appender, array_in, dict_in, writing};
use super::revocation::Validation;
use super::verify::sha1;
use crate::error::Result;

/// A stream holding `der`, compressed.
fn stream(der: &[u8]) -> Result<Object> {
    let mut stream = Stream::new(Dictionary::new(), der.to_vec());
    stream.compress().map_err(writing)?;
    Ok(Object::Stream(stream))
}

/// The VRI key of a signature whose `/Contents` holds `contents` (the bytes
/// the hex string stands for, padding included, as validators hash them).
pub(super) fn vri_key(contents: &[u8]) -> String {
    hex::encode_upper(sha1(contents))
}

/// Appends a DSS update to `original` with `validation`, and a VRI entry for
/// each signature in `signatures` (their `/Contents` bytes).
pub(super) fn append(
    original: &[u8],
    name: &str,
    validation: &Validation,
    signatures: &[Vec<u8>],
) -> Result<Vec<u8>> {
    let pdf = Pdf::load_mem(original).map_err(|e| crate::error::Error::Engine {
        engine: "signer",
        message: format!("cannot read the signed file back to add validation data: {e}"),
    })?;
    let mut up = Appender::new(original, &pdf, name)?;
    let mut catalog = up.catalog();
    let mut dss = dict_in(&pdf, &catalog, b"DSS").unwrap_or_default();

    let add_all = |up: &mut Appender<'_>, ders: &[Vec<u8>]| -> Result<Vec<ObjectId>> {
        ders.iter().map(|d| Ok(up.add(stream(d)?))).collect()
    };
    let certs = add_all(&mut up, &validation.certs)?;
    let ocsps = add_all(&mut up, &validation.ocsps)?;
    let crls = add_all(&mut up, &validation.crls)?;

    let refs = |ids: &[ObjectId]| ids.iter().map(|id| Object::from(*id)).collect::<Vec<_>>();
    for (key, ids) in [("Certs", &certs), ("OCSPs", &ocsps), ("CRLs", &crls)] {
        if ids.is_empty() {
            continue;
        }
        let (mut array, _) = array_in(&pdf, &dss, key.as_bytes());
        array.extend(refs(ids));
        dss.set(key, array);
    }
    let mut vri = dict_in(&pdf, &dss, b"VRI").unwrap_or_default();
    for contents in signatures {
        let mut entry = Dictionary::new();
        for (key, ids) in [("Cert", &certs), ("OCSP", &ocsps), ("CRL", &crls)] {
            if !ids.is_empty() {
                entry.set(key, refs(ids));
            }
        }
        vri.set(vri_key(contents), entry);
    }
    if !vri.is_empty() {
        dss.set("VRI", vri);
    }
    dss.set("Type", "DSS");
    let dss_id = up.add(Object::Dictionary(dss));
    catalog.set("DSS", dss_id);
    let root_id = up.root_id;
    up.put(root_id, Object::Dictionary(catalog));
    Ok(up.finish(None)?.bytes)
}
