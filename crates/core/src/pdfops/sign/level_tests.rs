//! B-T, B-LT, B-LTA and certification, against a real OpenSSL timestamp
//! authority, OCSP responder and CRL: the PKI is made by `test/sign/pki.sh`,
//! and the [`Outbound`] answers each request by running `openssl ts -reply` or
//! `openssl ocsp`, or by serving the CRL and root certificate, with no
//! network. `make sign-check` runs the same against servers on loopback.
//!
//! The results are checked without the signer's code: the CMS signature as in
//! `tests.rs`, the timestamp token's message imprint against the signature
//! value or the byte ranges, and the DSS by reading the catalog back.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use ::lopdf::{Document as Pdf, Object};
use async_trait::async_trait;
use cms::content_info::ContentInfo;
use cms::signed_data::SignedData;
use der::asn1::OctetString;
use der::{Decode, Encode};
use sha2::{Digest, Sha256};
use x509_tsp::TstInfo;

use super::net::{Outbound, OutboundRequest, upstream};
use super::tests::{byte_ranges, contents, ranges_digest, signature_field, verify_revision};
use super::{
    Appearance, DocMdp, Level, Limits, Network, SignOptions, Signer, Signing, Tsa, sign_with,
};
use crate::error::{Error, Result};
use crate::job::Document;
use crate::pdfops::fixtures;

/// The test PKI, or `None` where OpenSSL 3 or bash is missing.
struct Pki {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

impl Pki {
    fn make() -> Option<Self> {
        let version = Command::new("openssl").arg("version").output().ok()?;
        let version = String::from_utf8_lossy(&version.stdout).to_string();
        let major: u32 = version
            .strip_prefix("OpenSSL ")?
            .split('.')
            .next()?
            .parse()
            .ok()?;
        if major < 3 {
            eprintln!("skipped: needs OpenSSL 3 or later, found {version}");
            return None;
        }
        let dir = tempfile::tempdir().unwrap();
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/sign/pki.sh");
        let out = Command::new("bash")
            .arg("-c")
            .arg("source \"$0\" && pki_make \"$1\"")
            .arg(&script)
            .arg(dir.path())
            // The URLs in the certificates; nothing listens there.
            .env("PKI_HTTP_PORT", "8")
            .env("PKI_OCSP_PORT", "9")
            .output()
            .expect("bash runs");
        assert!(
            out.status.success(),
            "pki.sh failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let path = dir.path().join("pki");
        Some(Self { _dir: dir, path })
    }

    fn file(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    fn text(&self, name: &str) -> String {
        std::fs::read_to_string(self.file(name)).unwrap()
    }

    fn signer(&self, name: &str) -> Signer {
        Signer::from_files(
            &self.file(&format!("{name}.key")),
            &self.file(&format!("{name}-chain.pem")),
        )
        .unwrap()
    }

    fn revoke(&self, name: &str) {
        let status = Command::new("openssl")
            .args(["ca", "-config"])
            .arg(self.file("ca.cnf"))
            .arg("-revoke")
            .arg(self.file(&format!("{name}.pem")))
            .output()
            .unwrap();
        assert!(status.status.success(), "{status:?}");
    }
}

/// Answers what the PKI's servers would, by running OpenSSL.
#[derive(Debug)]
struct Openssl {
    pki: PathBuf,
    asked: Mutex<Vec<String>>,
    tsa_down: bool,
}

impl Openssl {
    fn new(pki: &Pki) -> Arc<Self> {
        Arc::new(Self {
            pki: pki.path.clone(),
            asked: Mutex::new(Vec::new()),
            tsa_down: false,
        })
    }

    fn run(&self, args: &[&str], input: &[u8]) -> Result<Vec<u8>> {
        let dir = tempfile::tempdir().unwrap();
        let (inp, out) = (dir.path().join("in.der"), dir.path().join("out.der"));
        std::fs::write(&inp, input).unwrap();
        let mut command = Command::new("openssl");
        for arg in args {
            command.arg(
                arg.replace("{pki}", &self.pki.to_string_lossy())
                    .replace("{in}", &inp.to_string_lossy())
                    .replace("{out}", &out.to_string_lossy()),
            );
        }
        let result = command.output().unwrap();
        if !result.status.success() {
            return Err(upstream(
                "openssl",
                String::from_utf8_lossy(&result.stderr).to_string(),
            ));
        }
        Ok(std::fs::read(out).unwrap())
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

#[async_trait]
impl Outbound for Openssl {
    async fn fetch(&self, request: OutboundRequest) -> Result<Vec<u8>> {
        self.asked.lock().unwrap().push(request.url.clone());
        let path = url::Url::parse(&request.url).unwrap().path().to_string();
        match (path.as_str(), request.content_type, request.body.as_deref()) {
            ("/tsa", Some("application/timestamp-query"), Some(query)) => {
                if self.tsa_down {
                    return Err(upstream(request.purpose, "connection refused"));
                }
                self.run(
                    &[
                        "ts",
                        "-reply",
                        "-config",
                        "{pki}/ca.cnf",
                        "-queryfile",
                        "{in}",
                        "-signer",
                        "{pki}/tsa.pem",
                        "-inkey",
                        "{pki}/tsa.key",
                        "-chain",
                        "{pki}/root.pem",
                        "-out",
                        "{out}",
                    ],
                    query,
                )
            }
            ("/", Some("application/ocsp-request"), Some(query)) => self.run(
                &[
                    "ocsp",
                    "-index",
                    "{pki}/index.txt",
                    "-CA",
                    "{pki}/root.pem",
                    "-rsigner",
                    "{pki}/ocsp.pem",
                    "-rkey",
                    "{pki}/ocsp.key",
                    "-reqin",
                    "{in}",
                    "-respout",
                    "{out}",
                ],
                query,
            ),
            ("/root.crl", None, None) => Ok(std::fs::read(self.pki.join("root.crl")).unwrap()),
            ("/root.der", None, None) => Ok(std::fs::read(self.pki.join("root.der")).unwrap()),
            _ => Err(upstream(request.purpose, "404")),
        }
    }
}

fn tsa() -> Tsa {
    Tsa::new("http://127.0.0.1:8/tsa", Duration::from_secs(10))
}

async fn sign_at_level(
    pki: &Pki,
    outbound: &Openssl,
    original: &[u8],
    options: SignOptions,
) -> Result<Vec<u8>> {
    let tsa = tsa();
    let limits = Limits::default();
    let network = Network {
        tsa: Some(&tsa),
        outbound: Some(outbound),
        limits: &limits,
    };
    let signer = Arc::new(pki.signer("signer"));
    sign_with(
        &Document::new("in.pdf", original.to_vec()),
        signer,
        &options,
        network,
        SystemTime::now(),
        Duration::from_secs(60),
    )
    .await
    .map(|d| d.bytes.to_vec())
}

/// The TSTInfo of a DER timestamp token, which zero padding may follow.
fn tst_info(token: &[u8]) -> TstInfo {
    let mut reader = der::SliceReader::new(token).unwrap();
    let info: ContentInfo = der::Reader::decode(&mut reader).unwrap();
    let signed: SignedData = info.content.decode_as().unwrap();
    let econtent = signed.encap_content_info.econtent.unwrap();
    let octets = OctetString::from_der(&econtent.to_der().unwrap()).unwrap();
    TstInfo::from_der(octets.as_bytes()).unwrap()
}

/// The signature timestamp token in a signer's unsigned attributes.
fn signature_timestamp(signed: &SignedData) -> (Vec<u8>, Vec<u8>) {
    let info = signed.signer_infos.0.get(0).unwrap();
    let attr = info
        .unsigned_attrs
        .as_ref()
        .expect("unsigned attributes")
        .iter()
        .find(|a| a.oid.to_string() == "1.2.840.113549.1.9.16.2.14")
        .expect("a signature timestamp")
        .values
        .get(0)
        .unwrap()
        .to_der()
        .unwrap();
    (attr, info.signature.as_bytes().to_vec())
}

#[tokio::test]
async fn b_t_adds_a_timestamp_over_the_signature_value() {
    let Some(pki) = Pki::make() else { return };
    let outbound = Openssl::new(&pki);
    let original = fixtures::pages(2);
    let signed = sign_at_level(
        &pki,
        &outbound,
        &original,
        SignOptions {
            level: Some(Level::BT),
            ..SignOptions::default()
        },
    )
    .await
    .unwrap();
    let verified = verify_revision(&original, &signed, &pki.text("signer.pem"), 0);
    assert_eq!(
        verified.byte_range[2] + verified.byte_range[3],
        signed.len()
    );
    let (token, value) = signature_timestamp(&verified.signed_data);
    let tst = tst_info(&token);
    assert_eq!(
        tst.message_imprint.hashed_message.as_bytes(),
        &Sha256::digest(&value)[..],
        "the token stamps the signature value"
    );
    assert_eq!(outbound.asked(), ["http://127.0.0.1:8/tsa"]);
}

#[tokio::test]
async fn b_lt_adds_a_dss_with_ocsp_crls_and_vri() {
    let Some(pki) = Pki::make() else { return };
    let outbound = Openssl::new(&pki);
    let original = fixtures::pages(1);
    let signed = sign_at_level(
        &pki,
        &outbound,
        &original,
        SignOptions {
            level: Some(Level::BLt),
            ..SignOptions::default()
        },
    )
    .await
    .unwrap();
    let verified = verify_revision(&original, &signed, &pki.text("signer.pem"), 0);
    assert!(
        verified.byte_range[2] + verified.byte_range[3] < signed.len(),
        "a DSS update follows"
    );

    let pdf = Pdf::load_mem(&signed).unwrap();
    let dss = match pdf.catalog().unwrap().get(b"DSS").unwrap() {
        Object::Reference(id) => pdf.get_dictionary(*id).unwrap().clone(),
        other => panic!("unexpected DSS {other:?}"),
    };
    let streams = |key: &[u8]| -> Vec<Vec<u8>> {
        dss.get(key)
            .map(|a| a.as_array().unwrap().clone())
            .unwrap_or_default()
            .iter()
            .map(|r| {
                let mut s = pdf
                    .get_object(r.as_reference().unwrap())
                    .unwrap()
                    .as_stream()
                    .unwrap()
                    .clone();
                s.decompress().unwrap();
                s.content
            })
            .collect()
    };
    let certs = streams(b"Certs");
    let subjects: Vec<String> = certs
        .iter()
        .map(|c| {
            x509_cert::Certificate::from_der(c)
                .unwrap()
                .tbs_certificate
                .subject
                .to_string()
        })
        .collect();
    for cn in ["test signer", "test root CA", "test TSA", "OCSP responder"] {
        assert!(
            subjects.iter().any(|s| s.contains(cn)),
            "{cn} in {subjects:?}"
        );
    }
    // The signer by OCSP (its certificate names a responder), the TSA by
    // CRL (its certificate names only a distribution point).
    let ocsps = streams(b"OCSPs");
    assert_eq!(ocsps.len(), 1, "one OCSP response");
    assert!(x509_ocsp::OcspResponse::from_der(&ocsps[0]).is_ok());
    let crls = streams(b"CRLs");
    assert_eq!(crls.len(), 1, "one CRL");
    assert!(x509_cert::crl::CertificateList::from_der(&crls[0]).is_ok());

    // VRI is keyed by the SHA-1 of the signature's /Contents.
    let key = hex::encode_upper(aws_lc_rs::digest::digest(
        &aws_lc_rs::digest::SHA1_FOR_LEGACY_USE_ONLY,
        &verified.contents,
    ));
    let vri = dss.get(b"VRI").unwrap().as_dict().unwrap();
    let entry = vri
        .get(key.as_bytes())
        .expect("a VRI entry")
        .as_dict()
        .unwrap();
    assert_eq!(
        entry.get(b"Cert").unwrap().as_array().unwrap().len(),
        certs.len()
    );
    assert_eq!(entry.get(b"OCSP").unwrap().as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn b_lta_adds_a_document_timestamp_over_everything() {
    let Some(pki) = Pki::make() else { return };
    let outbound = Openssl::new(&pki);
    let original = fixtures::pages(1);
    let signed = sign_at_level(
        &pki,
        &outbound,
        &original,
        SignOptions {
            level: Some(Level::BLta),
            ..SignOptions::default()
        },
    )
    .await
    .unwrap();
    verify_revision(&original, &signed, &pki.text("signer.pem"), 0);
    let ranges = byte_ranges(&signed);
    assert_eq!(ranges.len(), 2);
    let br = ranges[1];
    assert_eq!(
        br[2] + br[3],
        signed.len(),
        "the document timestamp covers the file"
    );
    let tst = tst_info(&contents(&signed, br));
    assert_eq!(
        tst.message_imprint.hashed_message.as_bytes(),
        &ranges_digest(&signed, br)[..]
    );
    let (_, sig) = signature_field(&signed, "Signature2");
    assert_eq!(
        sig.get(b"Type").unwrap().as_name().unwrap(),
        b"DocTimeStamp"
    );
    assert_eq!(
        sig.get(b"SubFilter").unwrap().as_name().unwrap(),
        b"ETSI.RFC3161"
    );
    // The DSS sits between the signature and the document timestamp.
    let dss_at = signed.windows(4).rposition(|w| w == b"/VRI").unwrap();
    assert!(dss_at > ranges[0][2] + ranges[0][3] && dss_at < br[1]);
}

#[tokio::test]
async fn a_failing_timestamp_authority_is_an_error_not_an_unstamped_file() {
    let Some(pki) = Pki::make() else { return };
    let outbound = Arc::new(Openssl {
        pki: pki.path.clone(),
        asked: Mutex::new(Vec::new()),
        tsa_down: true,
    });
    let err = sign_at_level(
        &pki,
        &outbound,
        &fixtures::pages(1),
        SignOptions {
            level: Some(Level::BT),
            ..SignOptions::default()
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, Error::Status(502, _)), "{err}");
    assert!(err.to_string().contains("timestamp authority"), "{err}");
}

#[tokio::test]
async fn a_revoked_signer_is_refused_at_b_lt() {
    let Some(pki) = Pki::make() else { return };
    pki.revoke("signer");
    let outbound = Openssl::new(&pki);
    let err = sign_at_level(
        &pki,
        &outbound,
        &fixtures::pages(1),
        SignOptions {
            level: Some(Level::BLt),
            ..SignOptions::default()
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("REVOKED"), "{err}");
}

#[tokio::test]
async fn certification_writes_docmdp_and_blocks_what_it_forbids() {
    let Some(pki) = Pki::make() else { return };
    let signer = pki.signer("signer");
    let original = fixtures::form();
    let certified = super::sign_at(
        &Document::new("form.pdf", original.clone()),
        &signer,
        &SignOptions {
            certify: Some(DocMdp::FormFilling),
            ..SignOptions::default()
        },
        SystemTime::now(),
    )
    .unwrap();
    verify_revision(&original, &certified.bytes, &pki.text("signer.pem"), 0);
    let pdf = Pdf::load_mem(&certified.bytes).unwrap();
    let perms = pdf
        .catalog()
        .unwrap()
        .get(b"Perms")
        .unwrap()
        .as_dict()
        .unwrap();
    let sig_id = perms.get(b"DocMDP").unwrap().as_reference().unwrap();
    let (field, sig) = signature_field(&certified.bytes, "Signature1");
    assert_eq!(field.get(b"V").unwrap().as_reference().unwrap(), sig_id);
    let reference = sig.get(b"Reference").unwrap().as_array().unwrap()[0]
        .as_dict()
        .unwrap()
        .clone();
    assert_eq!(
        reference.get(b"Type").unwrap().as_name().unwrap(),
        b"SigRef"
    );
    assert_eq!(
        reference
            .get(b"TransformMethod")
            .unwrap()
            .as_name()
            .unwrap(),
        b"DocMDP"
    );
    let params = reference
        .get(b"TransformParams")
        .unwrap()
        .as_dict()
        .unwrap();
    assert_eq!(params.get(b"P").unwrap().as_i64().unwrap(), 2);
    assert_eq!(params.get(b"V").unwrap().as_name().unwrap(), b"1.2");

    // A second certification cannot follow; an approval can, under P 2.
    let again = super::sign_at(
        &certified,
        &signer,
        &SignOptions {
            certify: Some(DocMdp::NoChanges),
            ..SignOptions::default()
        },
        SystemTime::now(),
    )
    .unwrap_err();
    assert!(
        again.is_client_error() && again.to_string().contains("first"),
        "{again}"
    );
    super::sign_at(
        &certified,
        &signer,
        &SignOptions::default(),
        SystemTime::now(),
    )
    .unwrap();

    // Under P 1 no approval signature may follow.
    let locked = super::sign_at(
        &Document::new("in.pdf", fixtures::pages(1)),
        &signer,
        &SignOptions {
            certify: Some(DocMdp::NoChanges),
            ..SignOptions::default()
        },
        SystemTime::now(),
    )
    .unwrap();
    let err =
        super::sign_at(&locked, &signer, &SignOptions::default(), SystemTime::now()).unwrap_err();
    assert!(err.to_string().contains("DocMDP permission 1"), "{err}");
}

#[test]
fn text_appearances_name_the_signer_the_date_and_the_reason() {
    let key = super::tests::self_signed(&rcgen::PKCS_ECDSA_P256_SHA256);
    let signer = super::tests::signer(&key);
    let original = fixtures::pages(1);
    let signed = super::sign_at(
        &Document::new("in.pdf", original.clone()),
        &signer,
        &SignOptions {
            reason: Some("Approved".into()),
            appearance: Some(Appearance {
                page: 1,
                x: 72.0,
                y: 72.0,
                width: None,
                image: None,
                name: "text".into(),
            }),
            ..SignOptions::default()
        },
        super::tests::now(),
    )
    .unwrap();
    let (field, _) = signature_field(&signed.bytes, "Signature1");
    let rect: Vec<f32> = field
        .get(b"Rect")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_float().unwrap())
        .collect();
    assert_eq!(rect[..3], [72.0, 72.0, 272.0]);
    assert!(rect[3] > 100.0);
    let pdf = Pdf::load_mem(&signed.bytes).unwrap();
    let ap = field.get(b"AP").unwrap().as_dict().unwrap();
    let form = pdf
        .get_object(ap.get(b"N").unwrap().as_reference().unwrap())
        .unwrap()
        .as_stream()
        .unwrap();
    let content = String::from_utf8_lossy(&form.content).to_string();
    for text in [
        "Digitally signed by rusticdoc test signer",
        "Date: 2026-10-26 07:33:20 UTC",
        "Reason: Approved",
    ] {
        let hex = hex::encode_upper(text.as_bytes());
        assert!(content.contains(&hex), "{text} in {content}");
    }
    assert!(content.contains("/F1 9 Tf"), "{content}");
}

#[test]
fn named_keys_and_levels_are_checked_before_any_work() {
    let key = super::tests::self_signed(&rcgen::PKCS_ECDSA_P256_SHA256);
    let signing = Signing::with_default(super::tests::signer(&key)).with_named(
        "acme",
        super::tests::signer(&key),
        vec!["tenant:acme".into()],
    );
    assert!(signing.check(&SignOptions::default()).is_ok());
    let named = SignOptions {
        key: Some("acme".into()),
        ..SignOptions::default()
    };
    assert!(signing.check(&named).is_ok());
    assert_eq!(
        signing.allow_list(Some("acme")).unwrap(),
        Some(&["tenant:acme".to_string()][..])
    );
    assert_eq!(signing.allow_list(None).unwrap(), None);
    let unknown = SignOptions {
        key: Some("globex".into()),
        ..SignOptions::default()
    };
    assert!(signing.check(&unknown).unwrap_err().is_client_error());
    // B-T needs a timestamp authority, and says which setting.
    let stamped = SignOptions {
        level: Some(Level::BT),
        ..SignOptions::default()
    };
    let err = signing.check(&stamped).unwrap_err();
    assert!(
        matches!(&err, Error::Status(503, m) if m.contains("RUSTICDOC_SIGNING_TSA_URL")),
        "{err}"
    );
    assert!(matches!(
        Signing::default().check(&SignOptions::default()),
        Err(Error::Status(503, _))
    ));
}

#[test]
fn levels_and_permissions_parse() {
    assert_eq!(Level::parse("B-LTA").unwrap(), Level::BLta);
    assert_eq!(Level::parse("pades-b-t").unwrap(), Level::BT);
    assert!(Level::parse("b-x").unwrap_err().is_client_error());
    assert_eq!(DocMdp::parse("3").unwrap(), DocMdp::Annotations);
    assert!(DocMdp::parse("4").unwrap_err().is_client_error());
    let options = SignOptions::from_fields(&|name| match name {
        "level" => Some("b-lt".into()),
        "certify" => Some("1".into()),
        "signingKey" => Some("acme".into()),
        _ => None,
    })
    .unwrap();
    assert_eq!(options.level, Some(Level::BLt));
    assert_eq!(options.certify, Some(DocMdp::NoChanges));
    assert_eq!(options.key.as_deref(), Some("acme"));
    assert!(
        SignOptions::from_fields(&|name| (name == "signingKey").then(|| "../x".into())).is_err()
    );
}
