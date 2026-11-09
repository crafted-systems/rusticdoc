//! The signing key and its certificate chain: read from PEM, checked against
//! each other once, then used for every signature.
//!
//! The private-key operations run in aws-lc-rs. RustCrypto's `rsa` crate has
//! an unpatched timing advisory (RUSTSEC-2023-0071) that matters exactly here,
//! in a service whose response times a client can measure.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{
    ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair, RSA_PKCS1_SHA256, RsaKeyPair,
};
use der::asn1::{Any, ObjectIdentifier};
use der::{Decode, Encode};
use x509_cert::Certificate;
use x509_cert::ext::pkix::KeyUsage;
use x509_cert::spki::AlgorithmIdentifierOwned;

use super::pkcs11::{Pkcs11Settings, TokenKey, TokenKeySpec};
use crate::error::{Error, Result};

const RSA_ENCRYPTION: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");
const EC_PUBLIC_KEY: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.2.1");
const SECP256R1: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.3.1.7");
const ECDSA_WITH_SHA256: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.2");
const KEY_USAGE: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.29.15");
const COMMON_NAME: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.3");

/// Most certificates a chain file may hold. Real chains have two or three.
const MAX_CHAIN: usize = 10;

/// Largest key or certificate file read. Real ones are a few kilobytes.
const MAX_PEM_BYTES: u64 = 1024 * 1024;

enum Key {
    Rsa(RsaKeyPair),
    P256(EcdsaKeyPair),
    /// A key on a PKCS#11 token, of the kind its certificate says.
    Token(TokenKey, TokenKind),
}

#[derive(Clone, Copy)]
enum TokenKind {
    /// With the modulus length in bytes.
    Rsa(usize),
    P256,
}

/// RFC 8017 `RSAPublicKey`, to read a modulus length from a certificate.
#[derive(der::Sequence)]
struct RsaPublicKey {
    modulus: der::asn1::Uint,
    public_exponent: der::asn1::Uint,
}

/// An ECDSA signature as DER, from the `r || s` a PKCS#11 token returns.
#[derive(der::Sequence)]
struct EcdsaSignature<'a> {
    r: der::asn1::UintRef<'a>,
    s: der::asn1::UintRef<'a>,
}

/// A private key with the certificate it belongs to, and any chain
/// certificates to send along. Its `Debug` output names the certificate and
/// the algorithm, never the key.
pub struct Signer {
    key: Key,
    /// The signer's certificate first, then the chain as given.
    chain: Vec<Certificate>,
    subject: String,
    not_before: SystemTime,
    not_after: SystemTime,
    warnings: Vec<String>,
}

impl std::fmt::Debug for Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Signer")
            .field("subject", &self.subject)
            .field("algorithm", &self.algorithm())
            .field("chain", &self.chain.len())
            .finish_non_exhaustive()
    }
}

impl Signer {
    /// Reads a PEM private key (PKCS#8, PKCS#1 or SEC1) and a PEM certificate
    /// file (the signer's certificate first, then any chain), and checks them
    /// as [`Signer::from_pem`] does. Errors name the file, never its content.
    pub fn from_files(key: &Path, cert: &Path) -> std::result::Result<Self, String> {
        let mut key_pem = read_limited(key, "signing key")?;
        let cert_pem = read_limited(cert, "signing certificate")?;
        let signer = Self::from_pem(&key_pem, &cert_pem);
        wipe(&mut key_pem);
        let mut signer = signer?;
        if let Some(warning) = readable_by_others(key) {
            signer.warnings.push(warning);
        }
        Ok(signer)
    }

    /// Parses and checks a key and certificate chain now: the key must match
    /// the first certificate, which must be within its validity period.
    pub fn from_pem(key_pem: &[u8], cert_pem: &[u8]) -> std::result::Result<Self, String> {
        Self::from_pem_at(key_pem, cert_pem, SystemTime::now())
    }

    pub(crate) fn from_pem_at(
        key_pem: &[u8],
        cert_pem: &[u8],
        now: SystemTime,
    ) -> std::result::Result<Self, String> {
        let key = parse_key(key_pem)?;
        let chain = parse_chain(cert_pem)?;
        Self::from_parts(key, chain, now)
    }

    /// Opens the key `spec` names on a PKCS#11 token and reads its
    /// certificate chain from `cert` (PEM, the signer's certificate first).
    /// The key must sign a test message that the certificate's public key
    /// verifies, which proves that they belong together.
    pub fn from_pkcs11(
        settings: &Pkcs11Settings,
        spec: &TokenKeySpec,
        cert: &Path,
    ) -> std::result::Result<Self, String> {
        let chain = parse_chain(&read_limited(cert, "signing certificate")?)?;
        let first = chain
            .first()
            .ok_or("the signing certificate file holds no certificate")?;
        let spki = &first.tbs_certificate.subject_public_key_info;
        let kind = if spki.algorithm.oid == RSA_ENCRYPTION {
            let key = RsaPublicKey::from_der(spki.subject_public_key.raw_bytes())
                .map_err(|e| format!("the signing certificate's RSA key cannot be read: {e}"))?;
            TokenKind::Rsa(key.modulus.as_bytes().len())
        } else if spki.algorithm.oid == EC_PUBLIC_KEY
            && spki
                .algorithm
                .parameters
                .as_ref()
                .and_then(|p| p.decode_as::<ObjectIdentifier>().ok())
                == Some(SECP256R1)
        {
            TokenKind::P256
        } else {
            return Err("the signing certificate's key is neither RSA nor ECDSA P-256".into());
        };
        let token = TokenKey::open(settings, spec)?;
        let shown = token.shown.clone();
        let key = Key::Token(token, kind);
        let probe = b"rusticdoc: does this key belong to this certificate?";
        let signature = sign_with(&key, probe).map_err(|e| {
            format!(
                "the PKCS#11 key {shown} cannot make the kind of signature the first certificate in the file ({}) needs, so it does not belong to it: {e}",
                first.tbs_certificate.subject
            )
        })?;
        super::verify::signature(
            first,
            &super::cms::ID_SHA256,
            &spki.algorithm.oid,
            probe,
            &signature,
        )
        .map_err(|_| {
            format!(
                "the PKCS#11 key {shown} does not belong to the first certificate in the file ({})",
                first.tbs_certificate.subject
            )
        })?;
        Self::from_parts(key, chain, SystemTime::now())
    }

    fn from_parts(
        key: Key,
        chain: Vec<Certificate>,
        now: SystemTime,
    ) -> std::result::Result<Self, String> {
        if chain.is_empty() {
            return Err("the signing certificate file holds no certificate".into());
        }
        if chain.len() > MAX_CHAIN {
            return Err(format!(
                "the signing certificate file holds {} certificates; at most {MAX_CHAIN} are sent with a signature",
                chain.len()
            ));
        }
        let cert = &chain[0];
        let tbs = &cert.tbs_certificate;
        let subject = tbs.subject.to_string();
        check_key_matches(&key, cert, &subject)?;
        let not_before = UNIX_EPOCH + tbs.validity.not_before.to_unix_duration();
        let not_after = UNIX_EPOCH + tbs.validity.not_after.to_unix_duration();
        let mut signer = Self {
            key,
            subject,
            not_before,
            not_after,
            warnings: Vec::new(),
            chain: Vec::new(),
        };
        signer.usable_at(now).map_err(|e| match e {
            Error::Status(_, message) => message,
            other => other.to_string(),
        })?;
        signer.warnings = warnings(&chain, now);
        signer.chain = chain;
        Ok(signer)
    }

    /// The signer certificate's subject, as RFC 4514 text.
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// `RSA-2048`, `ECDSA P-256` and so on.
    pub fn algorithm(&self) -> String {
        match &self.key {
            Key::Rsa(k) => format!("RSA-{}", k.public_modulus_len() * 8),
            Key::P256(_) => "ECDSA P-256".into(),
            Key::Token(t, TokenKind::Rsa(n)) => format!("RSA-{} on PKCS#11 ({})", n * 8, t.shown),
            Key::Token(t, TokenKind::P256) => format!("ECDSA P-256 on PKCS#11 ({})", t.shown),
        }
    }

    /// The common name of the signer's certificate, or its whole subject.
    pub fn common_name(&self) -> String {
        self.chain
            .first()
            .and_then(|c| {
                c.tbs_certificate
                    .subject
                    .0
                    .iter()
                    .flat_map(|rdn| rdn.0.iter())
                    .find(|atv| atv.oid == COMMON_NAME)
                    .and_then(|atv| {
                        atv.value.decode_as::<String>().ok().or_else(|| {
                            atv.value
                                .decode_as::<der::asn1::PrintableStringRef<'_>>()
                                .ok()
                                .map(|s| s.to_string())
                        })
                    })
            })
            .unwrap_or_else(|| self.subject.clone())
    }

    /// When the signer's certificate stops being valid.
    pub fn not_after(&self) -> SystemTime {
        self.not_after
    }

    /// [`Signer::not_after`] as `2026-11-07T12:00:00Z`.
    pub fn expiry(&self) -> String {
        timestamp(self.not_after)
    }

    /// What an operator should know but that does not stop signing: a key
    /// usage that validators may refuse, a self-signed certificate, a chain out
    /// of order, a key file others can read.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The signer's certificate and the chain, signer first.
    pub(super) fn chain(&self) -> &[Certificate] {
        &self.chain
    }

    /// Refuses to sign outside the certificate's validity period: the
    /// signature would be valid cryptographically and refused by every
    /// validator.
    pub(super) fn usable_at(&self, now: SystemTime) -> Result<()> {
        if now > self.not_after {
            return Err(Error::Status(
                503,
                format!(
                    "the signing certificate ({}) expired on {}",
                    self.subject,
                    timestamp(self.not_after)
                ),
            ));
        }
        if now < self.not_before {
            return Err(Error::Status(
                503,
                format!(
                    "the signing certificate ({}) is not valid before {}",
                    self.subject,
                    timestamp(self.not_before)
                ),
            ));
        }
        Ok(())
    }

    /// The CMS `signatureAlgorithm` of this key.
    pub(super) fn signature_algorithm(&self) -> AlgorithmIdentifierOwned {
        match &self.key {
            // rsaEncryption with the digest named by digestAlgorithm, as
            // OpenSSL and most PDF signers write it.
            Key::Rsa(_) | Key::Token(_, TokenKind::Rsa(_)) => AlgorithmIdentifierOwned {
                oid: RSA_ENCRYPTION,
                parameters: Some(Any::null()),
            },
            Key::P256(_) | Key::Token(_, TokenKind::P256) => AlgorithmIdentifierOwned {
                oid: ECDSA_WITH_SHA256,
                parameters: None,
            },
        }
    }

    /// The longest signature this key produces, to size the placeholder.
    pub(super) fn max_signature_len(&self) -> usize {
        match &self.key {
            Key::Rsa(k) => k.public_modulus_len(),
            Key::Token(_, TokenKind::Rsa(n)) => *n,
            // A DER SEQUENCE of two INTEGERs of up to 33 bytes each.
            Key::P256(_) | Key::Token(_, TokenKind::P256) => 72,
        }
    }

    /// Signs `message` with SHA-256: PKCS#1 v1.5 for RSA, DER-encoded ECDSA
    /// for P-256.
    pub(super) fn sign(&self, message: &[u8]) -> Result<Vec<u8>> {
        sign_with(&self.key, message).map_err(|message| Error::Engine {
            engine: "signer",
            message,
        })
    }
}

fn sign_with(key: &Key, message: &[u8]) -> std::result::Result<Vec<u8>, String> {
    let rng = SystemRandom::new();
    let failed = |_| "the private key operation failed".to_string();
    match key {
        Key::Rsa(k) => {
            let mut signature = vec![0; k.public_modulus_len()];
            k.sign(&RSA_PKCS1_SHA256, &rng, message, &mut signature)
                .map_err(failed)?;
            Ok(signature)
        }
        Key::P256(k) => Ok(k.sign(&rng, message).map_err(failed)?.as_ref().to_vec()),
        Key::Token(t, TokenKind::Rsa(_)) => {
            t.sign(&cryptoki::mechanism::Mechanism::Sha256RsaPkcs, message)
        }
        Key::Token(t, TokenKind::P256) => {
            // CKM_ECDSA signs a digest and returns r || s.
            let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, message);
            let raw = t.sign(&cryptoki::mechanism::Mechanism::Ecdsa, digest.as_ref())?;
            if raw.len() != 64 {
                return Err(format!(
                    "the PKCS#11 token returned a {}-byte ECDSA signature, not 64",
                    raw.len()
                ));
            }
            let (r, s) = raw.split_at(32);
            EcdsaSignature {
                r: der::asn1::UintRef::new(r).map_err(|e| e.to_string())?,
                s: der::asn1::UintRef::new(s).map_err(|e| e.to_string())?,
            }
            .to_der()
            .map_err(|e| e.to_string())
        }
    }
}

fn read_limited(path: &Path, what: &str) -> std::result::Result<Vec<u8>, String> {
    use std::io::Read as _;
    let file = std::fs::File::open(path)
        .map_err(|e| format!("cannot read the {what} at {}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_PEM_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read the {what} at {}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_PEM_BYTES {
        wipe(&mut bytes);
        return Err(format!(
            "the {what} at {} is larger than {MAX_PEM_BYTES} bytes; it should be a PEM file",
            path.display()
        ));
    }
    Ok(bytes)
}

/// Overwrites a buffer that held key material before it is freed, so the
/// bytes do not linger in memory the allocator hands out again.
fn wipe(bytes: &mut [u8]) {
    for b in bytes.iter_mut() {
        // Volatile, so the writes are not optimised away as dead stores.
        // SAFETY: `b` is a valid, aligned, exclusive reference to one byte.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
}

#[cfg(unix)]
fn readable_by_others(path: &Path) -> Option<String> {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::metadata(path).ok()?.permissions().mode();
    (mode & 0o077 != 0).then(|| {
        format!(
            "the signing key at {} is readable by other users (mode {:o}); make it 0600 or 0400",
            path.display(),
            mode & 0o777
        )
    })
}

#[cfg(not(unix))]
fn readable_by_others(_path: &Path) -> Option<String> {
    None
}

/// The PEM blocks of a file, as (label, DER). Text around the blocks, such as
/// the `Bag Attributes` OpenSSL writes, is skipped.
fn pem_blocks(text: &str) -> std::result::Result<Vec<(String, Vec<u8>)>, String> {
    let mut blocks = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("-----BEGIN ") {
        let after = &rest[start + "-----BEGIN ".len()..];
        let Some(label_end) = after.find("-----") else {
            break;
        };
        let label = after[..label_end].to_string();
        let end_marker = format!("-----END {label}-----");
        let Some(end) = rest[start..].find(&end_marker) else {
            return Err(format!("the PEM block '{label}' has no END line"));
        };
        let block = &rest[start..start + end + end_marker.len()];
        // PKCS#8 says so in its label, OpenSSL's legacy format in a header.
        if label.contains("ENCRYPTED") || block.contains("Proc-Type:") {
            return Err(
                "the signing key is encrypted; store it decrypted (openssl pkcs8 -topk8 -nocrypt) and protect the file instead"
                    .into(),
            );
        }
        let (_, der) = der::pem::decode_vec(block.as_bytes())
            .map_err(|e| format!("the PEM block '{label}' cannot be decoded: {e}"))?;
        blocks.push((label, der));
        rest = &rest[start + end + end_marker.len()..];
    }
    Ok(blocks)
}

/// The certificates of a PEM file, in order. `Certificate::load_pem_chain` is
/// not used: it panics on an empty file and refuses text between blocks.
pub(super) fn parse_chain(pem: &[u8]) -> std::result::Result<Vec<Certificate>, String> {
    let text =
        std::str::from_utf8(pem).map_err(|_| "the signing certificate is not a PEM text file")?;
    pem_blocks(text)?
        .into_iter()
        .filter(|(label, _)| label == "CERTIFICATE")
        .map(|(_, der)| {
            Certificate::from_der(&der).map_err(|e| {
                format!("a certificate in the signing certificate file is not X.509: {e}")
            })
        })
        .collect()
}

fn parse_key(pem: &[u8]) -> std::result::Result<Key, String> {
    let text = std::str::from_utf8(pem).map_err(|_| "the signing key is not a PEM text file")?;
    let mut blocks = pem_blocks(text)?;
    let mut found = None;
    for (label, der) in &blocks {
        let key = match label.as_str() {
            "PRIVATE KEY" => match RsaKeyPair::from_pkcs8(der) {
                Ok(k) => Ok(Key::Rsa(k)),
                Err(rsa) => EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, der)
                    .map(Key::P256)
                    .map_err(|ec| {
                        format!(
                            "the PKCS#8 signing key is neither a usable RSA key ({rsa}) nor a P-256 key ({ec})"
                        )
                    }),
            },
            "RSA PRIVATE KEY" => RsaKeyPair::from_der(der)
                .map(Key::Rsa)
                .map_err(|e| format!("the RSA signing key is not usable: {e}")),
            "EC PRIVATE KEY" => {
                EcdsaKeyPair::from_private_key_der(&ECDSA_P256_SHA256_ASN1_SIGNING, der)
                    .map(Key::P256)
                    .map_err(|e| {
                        format!("the EC signing key is not usable (only P-256 is supported): {e}")
                    })
            }
            _ => continue,
        };
        found = Some(key);
        break;
    }
    for (_, der) in blocks.iter_mut() {
        wipe(der);
    }
    found.unwrap_or_else(|| {
        Err(
            "the signing key file holds no PRIVATE KEY, RSA PRIVATE KEY or EC PRIVATE KEY block"
                .into(),
        )
    })
}

fn check_key_matches(
    key: &Key,
    cert: &Certificate,
    subject: &str,
) -> std::result::Result<(), String> {
    let spki = &cert.tbs_certificate.subject_public_key_info;
    let (algorithm_matches, public) = match key {
        Key::Rsa(k) => (
            spki.algorithm.oid == RSA_ENCRYPTION,
            k.public_key().as_ref(),
        ),
        Key::Token(..) => return Ok(()),
        Key::P256(k) => (
            spki.algorithm.oid == EC_PUBLIC_KEY
                && spki
                    .algorithm
                    .parameters
                    .as_ref()
                    .and_then(|p| p.decode_as::<ObjectIdentifier>().ok())
                    == Some(SECP256R1),
            k.public_key().as_ref(),
        ),
    };
    if !algorithm_matches || spki.subject_public_key.raw_bytes() != public {
        return Err(format!(
            "the signing key does not belong to the first certificate in the file ({subject})"
        ));
    }
    Ok(())
}

fn warnings(chain: &[Certificate], now: SystemTime) -> Vec<String> {
    let mut warnings = Vec::new();
    let tbs = &chain[0].tbs_certificate;
    let usage = tbs
        .extensions
        .iter()
        .flatten()
        .find(|e| e.extn_id == KEY_USAGE)
        .and_then(|e| KeyUsage::from_der(e.extn_value.as_bytes()).ok());
    // No key usage extension at all allows every use.
    if let Some(usage) = usage
        && !usage.digital_signature()
        && !usage.non_repudiation()
    {
        warnings.push(format!(
            "the signing certificate ({}) allows neither digitalSignature nor nonRepudiation, so validators may refuse its signatures",
            tbs.subject
        ));
    }
    if tbs.subject == tbs.issuer {
        warnings.push(format!(
            "the signing certificate ({}) is self-signed: validators will find signatures intact but the signer untrusted",
            tbs.subject
        ));
    }
    for pair in chain.windows(2) {
        if pair[0].tbs_certificate.issuer != pair[1].tbs_certificate.subject {
            warnings.push(format!(
                "the certificate after {} is not its issuer; list the chain in order, signer first",
                pair[0].tbs_certificate.subject
            ));
            break;
        }
    }
    for cert in &chain[1..] {
        let not_after = UNIX_EPOCH + cert.tbs_certificate.validity.not_after.to_unix_duration();
        if now > not_after {
            warnings.push(format!(
                "the chain certificate {} expired on {}",
                cert.tbs_certificate.subject,
                timestamp(not_after)
            ));
        }
    }
    warnings
}

/// A time as `2026-11-07T12:00:00Z`.
pub(super) fn timestamp(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs as i64, 0)
        .map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| format!("{secs} seconds after 1970"))
}

/// The DER of a certificate.
pub(super) fn cert_der(cert: &Certificate) -> Result<Vec<u8>> {
    cert.to_der().map_err(|e| Error::Engine {
        engine: "signer",
        message: format!("cannot encode the certificate: {e}"),
    })
}
