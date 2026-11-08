//! Signs with throwaway keys and checks the result independently: the byte
//! ranges are hashed again, the digest compared with the signed attribute, and
//! the CMS signature verified with the certificate's public key.

use std::time::{Duration, SystemTime};

use ::lopdf::{Document as Pdf, Object};
use aws_lc_rs::signature::{
    ECDSA_P256_SHA256_ASN1, RSA_PKCS1_2048_8192_SHA256, UnparsedPublicKey, VerificationAlgorithm,
};
use cms::content_info::ContentInfo;
use cms::signed_data::{SignedData, SignerIdentifier};
use der::asn1::OctetString;
use der::{Decode, Encode, Reader, SliceReader};
use rcgen::{
    CertificateParams, CertifiedIssuer, DistinguishedName, DnType, IsCa, KeyPair, KeyUsagePurpose,
    PKCS_ECDSA_P256_SHA256, PKCS_RSA_SHA256, date_time_ymd,
};
use sha2::{Digest, Sha256};
use x509_cert::Certificate;

use super::cms::{ID_CONTENT_TYPE, ID_DATA, ID_MESSAGE_DIGEST, ID_SIGNING_CERTIFICATE_V2};
use super::{Appearance, SignOptions, Signer, sign_at};
use crate::error::Error;
use crate::job::Document;
use crate::pdfops::fixtures;

/// A throwaway key and a self-signed certificate for it, as PEM.
struct TestKey {
    key_pem: String,
    cert_pem: String,
}

fn params(name: &str) -> CertificateParams {
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, name);
    dn.push(DnType::OrganizationName, "rusticdoc tests");
    params.distinguished_name = dn;
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::ContentCommitment,
    ];
    params.not_before = date_time_ymd(2025, 1, 1);
    params.not_after = date_time_ymd(2035, 1, 1);
    params
}

fn self_signed(alg: &'static rcgen::SignatureAlgorithm) -> TestKey {
    let key = KeyPair::generate_for(alg).unwrap();
    let cert = params("rusticdoc test signer").self_signed(&key).unwrap();
    TestKey {
        key_pem: key.serialize_pem(),
        cert_pem: cert.pem(),
    }
}

fn signer(key: &TestKey) -> Signer {
    Signer::from_pem_at(key.key_pem.as_bytes(), key.cert_pem.as_bytes(), now()).unwrap()
}

fn now() -> SystemTime {
    // 2026-10-26T07:33:20Z, inside every test certificate's validity.
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_793_000_000)
}

/// What the checks found, for the tests to look at further.
struct Verified {
    signed_data: SignedData,
    byte_range: [usize; 4],
}

/// The last `/ByteRange` in the file.
fn byte_range(bytes: &[u8]) -> [usize; 4] {
    let at = bytes
        .windows(11)
        .rposition(|w| w == b"/ByteRange ")
        .expect("a byte range");
    let rest = &bytes[at + 11..];
    let open = rest.iter().position(|&b| b == b'[').unwrap();
    let close = rest.iter().position(|&b| b == b']').unwrap();
    let numbers: Vec<usize> = std::str::from_utf8(&rest[open + 1..close])
        .unwrap()
        .split_ascii_whitespace()
        .map(|n| n.parse().unwrap())
        .collect();
    numbers.try_into().unwrap()
}

/// Checks a signature made over `original` without trusting the signer's
/// code: the original is a prefix, the byte ranges cover the file around
/// `/Contents`, their SHA-256 is the message-digest attribute, and the
/// signature verifies with the public key of `cert_pem`'s first certificate.
fn verify(original: &[u8], signed: &[u8], cert_pem: &str) -> Verified {
    assert!(
        signed.starts_with(original),
        "the original bytes are a prefix"
    );
    let br = byte_range(signed);
    assert_eq!(br[0], 0);
    assert_eq!(br[2] + br[3], signed.len(), "the ranges reach the end");
    assert!(br[1] > original.len(), "the signature is in the update");
    let contents = &signed[br[1]..br[2]];
    assert_eq!(contents[0], b'<');
    assert_eq!(contents[contents.len() - 1], b'>');
    let der = hex::decode(&contents[1..contents.len() - 1]).unwrap();

    // The DER is followed by the placeholder's zero padding.
    let mut reader = SliceReader::new(&der).unwrap();
    let info: ContentInfo = reader.decode().unwrap();
    assert_eq!(info.content_type, super::cms::ID_SIGNED_DATA);
    assert!(
        der[reader.position().try_into().unwrap()..]
            .iter()
            .all(|&b| b == 0)
    );
    let signed_data: SignedData = info.content.decode_as().unwrap();
    assert!(
        signed_data.encap_content_info.econtent.is_none(),
        "detached"
    );
    assert_eq!(signed_data.encap_content_info.econtent_type, ID_DATA);

    let mut hasher = Sha256::new();
    hasher.update(&signed[br[0]..br[0] + br[1]]);
    hasher.update(&signed[br[2]..br[2] + br[3]]);
    let digest = hasher.finalize().to_vec();

    let signer_info = signed_data.signer_infos.0.get(0).unwrap();
    let attrs = signer_info.signed_attrs.as_ref().unwrap();
    let attr = |oid| {
        attrs
            .iter()
            .find(|a| a.oid == oid)
            .unwrap_or_else(|| panic!("no attribute {oid}"))
            .values
            .get(0)
            .unwrap()
            .clone()
    };
    let message_digest: OctetString = attr(ID_MESSAGE_DIGEST).decode_as().unwrap();
    assert_eq!(message_digest.as_bytes(), &digest[..], "message digest");
    let content_type: der::asn1::ObjectIdentifier = attr(ID_CONTENT_TYPE).decode_as().unwrap();
    assert_eq!(content_type, ID_DATA);
    assert!(
        !attrs
            .iter()
            .any(|a| a.oid.to_string() == "1.2.840.113549.1.9.5"),
        "PAdES baseline has no signing-time attribute"
    );

    let cert = Certificate::load_pem_chain(cert_pem.as_bytes()).unwrap()[0].clone();
    let cert_der = cert.to_der().unwrap();
    // ESS signing-certificate-v2 names this certificate by its SHA-256.
    let ess = attr(ID_SIGNING_CERTIFICATE_V2).to_der().unwrap();
    let hash = Sha256::digest(&cert_der);
    assert!(
        ess.windows(32).any(|w| w == &hash[..]),
        "signing-certificate-v2 holds the certificate hash"
    );
    match &signer_info.sid {
        SignerIdentifier::IssuerAndSerialNumber(id) => {
            assert_eq!(id.serial_number, cert.tbs_certificate.serial_number);
            assert_eq!(id.issuer, cert.tbs_certificate.issuer);
        }
        other => panic!("unexpected signer identifier {other:?}"),
    }
    let certificates = signed_data.certificates.as_ref().unwrap();
    assert!(
        certificates.0.iter().any(|c| matches!(
            c, cms::cert::CertificateChoices::Certificate(c) if c.to_der().unwrap() == cert_der
        )),
        "the signer's certificate is included"
    );

    let spki = &cert.tbs_certificate.subject_public_key_info;
    let algorithm: &dyn VerificationAlgorithm = match spki.algorithm.oid.to_string().as_str() {
        "1.2.840.113549.1.1.1" => &RSA_PKCS1_2048_8192_SHA256,
        "1.2.840.10045.2.1" => &ECDSA_P256_SHA256_ASN1,
        other => panic!("unexpected key algorithm {other}"),
    };
    UnparsedPublicKey::new(algorithm, spki.subject_public_key.raw_bytes())
        .verify(&attrs.to_der().unwrap(), signer_info.signature.as_bytes())
        .expect("the signature verifies with the certificate's key");

    Verified {
        signed_data,
        byte_range: br,
    }
}

/// The signature field the update added, and its `/V`.
fn signature_field(signed: &[u8], name: &str) -> (::lopdf::Dictionary, ::lopdf::Dictionary) {
    let pdf = Pdf::load_mem(signed).unwrap();
    let acroform = match pdf.catalog().unwrap().get(b"AcroForm").unwrap() {
        Object::Reference(id) => pdf.get_dictionary(*id).unwrap().clone(),
        Object::Dictionary(d) => d.clone(),
        other => panic!("unexpected AcroForm {other:?}"),
    };
    assert_eq!(acroform.get(b"SigFlags").unwrap().as_i64().unwrap(), 3);
    let fields = acroform.get(b"Fields").unwrap().as_array().unwrap();
    let field = fields
        .iter()
        .map(|f| pdf.get_dictionary(f.as_reference().unwrap()).unwrap())
        .find(|f| f.get(b"T").unwrap().as_str().unwrap() == name.as_bytes())
        .unwrap_or_else(|| panic!("no field {name}"))
        .clone();
    assert_eq!(field.get(b"FT").unwrap().as_name().unwrap(), b"Sig");
    let sig = pdf
        .get_dictionary(field.get(b"V").unwrap().as_reference().unwrap())
        .unwrap()
        .clone();
    (field, sig)
}

#[test]
fn rsa_invisible_signature_verifies_and_keeps_the_original() {
    let key = self_signed(&PKCS_RSA_SHA256);
    let original = fixtures::pages(3);
    let options = SignOptions {
        reason: Some("Approved".into()),
        location: Some("Nairobi".into()),
        contact_info: Some("signer@example.com".into()),
        signer_name: Some("Zoë Example".into()),
        appearance: None,
    };
    let signed = sign_at(
        &Document::new("in.pdf", original.clone()),
        &signer(&key),
        &options,
        now(),
    )
    .unwrap();
    let verified = verify(&original, &signed.bytes, &key.cert_pem);
    assert!(verified.byte_range[1] > original.len());

    let (field, sig) = signature_field(&signed.bytes, "Signature1");
    let rect: Vec<f32> = field
        .get(b"Rect")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_float().unwrap())
        .collect();
    assert_eq!(rect, [0.0; 4], "invisible");
    assert_eq!(
        sig.get(b"Filter").unwrap().as_name().unwrap(),
        b"Adobe.PPKLite"
    );
    assert_eq!(
        sig.get(b"SubFilter").unwrap().as_name().unwrap(),
        b"ETSI.CAdES.detached"
    );
    assert_eq!(sig.get(b"Reason").unwrap().as_str().unwrap(), b"Approved");
    assert_eq!(sig.get(b"Location").unwrap().as_str().unwrap(), b"Nairobi");
    assert_eq!(
        ::lopdf::decode_text_string(sig.get(b"Name").unwrap()).unwrap(),
        "Zoë Example"
    );
    assert_eq!(
        sig.get(b"M").unwrap().as_str().unwrap(),
        b"D:20261026073320Z"
    );
    // The pages are as they were.
    let pdf = Pdf::load_mem(&signed.bytes).unwrap();
    assert_eq!(pdf.get_pages().len(), 3);
}

#[test]
fn ecdsa_visible_signature_uses_the_image_as_its_appearance() {
    let key = self_signed(&PKCS_ECDSA_P256_SHA256);
    let original = fixtures::pages(2);
    let options = SignOptions {
        appearance: Some(Appearance {
            page: 2,
            x: 100.0,
            y: 72.0,
            width: Some(150.0),
            image: fixtures::PNG_DOT.to_vec(),
            name: "sig.png".into(),
        }),
        ..SignOptions::default()
    };
    let signed = sign_at(
        &Document::new("in.pdf", original.clone()),
        &signer(&key),
        &options,
        now(),
    )
    .unwrap();
    verify(&original, &signed.bytes, &key.cert_pem);

    let (field, _) = signature_field(&signed.bytes, "Signature1");
    let rect: Vec<f32> = field
        .get(b"Rect")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_float().unwrap())
        .collect();
    assert_eq!(rect, [100.0, 72.0, 250.0, 222.0]);
    let pdf = Pdf::load_mem(&signed.bytes).unwrap();
    let page2 = pdf.get_pages()[&2];
    assert_eq!(field.get(b"P").unwrap().as_reference().unwrap(), page2);
    let annots = pdf
        .get_dictionary(page2)
        .unwrap()
        .get(b"Annots")
        .unwrap()
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(annots.len(), 1);
    let ap = field
        .get(b"AP")
        .unwrap()
        .as_dict()
        .unwrap()
        .get(b"N")
        .unwrap()
        .as_reference()
        .unwrap();
    let form = pdf.get_object(ap).unwrap().as_stream().unwrap();
    assert!(String::from_utf8_lossy(&form.content).contains("/Img Do"));
}

#[test]
fn documents_with_cross_reference_streams_get_one_too() {
    let key = self_signed(&PKCS_ECDSA_P256_SHA256);
    let mut pdf = Pdf::load_mem(&fixtures::pages(1)).unwrap();
    let mut original = Vec::new();
    pdf.save_modern(&mut original).unwrap();
    let signed = sign_at(
        &Document::new("modern.pdf", original.clone()),
        &signer(&key),
        &SignOptions::default(),
        now(),
    )
    .unwrap();
    verify(&original, &signed.bytes, &key.cert_pem);
    let tail = String::from_utf8_lossy(&signed.bytes[original.len()..]).to_string();
    assert!(
        tail.contains("/Type/XRef") || tail.contains("/Type /XRef"),
        "{tail}"
    );
    assert!(!tail.contains("\nxref\n"));
    signature_field(&signed.bytes, "Signature1");
}

#[test]
fn existing_fields_are_kept_and_a_second_signature_gets_its_own_name() {
    let key = self_signed(&PKCS_RSA_SHA256);
    let original = fixtures::form();
    let once = sign_at(
        &Document::new("form.pdf", original.clone()),
        &signer(&key),
        &SignOptions::default(),
        now(),
    )
    .unwrap();
    verify(&original, &once.bytes, &key.cert_pem);
    let names: Vec<String> = crate::pdfform::fields(&once.bytes)
        .unwrap()
        .into_iter()
        .map(|f| f.name)
        .collect();
    assert!(names.contains(&"name".to_string()), "{names:?}");
    assert!(names.contains(&"agree".to_string()), "{names:?}");

    let twice = sign_at(&once, &signer(&key), &SignOptions::default(), now()).unwrap();
    verify(&once.bytes, &twice.bytes, &key.cert_pem);
    signature_field(&twice.bytes, "Signature1");
    signature_field(&twice.bytes, "Signature2");
}

#[test]
fn chain_certificates_travel_with_the_signature() {
    let ca_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut ca_params = params("rusticdoc test CA");
    ca_params.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    let ca = CertifiedIssuer::self_signed(ca_params, ca_key).unwrap();
    let leaf_key = KeyPair::generate_for(&PKCS_RSA_SHA256).unwrap();
    let leaf = params("rusticdoc test leaf")
        .signed_by(&leaf_key, &ca)
        .unwrap();
    let key = TestKey {
        key_pem: leaf_key.serialize_pem(),
        cert_pem: format!("{}{}", leaf.pem(), ca.pem()),
    };
    let signer = signer(&key);
    assert!(signer.warnings().is_empty(), "{:?}", signer.warnings());
    let original = fixtures::pages(1);
    let signed = sign_at(
        &Document::new("in.pdf", original.clone()),
        &signer,
        &SignOptions::default(),
        now(),
    )
    .unwrap();
    let verified = verify(&original, &signed.bytes, &key.cert_pem);
    assert_eq!(
        verified.signed_data.certificates.unwrap().0.len(),
        2,
        "signer and CA"
    );
}

/// The PKCS#8 wrapper, to take the key inside out of it.
#[derive(der::Sequence)]
struct PrivateKeyInfo {
    version: u8,
    algorithm: x509_cert::spki::AlgorithmIdentifierOwned,
    private_key: OctetString,
}

fn unwrap_pkcs8(pem: &str, label: &str) -> String {
    let (_, der) = der::pem::decode_vec(pem.as_bytes()).unwrap();
    let info = PrivateKeyInfo::from_der(&der).unwrap();
    assert_eq!(info.version, 0);
    der::pem::encode_string(label, der::pem::LineEnding::LF, info.private_key.as_bytes()).unwrap()
}

#[test]
fn keys_load_from_pkcs1_and_sec1_too() {
    let rsa = self_signed(&PKCS_RSA_SHA256);
    let pkcs1 = unwrap_pkcs8(&rsa.key_pem, "RSA PRIVATE KEY");
    let s = Signer::from_pem_at(pkcs1.as_bytes(), rsa.cert_pem.as_bytes(), now()).unwrap();
    assert_eq!(s.algorithm(), "RSA-2048");

    let ec = self_signed(&PKCS_ECDSA_P256_SHA256);
    // OpenSSL writes the curve as its own block before the key.
    let sec1 = format!(
        "-----BEGIN EC PARAMETERS-----\nBggqhkjOPQMBBw==\n-----END EC PARAMETERS-----\n{}",
        unwrap_pkcs8(&ec.key_pem, "EC PRIVATE KEY")
    );
    let s = Signer::from_pem_at(sec1.as_bytes(), ec.cert_pem.as_bytes(), now()).unwrap();
    assert_eq!(s.algorithm(), "ECDSA P-256");
    let original = fixtures::pages(1);
    let signed = sign_at(
        &Document::new("in.pdf", original.clone()),
        &s,
        &SignOptions::default(),
        now(),
    )
    .unwrap();
    verify(&original, &signed.bytes, &ec.cert_pem);
}

#[test]
fn keys_that_do_not_fit_are_refused_and_never_printed() {
    let a = self_signed(&PKCS_ECDSA_P256_SHA256);
    let b = self_signed(&PKCS_ECDSA_P256_SHA256);
    let err = Signer::from_pem_at(a.key_pem.as_bytes(), b.cert_pem.as_bytes(), now()).unwrap_err();
    assert!(err.contains("does not belong"), "{err}");
    assert!(!err.contains("PRIVATE"), "{err}");

    let err = Signer::from_pem_at(b"", a.cert_pem.as_bytes(), now()).unwrap_err();
    assert!(err.contains("no PRIVATE KEY"), "{err}");
    let err = Signer::from_pem_at(a.key_pem.as_bytes(), b"", now()).unwrap_err();
    assert!(err.contains("no certificate"), "{err}");
    let encrypted =
        "-----BEGIN ENCRYPTED PRIVATE KEY-----\nAAAA\n-----END ENCRYPTED PRIVATE KEY-----\n";
    let err = Signer::from_pem_at(encrypted.as_bytes(), a.cert_pem.as_bytes(), now()).unwrap_err();
    assert!(err.contains("encrypted"), "{err}");

    let signer = signer(&a);
    let shown = format!("{signer:?}");
    assert!(shown.contains("rusticdoc test signer"), "{shown}");
    assert!(!shown.contains("PRIVATE") && !shown.contains(&a.key_pem[30..60]));
    assert!(
        signer.warnings().iter().any(|w| w.contains("self-signed")),
        "{:?}",
        signer.warnings()
    );
}

#[test]
fn certificates_outside_their_validity_are_refused() {
    let key = self_signed(&PKCS_ECDSA_P256_SHA256);
    let later = SystemTime::UNIX_EPOCH + Duration::from_secs(2_100_000_000);
    let err =
        Signer::from_pem_at(key.key_pem.as_bytes(), key.cert_pem.as_bytes(), later).unwrap_err();
    assert!(err.contains("expired"), "{err}");

    // One that expires after loading stops signing, with a 503.
    let signer = signer(&key);
    let err = sign_at(
        &Document::new("in.pdf", fixtures::pages(1)),
        &signer,
        &SignOptions::default(),
        later,
    )
    .unwrap_err();
    assert!(matches!(err, Error::Status(503, _)), "{err}");
}

#[test]
fn key_usage_without_signing_is_a_warning() {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut params = params("encipherment only");
    params.key_usages = vec![KeyUsagePurpose::KeyEncipherment];
    let cert = params.self_signed(&key).unwrap();
    let signer =
        Signer::from_pem_at(key.serialize_pem().as_bytes(), cert.pem().as_bytes(), now()).unwrap();
    assert!(
        signer
            .warnings()
            .iter()
            .any(|w| w.contains("digitalSignature")),
        "{:?}",
        signer.warnings()
    );
}

#[test]
fn bad_requests_are_client_errors() {
    let key = self_signed(&PKCS_ECDSA_P256_SHA256);
    let signer = signer(&key);
    let doc = Document::new("in.pdf", fixtures::pages(1));
    let long = SignOptions {
        reason: Some("x".repeat(super::MAX_SIGN_TEXT + 1)),
        ..SignOptions::default()
    };
    let missing_page = SignOptions {
        appearance: Some(Appearance {
            page: 5,
            x: 0.0,
            y: 0.0,
            width: None,
            image: fixtures::PNG_DOT.to_vec(),
            name: "sig.png".into(),
        }),
        ..SignOptions::default()
    };
    let not_an_image = SignOptions {
        appearance: Some(Appearance {
            page: 1,
            x: 0.0,
            y: 0.0,
            width: None,
            image: b"GIF89a".to_vec(),
            name: "sig.gif".into(),
        }),
        ..SignOptions::default()
    };
    for options in [long, missing_page, not_an_image] {
        let err = sign_at(&doc, &signer, &options, now()).unwrap_err();
        assert!(err.is_client_error(), "{err}");
    }
    let err = sign_at(
        &Document::new("in.pdf", b"%PDF-1.7 nothing else".to_vec()),
        &signer,
        &SignOptions::default(),
        now(),
    )
    .unwrap_err();
    assert!(err.is_client_error(), "{err}");
}
