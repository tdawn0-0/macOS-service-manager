//! CA and leaf certificate material.
//!
//! Ported from the material half of `authority.go` and `proxy.go`. Authorities
//! are ECDSA P-256 self-signed CAs valid for five years; leaves are generated
//! per handshake for one domain and clamped to the CA's validity window.

use std::io::Cursor;

use chrono::{DateTime, Utc};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, PublicKeyData, SerialNumber,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use super::CaError;

/// The exact common name of Devbase's local development authority.
pub const AUTHORITY_NAME: &str = "Devbase Local Development CA";

/// Trust classification reported for one authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustState {
    NotChecked,
    Trusted,
    NotTrusted,
    Unknown,
}

impl TrustState {
    pub fn as_str(self) -> &'static str {
        match self {
            TrustState::NotChecked => "Not checked",
            TrustState::Trusted => "Trusted",
            TrustState::NotTrusted => "Not trusted",
            TrustState::Unknown => "Unknown",
        }
    }
}

/// The generation metadata stored beside each authority's key material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorityRecord {
    pub schema: i64,
    pub id: String,
    pub kind: String,
    pub created_at: DateTime<Utc>,
    pub pair_digest: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub base_digest: String,
}

impl Default for AuthorityRecord {
    fn default() -> Self {
        AuthorityRecord {
            schema: 1,
            id: String::new(),
            kind: String::new(),
            created_at: Utc::now(),
            pair_digest: String::new(),
            base_digest: String::new(),
        }
    }
}

/// The observable description of one stored authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorityInfo {
    pub id: String,
    pub name: String,
    pub status: String,
    pub detail: String,
    pub fingerprint: String,
    pub trust: TrustState,
    pub certificate_path: std::path::PathBuf,
    pub not_before: i64,
    pub not_after: i64,
    pub usable: bool,
}

impl Default for AuthorityInfo {
    fn default() -> Self {
        AuthorityInfo {
            id: String::new(),
            name: String::new(),
            status: "Invalid".into(),
            detail: String::new(),
            fingerprint: String::new(),
            trust: TrustState::NotChecked,
            certificate_path: std::path::PathBuf::new(),
            not_before: 0,
            not_after: 0,
            usable: false,
        }
    }
}

/// One parsed authority generation.
#[derive(Debug, Clone)]
pub struct AuthorityMaterial {
    pub info: AuthorityInfo,
    pub cert_pem: Vec<u8>,
    pub key_pem: Vec<u8>,
    pub digest: String,
    pub record: AuthorityRecord,
    /// Trusted, verified DER of the authority certificate.
    pub cert_der: Vec<u8>,
    /// Whether the certificate parsed, carries this app's CN, and is a CA.
    pub valid_ca: bool,
}

impl AuthorityMaterial {
    pub fn is_not_created(&self) -> bool {
        self.info.status == "Not created"
    }
}

/// `sha256( sha256(certPEM) + ":" + sha256(keyPEM) )`.
pub fn pair_digest(cert_pem: &[u8], key_pem: &[u8]) -> String {
    let cert_digest = hex::encode(Sha256::digest(cert_pem));
    let key_digest = hex::encode(Sha256::digest(key_pem));
    hex::encode(Sha256::digest(
        format!("{cert_digest}:{key_digest}").as_bytes(),
    ))
}

/// A random 128-bit positive serial number.
pub fn random_serial() -> SerialNumber {
    let raw = rand::random::<u128>();
    SerialNumber::from_slice(&raw.to_be_bytes())
}

/// Generate a fresh authority certificate and key. The secret key is PKCS#8
/// PEM; the certificate is PEM. Both are returned as bytes for atomic storage.
pub fn new_authority(now: DateTime<Utc>) -> Result<(Vec<u8>, Vec<u8>), CaError> {
    let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
        .map_err(|error| CaError::Invalid(error.to_string()))?;
    let mut params = CertificateParams::new(Vec::<String>::new())
        .map_err(|e| CaError::Invalid(e.to_string()))?;
    params
        .distinguished_name
        .push(DnType::CommonName, AUTHORITY_NAME);
    params.serial_number = Some(random_serial());
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.not_before = timestamp(now.timestamp() - 3600)?;
    params.not_after = timestamp(now.timestamp() + 5 * 365 * 24 * 3600)?;
    let cert = params
        .self_signed(&key)
        .map_err(|error| CaError::Invalid(error.to_string()))?;
    Ok((cert.pem().into_bytes(), key.serialize_pem().into_bytes()))
}

fn timestamp(seconds: i64) -> Result<OffsetDateTime, CaError> {
    OffsetDateTime::from_unix_timestamp(seconds)
        .map_err(|error| CaError::Invalid(error.to_string()))
}

/// Parse one stored authority and classify it exactly as the GUI expects.
///
/// `id` is empty for the legacy unmanaged layout.
pub fn parse_authority(
    cert_pem: &[u8],
    key_pem: &[u8],
    id: &str,
    certificate_path: std::path::PathBuf,
    now: i64,
) -> AuthorityMaterial {
    let mut info = AuthorityInfo {
        id: id.to_string(),
        certificate_path,
        ..AuthorityInfo::default()
    };
    let digest = pair_digest(cert_pem, key_pem);
    let mut material = AuthorityMaterial {
        info: AuthorityInfo::default(),
        cert_pem: cert_pem.to_vec(),
        key_pem: key_pem.to_vec(),
        digest: digest.clone(),
        record: AuthorityRecord::default(),
        cert_der: Vec::new(),
        valid_ca: false,
    };
    if cert_pem.is_empty() && key_pem.is_empty() {
        info.status = "Not created".into();
        info.detail = "No local authority is stored.".into();
        material.info = info;
        return material;
    }
    let der = match decode_cert_der(cert_pem) {
        Some(der) => der,
        None => {
            info.detail = "The CA certificate is missing or invalid. Prepare a replacement.".into();
            material.info = info;
            return material;
        }
    };
    let parsed = match x509_parser::parse_x509_certificate(&der) {
        Ok((_, parsed)) => parsed,
        Err(_) => {
            info.detail = "The CA certificate cannot be parsed.".into();
            material.info = info;
            return material;
        }
    };
    let name = common_name(&parsed);
    info.name = visible_text(&name);
    info.fingerprint = hex::encode(Sha256::digest(&der)).to_uppercase();
    info.not_before = parsed.validity().not_before.timestamp();
    info.not_after = parsed.validity().not_after.timestamp();
    material.cert_der = der.clone();

    if name != AUTHORITY_NAME {
        info.detail = "The stored certificate is not this app's development authority. Prepare a replacement.".into();
        material.info = info;
        return material;
    }
    if !parsed.is_ca() || parsed.verify_signature(None).is_err() {
        info.detail = "The stored certificate is not a valid self-signed authority.".into();
        material.info = info;
        return material;
    }
    material.valid_ca = true;
    let key_text = match std::str::from_utf8(key_pem) {
        Ok(text) if !text.trim().is_empty() => text,
        _ => {
            info.detail = "The CA private key is missing or invalid. Prepare a replacement.".into();
            material.info = info;
            return material;
        }
    };
    let key_pair = match KeyPair::from_pem(key_text) {
        Ok(pair) => pair,
        Err(_) => {
            info.detail = "The CA private key cannot be parsed.".into();
            material.info = info;
            return material;
        }
    };
    let public_key = &parsed.public_key().subject_public_key.data;
    if key_pair.der_bytes() != public_key.as_ref() {
        info.detail = "The stored certificate and private key do not match.".into();
        material.info = info;
        return material;
    }
    if now < info.not_before {
        info.status = "Not yet valid".into();
        info.detail = "Check the system clock before replacing the authority.".into();
        material.info = info;
        return material;
    }
    if now >= info.not_after {
        info.status = "Expired".into();
        info.detail = "Prepare, trust and activate a replacement authority.".into();
        material.info = info;
        return material;
    }
    info.usable = true;
    info.status = "Ready".into();
    info.detail = "Certificate and key match and are within their validity period.".into();
    if info.not_after - now < 30 * 24 * 3600 {
        info.status = "Expiring soon".into();
        info.detail =
            "The authority expires within 30 days. Prepare its replacement before expiry.".into();
    }
    material.info = info;
    material
}

/// A leaf certificate and its private key, ready to install in rustls.
pub struct LeafCertificate {
    pub chain: Vec<CertificateDer<'static>>,
    pub key: PrivateKeyDer<'static>,
}

/// Generate a leaf for `domain`, signed by the stored authority.
pub fn new_leaf(
    ca_cert_pem: &[u8],
    ca_key_pem: &[u8],
    domain: &str,
    now: DateTime<Utc>,
) -> Result<LeafCertificate, CaError> {
    let ca_der = decode_cert_der(ca_cert_pem)
        .ok_or_else(|| CaError::Invalid("the CA certificate is not PEM encoded".into()))?;
    let (_, ca) = x509_parser::parse_x509_certificate(&ca_der)
        .map_err(|_| CaError::Invalid("the CA certificate cannot be parsed".into()))?;
    let ca_not_before = ca.validity().not_before.timestamp();
    let ca_not_after = ca.validity().not_after.timestamp();

    let ca_key_text = std::str::from_utf8(ca_key_pem)
        .map_err(|_| CaError::Invalid("the CA private key is not UTF-8".into()))?;
    let ca_key = KeyPair::from_pem(ca_key_text).map_err(|error| {
        CaError::Invalid(format!("the CA private key cannot be parsed: {error}"))
    })?;
    let ca_cert_text = std::str::from_utf8(ca_cert_pem)
        .map_err(|_| CaError::Invalid("the CA certificate is not UTF-8".into()))?;
    let issuer = Issuer::from_ca_cert_pem(ca_cert_text, ca_key).map_err(|error| {
        CaError::Invalid(format!("the CA certificate cannot be reused: {error}"))
    })?;

    let mut params = CertificateParams::new(vec![domain.to_string()])
        .map_err(|error| CaError::Invalid(error.to_string()))?;
    params.distinguished_name.push(DnType::CommonName, domain);
    params.serial_number = Some(random_serial());
    params.is_ca = IsCa::NoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];

    let mut not_before = now.timestamp() - 3600;
    let mut not_after = now.timestamp() + 30 * 24 * 3600;
    if not_before < ca_not_before {
        not_before = ca_not_before;
    }
    if not_after > ca_not_after {
        not_after = ca_not_after;
    }
    params.not_before = timestamp(not_before)?;
    params.not_after = timestamp(not_after)?;

    let leaf_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
        .map_err(|error| CaError::Invalid(error.to_string()))?;
    let cert = params
        .signed_by(&leaf_key, &issuer)
        .map_err(|error| CaError::Invalid(error.to_string()))?;
    let key = PrivatePkcs8KeyDer::from(leaf_key.serialize_der()).into();
    Ok(LeafCertificate {
        chain: vec![cert.der().clone(), ca_der.into()],
        key,
    })
}

/// Derive a rustls signing key from a leaf private key.
pub fn signing_key(
    key: PrivateKeyDer<'static>,
) -> Result<std::sync::Arc<dyn rustls::sign::SigningKey>, CaError> {
    rustls::crypto::aws_lc_rs::sign::any_supported_type(&key)
        .map_err(|error| CaError::Invalid(format!("unusable leaf key: {error}")))
}

/// SHA-1 of the certificate DER, uppercase, for `security delete-certificate`.
pub fn sha1_fingerprint(der: &[u8]) -> String {
    use sha1::Digest as _;
    hex::encode(sha1::Sha1::digest(der)).to_uppercase()
}

fn decode_cert_der(pem: &[u8]) -> Option<Vec<u8>> {
    let mut reader = Cursor::new(pem);
    let certs: Vec<_> = rustls_pemfile::certs(&mut reader)
        .collect::<Result<_, _>>()
        .ok()?;
    certs.first().map(|cert| cert.as_ref().to_vec())
}

fn common_name(cert: &x509_parser::prelude::X509Certificate<'_>) -> String {
    cert.subject()
        .iter_common_name()
        .next()
        .and_then(|name| name.as_str().ok())
        .unwrap_or_default()
        .to_string()
}

fn visible_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len().min(240));
    for character in value.chars() {
        let character = if character.is_control() || character.is_whitespace() {
            ' '
        } else {
            character
        };
        out.push(character);
        if out.len() > 240 {
            break;
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_authority_is_usable_and_matched() {
        let (cert, key) = new_authority(Utc::now()).unwrap();
        let material = parse_authority(
            &cert,
            &key,
            "0123456789abcdef",
            std::path::PathBuf::from("/tmp/ca.pem"),
            Utc::now().timestamp(),
        );
        assert_eq!(material.info.status, "Ready");
        assert!(material.info.usable);
        assert_eq!(material.info.name, AUTHORITY_NAME);
        assert_eq!(material.info.fingerprint.len(), 64);
        assert!(
            material
                .info
                .fingerprint
                .chars()
                .all(|c| c.is_ascii_hexdigit())
        );
        assert_eq!(material.digest, pair_digest(&cert, &key));
        assert!(!String::from_utf8_lossy(&cert).contains("PRIVATE KEY"));
    }

    #[test]
    fn mismatched_or_foreign_material_is_invalid() {
        let (cert, _key) = new_authority(Utc::now()).unwrap();
        let (_other_cert, other_key) = new_authority(Utc::now()).unwrap();
        let mismatch = parse_authority(
            &cert,
            &other_key,
            "0123456789abcdef",
            "ca.pem".into(),
            Utc::now().timestamp(),
        );
        assert_eq!(mismatch.info.status, "Invalid");
        assert!(mismatch.info.detail.contains("do not match"));

        let garbage = parse_authority(
            b"not a certificate",
            &other_key,
            "0123456789abcdef",
            "ca.pem".into(),
            Utc::now().timestamp(),
        );
        assert_eq!(garbage.info.status, "Invalid");
        assert!(!garbage.info.usable);
    }

    #[test]
    fn not_created_is_distinct_from_invalid() {
        let material = parse_authority(b"", b"", "", "ca.pem".into(), 0);
        assert_eq!(material.info.status, "Not created");
        assert_eq!(material.info.detail, "No local authority is stored.");
    }

    #[test]
    fn leaf_is_signed_by_the_authority_and_clamped() {
        let (cert, key) = new_authority(Utc::now()).unwrap();
        let leaf = new_leaf(&cert, &key, "app.devbase.localhost", Utc::now()).unwrap();
        assert_eq!(leaf.chain.len(), 2);
        let (_, leaf_cert) = x509_parser::parse_x509_certificate(leaf.chain[0].as_ref()).unwrap();
        let ca_der = decode_cert_der(&cert).unwrap();
        let ca = x509_parser::parse_x509_certificate(&ca_der).unwrap().1;
        assert_eq!(common_name(&leaf_cert), "app.devbase.localhost");
        assert_eq!(
            leaf_cert
                .subject()
                .iter_common_name()
                .next()
                .unwrap()
                .as_str()
                .unwrap(),
            "app.devbase.localhost"
        );
        assert!(leaf_cert.validity().not_after.timestamp() <= ca.validity().not_after.timestamp());
        assert!(
            leaf_cert.verify_signature(Some(ca.public_key())).is_ok(),
            "leaf signature verifies against the CA"
        );
        assert!(signing_key(leaf.key).is_ok());
    }

    #[test]
    fn sha1_fingerprint_is_uppercase_hex() {
        let fingerprint = sha1_fingerprint(b"hello");
        assert_eq!(fingerprint.len(), 40);
        assert_eq!(fingerprint, fingerprint.to_uppercase());
    }
}
