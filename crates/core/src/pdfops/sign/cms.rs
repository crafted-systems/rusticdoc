//! The detached CMS `SignedData` a PAdES signature carries in `/Contents`.
//!
//! Signed attributes are content-type, message-digest and ESS
//! signing-certificate-v2. There is no signing-time attribute: ETSI EN 319
//! 142-1 (PAdES baseline) forbids it for `/ETSI.CAdES.detached`, and the
//! claimed time goes in the signature dictionary's `/M` instead.

use cms::cert::{CertificateChoices, IssuerAndSerialNumber};
use cms::content_info::{CmsVersion, ContentInfo};
use cms::signed_data::{
    CertificateSet, EncapsulatedContentInfo, SignedData, SignerIdentifier, SignerInfo, SignerInfos,
};
use der::asn1::{Any, ObjectIdentifier, OctetString, SetOfVec};
use der::{Encode, Sequence};
use sha2::{Digest, Sha256};
use x509_cert::attr::Attribute;
use x509_cert::ext::pkix::name::GeneralName;
use x509_cert::name::Name;
use x509_cert::serial_number::SerialNumber;
use x509_cert::spki::AlgorithmIdentifierOwned;

use super::keys::{Signer, cert_der};
use crate::error::{Error, Result};

pub(super) const ID_DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.1");
pub(super) const ID_SIGNED_DATA: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.2");
pub(super) const ID_SHA256: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.1");
pub(super) const ID_CONTENT_TYPE: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.3");
pub(super) const ID_MESSAGE_DIGEST: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.4");
pub(super) const ID_SIGNING_CERTIFICATE_V2: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.2.47");

/// RFC 5035 `SigningCertificateV2`, without the optional policies.
#[derive(Sequence)]
struct SigningCertificateV2 {
    certs: Vec<EssCertIdV2>,
}

/// RFC 5035 `ESSCertIDv2`. `hashAlgorithm` defaults to SHA-256, and DER
/// leaves a default value out.
#[derive(Sequence)]
struct EssCertIdV2 {
    cert_hash: OctetString,
    issuer_serial: IssuerSerial,
}

/// RFC 5035 `IssuerSerial`.
#[derive(Sequence)]
struct IssuerSerial {
    issuer: Vec<GeneralName>,
    serial_number: SerialNumber,
}

fn encoding(e: der::Error) -> Error {
    Error::Engine {
        engine: "signer",
        message: format!("cannot encode the CMS signature: {e}"),
    }
}

fn sha256() -> AlgorithmIdentifierOwned {
    // RFC 5754: the parameters of SHA-256 are absent.
    AlgorithmIdentifierOwned {
        oid: ID_SHA256,
        parameters: None,
    }
}

fn attribute(oid: ObjectIdentifier, value: Any) -> Result<Attribute> {
    Ok(Attribute {
        oid,
        values: SetOfVec::try_from(vec![value]).map_err(encoding)?,
    })
}

/// The signed attributes over `digest`, the SHA-256 of the signed byte ranges.
fn signed_attributes(signer: &Signer, digest: &[u8]) -> Result<SetOfVec<Attribute>> {
    let cert = &signer.chain()[0];
    let tbs = &cert.tbs_certificate;
    let signing_certificate = SigningCertificateV2 {
        certs: vec![EssCertIdV2 {
            cert_hash: OctetString::new(Sha256::digest(cert_der(cert)?).to_vec())
                .map_err(encoding)?,
            issuer_serial: IssuerSerial {
                issuer: vec![GeneralName::DirectoryName(Name::clone(&tbs.issuer))],
                serial_number: tbs.serial_number.clone(),
            },
        }],
    };
    let attributes = vec![
        attribute(
            ID_CONTENT_TYPE,
            Any::encode_from(&ID_DATA).map_err(encoding)?,
        )?,
        attribute(
            ID_MESSAGE_DIGEST,
            Any::encode_from(&OctetString::new(digest.to_vec()).map_err(encoding)?)
                .map_err(encoding)?,
        )?,
        attribute(
            ID_SIGNING_CERTIFICATE_V2,
            Any::encode_from(&signing_certificate).map_err(encoding)?,
        )?,
    ];
    SetOfVec::try_from(attributes).map_err(encoding)
}

/// A DER `ContentInfo` holding the detached `SignedData` over `digest`, with
/// the signature `sign` makes over the DER of the signed attributes.
fn build(
    signer: &Signer,
    digest: &[u8],
    sign: impl FnOnce(&[u8]) -> Result<Vec<u8>>,
) -> Result<Vec<u8>> {
    let cert = &signer.chain()[0];
    let signed_attrs = signed_attributes(signer, digest)?;
    // RFC 5652 5.4: the signature covers the attributes encoded as a SET OF,
    // not with the [0] IMPLICIT tag they carry inside SignerInfo.
    let signature = sign(&signed_attrs.to_der().map_err(encoding)?)?;
    let signer_info = SignerInfo {
        version: CmsVersion::V1,
        sid: SignerIdentifier::IssuerAndSerialNumber(IssuerAndSerialNumber {
            issuer: cert.tbs_certificate.issuer.clone(),
            serial_number: cert.tbs_certificate.serial_number.clone(),
        }),
        digest_alg: sha256(),
        signed_attrs: Some(signed_attrs),
        signature_algorithm: signer.signature_algorithm(),
        signature: OctetString::new(signature).map_err(encoding)?,
        unsigned_attrs: None,
    };
    let certificates = signer
        .chain()
        .iter()
        .cloned()
        .map(CertificateChoices::Certificate)
        .collect::<Vec<_>>();
    let signed_data = SignedData {
        version: CmsVersion::V1,
        digest_algorithms: SetOfVec::try_from(vec![sha256()]).map_err(encoding)?,
        encap_content_info: EncapsulatedContentInfo {
            econtent_type: ID_DATA,
            econtent: None,
        },
        certificates: Some(CertificateSet(
            SetOfVec::try_from(certificates).map_err(encoding)?,
        )),
        crls: None,
        signer_infos: SignerInfos(SetOfVec::try_from(vec![signer_info]).map_err(encoding)?),
    };
    ContentInfo {
        content_type: ID_SIGNED_DATA,
        content: Any::encode_from(&signed_data).map_err(encoding)?,
    }
    .to_der()
    .map_err(encoding)
}

/// The signature over `digest`, signed with the key.
pub(super) fn signed_data(signer: &Signer, digest: &[u8; 32]) -> Result<Vec<u8>> {
    build(signer, digest, |message| signer.sign(message))
}

/// The length of the longest `ContentInfo` [`signed_data`] can produce for
/// this signer, without using the key.
pub(super) fn max_len(signer: &Signer) -> Result<usize> {
    let longest = vec![0xff; signer.max_signature_len()];
    Ok(build(signer, &[0; 32], |_| Ok(longest))?.len())
}
