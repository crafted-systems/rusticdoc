//! Checks on what other parties sign: timestamp tokens, and the keys this
//! service signs with when they live in a token. Public-key operations only.

use aws_lc_rs::digest;
use aws_lc_rs::signature::{
    ECDSA_P256_SHA256_ASN1, ECDSA_P256_SHA384_ASN1, ECDSA_P384_SHA256_ASN1, ECDSA_P384_SHA384_ASN1,
    ECDSA_P384_SHA512_ASN1, RSA_PKCS1_2048_8192_SHA256, RSA_PKCS1_2048_8192_SHA384,
    RSA_PKCS1_2048_8192_SHA512, UnparsedPublicKey, VerificationAlgorithm,
};
use der::Decode;
use der::asn1::{ObjectIdentifier, OctetString};
use x509_cert::Certificate;

const SHA1: &str = "1.3.14.3.2.26";
const SHA256: &str = "2.16.840.1.101.3.4.2.1";
const SHA384: &str = "2.16.840.1.101.3.4.2.2";
const SHA512: &str = "2.16.840.1.101.3.4.2.3";
const SUBJECT_KEY_IDENTIFIER: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.29.14");

/// The digest of `data` with the algorithm `oid` names, if it is one of the
/// SHA family.
pub(super) fn digest(oid: &ObjectIdentifier, data: &[u8]) -> Option<Vec<u8>> {
    let algorithm = match oid.to_string().as_str() {
        SHA1 => &digest::SHA1_FOR_LEGACY_USE_ONLY,
        SHA256 => &digest::SHA256,
        SHA384 => &digest::SHA384,
        SHA512 => &digest::SHA512,
        _ => return None,
    };
    Some(digest::digest(algorithm, data).as_ref().to_vec())
}

/// SHA-1, for OCSP certificate identifiers and DSS VRI keys, where the
/// formats name it.
pub(super) fn sha1(data: &[u8]) -> Vec<u8> {
    digest::digest(&digest::SHA1_FOR_LEGACY_USE_ONLY, data)
        .as_ref()
        .to_vec()
}

/// The certificate's subject key identifier extension.
pub(super) fn subject_key_id(cert: &Certificate) -> Option<Vec<u8>> {
    cert.tbs_certificate
        .extensions
        .iter()
        .flatten()
        .find(|e| e.extn_id == SUBJECT_KEY_IDENTIFIER)
        .and_then(|e| OctetString::from_der(e.extn_value.as_bytes()).ok())
        .map(|o| o.as_bytes().to_vec())
}

/// Verifies `signature` over `message` with `cert`'s public key. The digest
/// is `digest_oid`; `signature_oid` is either a combined algorithm
/// (sha256WithRSAEncryption, ecdsa-with-SHA384, ...) or the bare key
/// algorithm, as CMS allows.
pub(super) fn signature(
    cert: &Certificate,
    digest_oid: &ObjectIdentifier,
    signature_oid: &ObjectIdentifier,
    message: &[u8],
    signature: &[u8],
) -> Result<(), String> {
    let spki = &cert.tbs_certificate.subject_public_key_info;
    let digest = match signature_oid.to_string().as_str() {
        "1.2.840.113549.1.1.11" | "1.2.840.10045.4.3.2" => SHA256.to_string(),
        "1.2.840.113549.1.1.12" | "1.2.840.10045.4.3.3" => SHA384.to_string(),
        "1.2.840.113549.1.1.13" | "1.2.840.10045.4.3.4" => SHA512.to_string(),
        _ => digest_oid.to_string(),
    };
    let curve = spki
        .algorithm
        .parameters
        .as_ref()
        .and_then(|p| p.decode_as::<ObjectIdentifier>().ok())
        .map(|o| o.to_string());
    let algorithm: &dyn VerificationAlgorithm = match (
        spki.algorithm.oid.to_string().as_str(),
        curve.as_deref(),
        digest.as_str(),
    ) {
        ("1.2.840.113549.1.1.1", _, SHA256) => &RSA_PKCS1_2048_8192_SHA256,
        ("1.2.840.113549.1.1.1", _, SHA384) => &RSA_PKCS1_2048_8192_SHA384,
        ("1.2.840.113549.1.1.1", _, SHA512) => &RSA_PKCS1_2048_8192_SHA512,
        ("1.2.840.10045.2.1", Some("1.2.840.10045.3.1.7"), SHA256) => &ECDSA_P256_SHA256_ASN1,
        ("1.2.840.10045.2.1", Some("1.2.840.10045.3.1.7"), SHA384) => &ECDSA_P256_SHA384_ASN1,
        ("1.2.840.10045.2.1", Some("1.3.132.0.34"), SHA256) => &ECDSA_P384_SHA256_ASN1,
        ("1.2.840.10045.2.1", Some("1.3.132.0.34"), SHA384) => &ECDSA_P384_SHA384_ASN1,
        ("1.2.840.10045.2.1", Some("1.3.132.0.34"), SHA512) => &ECDSA_P384_SHA512_ASN1,
        (key, _, digest) => {
            return Err(format!(
                "uses an algorithm this service cannot check (key {key}, digest {digest})"
            ));
        }
    };
    UnparsedPublicKey::new(algorithm, spki.subject_public_key.raw_bytes())
        .verify(message, signature)
        .map_err(|_| "does not verify with its certificate's key".to_string())
}
