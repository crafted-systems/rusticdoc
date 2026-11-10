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
    Appearance, DocMdp, Level, Limits, Network, SignOptions, Signer, Signing, Tsa, renew_with,
    sign_with,
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
        // The CRL the servers hand out lists it from now on.
        let pem = self.file("root.crl.pem");
        let status = Command::new("openssl")
            .args(["ca", "-gencrl", "-config"])
            .arg(self.file("ca.cnf"))
            .arg("-out")
            .arg(&pem)
            .output()
            .unwrap();
        assert!(status.status.success(), "{status:?}");
        let status = Command::new("openssl")
            .args(["crl", "-outform", "DER", "-in"])
            .arg(&pem)
            .arg("-out")
            .arg(self.file("root.crl"))
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
    /// Who signs OCSP answers: `ocsp`, or `rogue` for forgeries.
    ocsp_signer: &'static str,
    /// The CRL served: `root.crl`, or `rogue.crl` for a forgery.
    crl: &'static str,
}

impl Openssl {
    fn new(pki: &Pki) -> Arc<Self> {
        Self::forging(pki, "ocsp", "root.crl")
    }

    fn forging(pki: &Pki, ocsp_signer: &'static str, crl: &'static str) -> Arc<Self> {
        Arc::new(Self {
            pki: pki.path.clone(),
            asked: Mutex::new(Vec::new()),
            tsa_down: false,
            ocsp_signer,
            crl,
        })
    }

    fn run(&self, args: &[&str], input: &[u8]) -> Result<Vec<u8>> {
        let signer = self.ocsp_signer;
        let dir = tempfile::tempdir().unwrap();
        let (inp, out) = (dir.path().join("in.der"), dir.path().join("out.der"));
        std::fs::write(&inp, input).unwrap();
        let mut command = Command::new("openssl");
        for arg in args {
            command.arg(
                arg.replace("{pki}", &self.pki.to_string_lossy())
                    .replace("{signer}", signer)
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
                    "{pki}/{signer}.pem",
                    "-rkey",
                    "{pki}/{signer}.key",
                    "-nmin",
                    "60",
                    "-reqin",
                    "{in}",
                    "-respout",
                    "{out}",
                ],
                query,
            ),
            ("/root.crl", None, None) => Ok(std::fs::read(self.pki.join(self.crl)).unwrap()),
            ("/root.der", None, None) => Ok(std::fs::read(self.pki.join("root.der")).unwrap()),
            _ => Err(upstream(request.purpose, "404")),
        }
    }
}

/// The test authority, trusting tokens that chain to the PKI's root.
fn tsa(pki: &Pki) -> Tsa {
    Tsa::new("http://127.0.0.1:8/tsa", Duration::from_secs(10))
        .with_anchors_pem(pki.text("root.pem").as_bytes())
        .unwrap()
}

async fn sign_at_level(
    pki: &Pki,
    outbound: &Openssl,
    original: &[u8],
    options: SignOptions,
) -> Result<Vec<u8>> {
    sign_with_tsa(pki, outbound, original, options, tsa(pki)).await
}

async fn sign_with_tsa(
    pki: &Pki,
    outbound: &Openssl,
    original: &[u8],
    options: SignOptions,
    tsa: Tsa,
) -> Result<Vec<u8>> {
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
        tsa_down: true,
        ..Arc::into_inner(Openssl::new(&pki)).unwrap()
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

fn level(level: Level) -> SignOptions {
    SignOptions {
        level: Some(level),
        ..SignOptions::default()
    }
}

fn certificate(pem: &str) -> x509_cert::Certificate {
    x509_cert::Certificate::load_pem_chain(pem.as_bytes()).unwrap()[0].clone()
}

#[tokio::test]
async fn timestamps_are_trusted_only_through_the_configured_certificates() {
    let Some(pki) = Pki::make() else { return };
    let outbound = Openssl::new(&pki);
    let original = fixtures::pages(1);

    // The authority's own certificate, pinned, is enough.
    let pinned = Tsa::new("http://127.0.0.1:8/tsa", Duration::from_secs(10))
        .with_anchors_pem(pki.text("tsa.pem").as_bytes())
        .unwrap();
    sign_with_tsa(&pki, &outbound, &original, level(Level::BT), pinned)
        .await
        .unwrap();

    // A token whose signer chains to nothing configured is refused, with no
    // file: here the anchor is an unrelated root, then the impostor that has
    // the real root's name and another key.
    for anchor in [
        super::tests::self_signed(&rcgen::PKCS_ECDSA_P256_SHA256).cert_pem,
        pki.text("rogue.pem"),
    ] {
        let elsewhere = Tsa::new("http://127.0.0.1:8/tsa", Duration::from_secs(10))
            .with_anchors_pem(anchor.as_bytes())
            .unwrap();
        let err = sign_with_tsa(&pki, &outbound, &original, level(Level::BT), elsewhere)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Status(502, _)), "{err}");
        assert!(err.to_string().contains("not trusted"), "{err}");
    }

    // And with no anchor at all, the service refuses before any work.
    let key = super::tests::self_signed(&rcgen::PKCS_ECDSA_P256_SHA256);
    let signing = Signing::with_default(super::tests::signer(&key))
        .with_tsa(Tsa::new("http://127.0.0.1:8/tsa", Duration::from_secs(10)))
        .with_outbound(outbound.clone());
    let err = signing.check(&level(Level::BT)).unwrap_err();
    assert!(
        err.to_string().contains("RUSTICDOC_SIGNING_TSA_CERT"),
        "{err}"
    );
}

#[test]
fn a_timestamp_signer_needs_the_critical_usage_and_validity() {
    let Some(pki) = Pki::make() else { return };
    let root = certificate(&pki.text("root.pem"));
    let tsa = certificate(&pki.text("tsa.pem"));
    let now = SystemTime::now();
    assert!(super::tsa::check_signer(&tsa, &[], std::slice::from_ref(&root), now, now).is_ok());
    // Outside its validity: the certificates last 30 days.
    let later = now + Duration::from_secs(60 * 86_400);
    let err =
        super::tsa::check_signer(&tsa, &[], std::slice::from_ref(&root), later, later).unwrap_err();
    assert!(err.contains("not valid"), "{err}");
    // Valid at the token's time is not enough: it must be valid now as well.
    let err =
        super::tsa::check_signer(&tsa, &[], std::slice::from_ref(&root), now, later).unwrap_err();
    assert!(err.contains("not valid now"), "{err}");
    // A pinned authority certificate that has expired by now is refused too.
    let err =
        super::tsa::check_signer(&tsa, &[], std::slice::from_ref(&tsa), now, later).unwrap_err();
    assert!(err.contains("not valid now"), "{err}");
    // A signing certificate from the same root has no timestamping usage.
    let signer = certificate(&pki.text("signer.pem"));
    let err =
        super::tsa::check_signer(&signer, &[], std::slice::from_ref(&root), now, now).unwrap_err();
    assert!(err.contains("not for timestamping"), "{err}");
    // A timestamping usage that is not critical does not count either.
    let ca_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca = rcgen::CertifiedIssuer::self_signed(ca_params, ca_key).unwrap();
    let leaf_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut leaf_params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    leaf_params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::TimeStamping];
    let leaf = leaf_params.signed_by(&leaf_key, &ca).unwrap();
    let leaf = certificate(&leaf.pem());
    // rcgen writes the extension non-critical.
    assert!(matches!(
        super::verify::extended_key_usage(&leaf),
        Some((false, _))
    ));
    let err =
        super::tsa::check_signer(&leaf, &[], &[certificate(&ca.pem())], now, now).unwrap_err();
    assert!(err.contains("critical"), "{err}");
}

#[tokio::test]
async fn forged_ocsp_answers_give_way_to_the_crl() {
    let Some(pki) = Pki::make() else { return };
    // OCSP answers signed by the impostor: refused, and the CRL used instead.
    let outbound = Openssl::forging(&pki, "rogue", "root.crl");
    let original = fixtures::pages(1);
    let signed = sign_at_level(&pki, &outbound, &original, level(Level::BLt))
        .await
        .unwrap();
    let pdf = Pdf::load_mem(&signed).unwrap();
    let dss = match pdf.catalog().unwrap().get(b"DSS").unwrap() {
        Object::Reference(id) => pdf.get_dictionary(*id).unwrap().clone(),
        other => panic!("unexpected DSS {other:?}"),
    };
    assert!(dss.get(b"OCSPs").is_err(), "no forged OCSP answer is kept");
    assert_eq!(dss.get(b"CRLs").unwrap().as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn forged_revocation_data_fails_the_request() {
    let Some(pki) = Pki::make() else { return };
    // Both sources forged: no B-LT file.
    let outbound = Openssl::forging(&pki, "rogue", "rogue.crl");
    let err = sign_at_level(&pki, &outbound, &fixtures::pages(1), level(Level::BLt))
        .await
        .unwrap_err();
    let message = err.to_string();
    assert!(matches!(err, Error::Status(502, _)), "{message}");
    assert!(message.contains("response's signature"), "{message}");
    assert!(message.contains("CRL's signature"), "{message}");
}

#[tokio::test]
async fn revocation_data_must_be_current() {
    let Some(pki) = Pki::make() else { return };
    let outbound = Openssl::new(&pki);
    let certs = vec![
        certificate(&pki.text("signer.pem")),
        certificate(&pki.text("root.pem")),
    ];
    let limits = Limits::default();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let collect = |now| super::revocation::collect(&certs, &*outbound, &limits, deadline, now);

    let now = collect(SystemTime::now()).await.unwrap();
    assert_eq!((now.ocsps.len(), now.crls.len()), (1, 0));
    // Two hours on, the OCSP answer (next update in an hour) has expired and
    // the CRL (seven days) has not.
    let later = collect(SystemTime::now() + Duration::from_secs(2 * 3600))
        .await
        .unwrap();
    assert_eq!((later.ocsps.len(), later.crls.len()), (0, 1));
    // Ten days on, neither will do.
    let err = collect(SystemTime::now() + Duration::from_secs(10 * 86_400))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not current"), "{err}");
    // An hour back, both are from the future.
    let err = collect(SystemTime::now() - Duration::from_secs(3600))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("in the future"), "{err}");
}

#[test]
fn a_token_dated_away_from_now_is_refused() {
    let Some(pki) = Pki::make() else { return };
    let outbound = Openssl::new(&pki);
    let digest: [u8; 32] = Sha256::digest(b"stamp me").into();
    let nonce = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x08];
    let query = super::tsa::request(&digest, &nonce).unwrap();
    let response = outbound
        .run(
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
            &query,
        )
        .unwrap();
    let anchors = [certificate(&pki.text("root.pem"))];
    let now = SystemTime::now();
    super::tsa::check(&response, &digest, &nonce, &anchors, now).unwrap();
    // The same token, judged by a clock ten minutes on or back: refused.
    for clock in [
        now + Duration::from_secs(600),
        now - Duration::from_secs(600),
    ] {
        let err = super::tsa::check(&response, &digest, &nonce, &anchors, clock).unwrap_err();
        assert!(matches!(err, Error::Status(502, _)), "{err}");
        assert!(
            err.to_string().contains("from this service's clock"),
            "{err}"
        );
    }
}

#[tokio::test]
async fn an_impostor_with_the_issuers_name_cannot_stand_in_for_it() {
    let Some(pki) = Pki::make() else { return };
    // The signer's chain as configured leaves its root out, and the
    // impostor, which has the root's name and another key, comes first.
    // Its forged OCSP answers and CRL must not be believed, and the real root
    // (fetched from the AIA caIssuers URL) is used instead.
    let outbound = Openssl::forging(&pki, "rogue", "root.crl");
    let certs = vec![
        certificate(&pki.text("rogue.pem")),
        certificate(&pki.text("signer.pem")),
    ];
    let limits = Limits::default();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let found =
        super::revocation::collect(&certs, &*outbound, &limits, deadline, SystemTime::now())
            .await
            .unwrap();
    assert!(found.ocsps.is_empty(), "the forged OCSP answer is refused");
    assert_eq!(
        found.crls.len(),
        1,
        "the real CRL, checked with the real key"
    );
    assert!(
        outbound.asked().iter().any(|u| u.ends_with("/root.der")),
        "the real issuer was fetched: {:?}",
        outbound.asked()
    );
    let root = certificate(&pki.text("root.pem")).to_der().unwrap();
    assert!(found.certs.contains(&root));

    // With only the impostor to go on, nothing it vouches for is used.
    let outbound = Openssl::forging(&pki, "rogue", "rogue.crl");
    let err = super::revocation::collect(&certs, &*outbound, &limits, deadline, SystemTime::now())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Status(502, _)), "{err}");
}

#[test]
fn certificates_an_ocsp_answer_carries_do_not_join_the_pool() {
    let Some(pki) = Pki::make() else { return };
    let outbound = Openssl::new(&pki);
    let signer = certificate(&pki.text("signer.pem"));
    let root = certificate(&pki.text("root.pem"));
    let query = super::revocation::ocsp_request(&signer, &root).unwrap();
    let answer = outbound
        .run(
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
                // A stowaway in the answer's certificates.
                "-rother",
                "{pki}/rogue.pem",
                "-nmin",
                "60",
                "-reqin",
                "{in}",
                "-respout",
                "{out}",
            ],
            &query,
        )
        .unwrap();
    let delegate =
        super::revocation::read_ocsp(&answer, &signer, &root, SystemTime::now()).unwrap();
    let delegate = delegate.expect("the delegated responder");
    assert!(
        delegate
            .tbs_certificate
            .subject
            .to_string()
            .contains("OCSP responder"),
        "only the verified responder comes back"
    );
}

/// A one-page PDF whose AcroForm is a chain of `levels` field nodes, each
/// naming the next `fan` times in its `/Kids`: one bottom node, reached by
/// `fan^levels` paths. With `signed`, the bottom node is a signed signature
/// field.
fn shared_kids_form(levels: usize, fan: usize, signed: bool) -> Vec<u8> {
    use ::lopdf::dictionary;
    let mut pdf = Pdf::load_mem(&fixtures::pages(1)).unwrap();
    let bottom = if signed {
        let sig = pdf.add_object(dictionary! { "Type" => "Sig" });
        pdf.add_object(
            dictionary! { "FT" => "Sig", "T" => Object::string_literal("deep"), "V" => sig },
        )
    } else {
        pdf.add_object(dictionary! { "FT" => "Tx", "T" => Object::string_literal("deep") })
    };
    let mut below = bottom;
    for level in 0..levels {
        below = pdf.add_object(dictionary! {
            "T" => Object::string_literal(format!("n{level}")),
            "Kids" => vec![Object::Reference(below); fan],
        });
    }
    let acroform = pdf.add_object(dictionary! { "Fields" => vec![Object::Reference(below)] });
    let root = pdf.trailer.get(b"Root").unwrap().as_reference().unwrap();
    pdf.get_dictionary_mut(root)
        .unwrap()
        .set("AcroForm", acroform);
    let mut out = Vec::new();
    pdf.save_to(&mut out).unwrap();
    out
}

#[test]
fn certifying_walks_a_shared_kids_form_once_per_node() {
    let key = super::tests::self_signed(&rcgen::PKCS_ECDSA_P256_SHA256);
    let signer = super::tests::signer(&key);
    let certify = SignOptions {
        certify: Some(DocMdp::FormFilling),
        ..SignOptions::default()
    };
    // 40 levels of 8 shared kids: 8^40 paths for a walk that follows them
    // all, which the old recursion (depth cap 32) would never finish.
    let started = std::time::Instant::now();
    let unsigned = shared_kids_form(40, 8, false);
    super::sign_at(
        &Document::new("dag.pdf", unsigned),
        &signer,
        &certify,
        super::tests::now(),
    )
    .unwrap();
    // And it still looks all the way down: a signed field at the bottom of
    // the same graph stops a certification.
    let signed = shared_kids_form(40, 8, true);
    let err = super::sign_at(
        &Document::new("dag.pdf", signed),
        &signer,
        &certify,
        super::tests::now(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("signed already"), "{err}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_form_past_the_node_budget_is_refused_for_certification() {
    use ::lopdf::dictionary;
    let key = super::tests::self_signed(&rcgen::PKCS_ECDSA_P256_SHA256);
    let signer = super::tests::signer(&key);
    let mut pdf = Pdf::load_mem(&fixtures::pages(1)).unwrap();
    let fields: Vec<Object> = (0..=super::incremental::MAX_FIELD_NODES)
        .map(|i| {
            Object::Reference(pdf.add_object(
                dictionary! { "FT" => "Tx", "T" => Object::string_literal(format!("f{i}")) },
            ))
        })
        .collect();
    let acroform = pdf.add_object(dictionary! { "Fields" => fields });
    let root = pdf.trailer.get(b"Root").unwrap().as_reference().unwrap();
    pdf.get_dictionary_mut(root)
        .unwrap()
        .set("AcroForm", acroform);
    let mut bytes = Vec::new();
    pdf.save_to(&mut bytes).unwrap();
    let certify = SignOptions {
        certify: Some(DocMdp::NoChanges),
        ..SignOptions::default()
    };
    let err = super::sign_at(
        &Document::new("big.pdf", bytes.clone()),
        &signer,
        &certify,
        super::tests::now(),
    )
    .unwrap_err();
    assert!(err.is_client_error(), "{err}");
    assert!(err.to_string().contains("more than 10000 fields"), "{err}");
    // An approval signature does not need the walk, and goes ahead.
    super::sign_at(
        &Document::new("big.pdf", bytes),
        &signer,
        &SignOptions::default(),
        super::tests::now(),
    )
    .unwrap();
}

async fn renew(outbound: &Openssl, file: &[u8], tsa: Tsa) -> Result<Vec<u8>> {
    let limits = Limits::default();
    let network = Network {
        tsa: Some(&tsa),
        outbound: Some(outbound),
        limits: &limits,
    };
    renew_with(
        &Document::new("in.pdf", file.to_vec()),
        network,
        SystemTime::now(),
        Duration::from_secs(60),
    )
    .await
    .map(|d| d.bytes.to_vec())
}

/// The keys of the VRI dictionary in the DSS of `file`.
fn vri_keys(file: &[u8]) -> Vec<String> {
    let pdf = Pdf::load_mem(file).unwrap();
    let dss = match pdf.catalog().unwrap().get(b"DSS").unwrap() {
        Object::Reference(id) => pdf.get_dictionary(*id).unwrap().clone(),
        other => panic!("unexpected DSS {other:?}"),
    };
    let vri = match dss.get(b"VRI").unwrap() {
        Object::Reference(id) => pdf.get_dictionary(*id).unwrap().clone(),
        Object::Dictionary(d) => d.clone(),
        other => panic!("unexpected VRI {other:?}"),
    };
    vri.iter()
        .map(|(k, _)| String::from_utf8(k.clone()).unwrap())
        .collect()
}

fn sha1_key(contents: &[u8]) -> String {
    hex::encode_upper(aws_lc_rs::digest::digest(
        &aws_lc_rs::digest::SHA1_FOR_LEGACY_USE_ONLY,
        contents,
    ))
}

#[tokio::test]
async fn renewing_adds_validation_data_and_a_new_document_timestamp() {
    let Some(pki) = Pki::make() else { return };
    let outbound = Openssl::new(&pki);
    let original = fixtures::pages(1);
    let signed = sign_at_level(&pki, &outbound, &original, level(Level::BLta))
        .await
        .unwrap();
    let old = byte_ranges(&signed)[1];
    let old_contents = contents(&signed, old);
    assert!(!vri_keys(&signed).contains(&sha1_key(&old_contents)));

    let renewed = renew(&outbound, &signed, tsa(&pki)).await.unwrap();
    assert!(
        renewed.starts_with(&signed),
        "what was signed is a prefix of the renewed file"
    );
    verify_revision(&original, &renewed, &pki.text("signer.pem"), 0);
    let ranges = byte_ranges(&renewed);
    assert_eq!(ranges.len(), 3);
    assert_eq!(ranges[1], old, "the old timestamp is as it was");
    let br = ranges[2];
    assert_eq!(
        br[2] + br[3],
        renewed.len(),
        "the new timestamp covers the file"
    );
    assert!(br[1] > signed.len(), "and sits in the update");
    let tst = tst_info(&contents(&renewed, br));
    assert_eq!(
        tst.message_imprint.hashed_message.as_bytes(),
        &ranges_digest(&renewed, br)[..]
    );
    let (_, sig) = signature_field(&renewed, "Signature3");
    assert_eq!(
        sig.get(b"Type").unwrap().as_name().unwrap(),
        b"DocTimeStamp"
    );

    // The old timestamp now has validation data of its own, written before
    // the new timestamp so that it is covered by it.
    let keys = vri_keys(&renewed);
    assert!(keys.contains(&sha1_key(&old_contents)), "{keys:?}");
    for key in vri_keys(&signed) {
        assert!(keys.contains(&key), "the entry {key} is kept");
    }
    let vri_at = renewed.windows(4).rposition(|w| w == b"/VRI").unwrap();
    assert!(vri_at > signed.len() && vri_at < br[1]);

    // A renewed file can be renewed again.
    let again = renew(&outbound, &renewed, tsa(&pki)).await.unwrap();
    assert!(again.starts_with(&renewed));
    let ranges = byte_ranges(&again);
    assert_eq!(ranges.len(), 4);
    assert_eq!(ranges[3][2] + ranges[3][3], again.len());
    verify_revision(&original, &again, &pki.text("signer.pem"), 0);
    let keys = vri_keys(&again);
    assert!(
        keys.contains(&sha1_key(&contents(&renewed, br))),
        "{keys:?}"
    );

    // The authority's own certificate, pinned, is enough to renew through.
    let pinned = Tsa::new("http://127.0.0.1:8/tsa", Duration::from_secs(10))
        .with_anchors_pem(pki.text("tsa.pem").as_bytes())
        .unwrap();
    renew(&outbound, &signed, pinned).await.unwrap();
}

#[tokio::test]
async fn only_a_file_that_ends_with_a_trusted_document_timestamp_is_renewed() {
    let Some(pki) = Pki::make() else { return };
    let outbound = Openssl::new(&pki);
    let original = fixtures::pages(1);
    let refused = |result: Result<Vec<u8>>, what: &str| {
        let err = result.unwrap_err();
        assert!(err.is_client_error(), "{err}");
        assert!(err.to_string().contains(what), "{err}");
    };

    // No signature at all, a signature with no timestamp after it, and a
    // signature followed by a DSS.
    refused(
        renew(&outbound, &original, tsa(&pki)).await,
        "no document timestamp covers the whole file",
    );
    let b_b = sign_at_level(&pki, &outbound, &original, level(Level::BB))
        .await
        .unwrap();
    refused(
        renew(&outbound, &b_b, tsa(&pki)).await,
        "it ends with a signature, not a document timestamp",
    );
    let b_lt = sign_at_level(&pki, &outbound, &original, level(Level::BLt))
        .await
        .unwrap();
    refused(
        renew(&outbound, &b_lt, tsa(&pki)).await,
        "no document timestamp covers the whole file",
    );

    let b_lta = sign_at_level(&pki, &outbound, &original, level(Level::BLta))
        .await
        .unwrap();
    // Something written after the timestamp.
    let mut longer = b_lta.clone();
    longer.extend_from_slice(b"\n% a note\n");
    refused(
        renew(&outbound, &longer, tsa(&pki)).await,
        "no document timestamp covers the whole file",
    );
    // A byte changed under it.
    assert!(b_lta.starts_with(b"%PDF-1."));
    let mut changed = b_lta.clone();
    changed[7] = if changed[7] == b'4' { b'5' } else { b'4' };
    refused(
        renew(&outbound, &changed, tsa(&pki)).await,
        "over other bytes than the file's",
    );

    // A timestamp from an authority this service does not trust: an
    // unrelated root, then the impostor with the real root's name.
    let before = outbound.asked().len();
    for anchor in [
        super::tests::self_signed(&rcgen::PKCS_ECDSA_P256_SHA256).cert_pem,
        pki.text("rogue.pem"),
    ] {
        let elsewhere = Tsa::new("http://127.0.0.1:8/tsa", Duration::from_secs(10))
            .with_anchors_pem(anchor.as_bytes())
            .unwrap();
        refused(
            renew(&outbound, &b_lta, elsewhere).await,
            "its document timestamp is not trusted",
        );
    }
    assert_eq!(
        outbound.asked().len(),
        before,
        "nothing is fetched for a timestamp that is not trusted"
    );
}

#[tokio::test]
async fn a_revoked_authority_cannot_be_renewed_through() {
    let Some(pki) = Pki::make() else { return };
    let outbound = Openssl::new(&pki);
    let original = fixtures::pages(1);
    let b_lta = sign_at_level(&pki, &outbound, &original, level(Level::BLta))
        .await
        .unwrap();
    pki.revoke("tsa");
    let err = renew(&outbound, &b_lta, tsa(&pki)).await.unwrap_err();
    assert!(!err.is_client_error(), "{err}");
    assert!(err.to_string().to_lowercase().contains("revoked"), "{err}");
}

#[test]
fn renewal_needs_the_timestamp_authority_and_no_key() {
    let err = Signing::default().check_renewal().unwrap_err();
    assert!(
        matches!(&err, Error::Status(503, m) if m.contains("RUSTICDOC_SIGNING_TSA_URL")),
        "{err}"
    );
    let unanchored =
        Signing::default().with_tsa(Tsa::new("http://127.0.0.1:8/tsa", Duration::from_secs(10)));
    let err = unanchored.check_renewal().unwrap_err();
    assert!(
        err.to_string().contains("RUSTICDOC_SIGNING_TSA_CERT"),
        "{err}"
    );
}
