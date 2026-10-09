//! VAPID identification (RFC 8292) as `WebPush::Request#build_vapid_header` writes it:
//! `vapid t=<ES256 JWT>,k=<public key>`, the JWT carrying `aud`, `exp` (12 hours out) and `sub`.

use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::elliptic_curve::sec1::ToEncodedPoint;

use super::{decode64, encode64_nopad};
use crate::config::Config;

/// `WebPush::Notification#vapid_identification`: the subject and `Rails.configuration.x.vapid`
/// (`VAPID_PUBLIC_KEY`/`VAPID_PRIVATE_KEY`), parsed once at boot so that a bad key turns Web
/// Push off rather than failing (and being blamed on) each subscription.
#[derive(Debug, Clone)]
pub struct VapidConfig {
    subject: String,
    signing_key: SigningKey,
    public_key: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum VapidError {
    #[error("VAPID_PUBLIC_KEY and VAPID_PRIVATE_KEY aren't set")]
    Missing,
    #[error("VAPID_PUBLIC_KEY isn't a Base64 P-256 public key")]
    InvalidPublicKey,
    #[error("VAPID_PRIVATE_KEY isn't a Base64 P-256 private key")]
    InvalidPrivateKey,
    #[error("VAPID_PUBLIC_KEY isn't the public key of VAPID_PRIVATE_KEY")]
    Mismatched,
}

/// `WebPush::Request#expiration`
const EXPIRATION_SECONDS: i64 = 12 * 60 * 60;

impl VapidConfig {
    pub fn from_config(config: &Config) -> Result<Self, VapidError> {
        match (&config.vapid_public_key, &config.vapid_private_key) {
            (Some(public_key), Some(private_key)) => Self::new(&config.vapid_subject, public_key, private_key),
            _ => Err(VapidError::Missing),
        }
    }

    /// `VapidKey.from_keys(public_key, private_key)`: the private scalar signs; the public key is
    /// sent as given, so it must be the private key's.
    pub fn new(subject: &str, public_key: &str, private_key: &str) -> Result<Self, VapidError> {
        let public_key = decode64(public_key).map_err(|_| VapidError::InvalidPublicKey)?;
        let point = p256::PublicKey::from_sec1_bytes(&public_key).map_err(|_| VapidError::InvalidPublicKey)?;
        let private_key = decode64(private_key).map_err(|_| VapidError::InvalidPrivateKey)?;
        if private_key.len() > 32 {
            return Err(VapidError::InvalidPrivateKey);
        }
        let mut scalar = [0u8; 32];
        scalar[32 - private_key.len()..].copy_from_slice(&private_key);
        let signing_key = SigningKey::from_slice(&scalar).map_err(|_| VapidError::InvalidPrivateKey)?;
        if signing_key.verifying_key().to_encoded_point(false) != point.to_encoded_point(false) {
            return Err(VapidError::Mismatched);
        }
        Ok(Self { subject: subject.to_string(), signing_key, public_key })
    }

    /// The `Authorization` header for a push service at `audience` (`scheme://host`).
    pub fn authorization(&self, audience: &str, now: i64) -> String {
        let header = r#"{"typ":"JWT","alg":"ES256"}"#;
        let claims = serde_json::json!({ "aud": audience, "exp": now + EXPIRATION_SECONDS, "sub": self.subject }).to_string();
        let signing_input = format!("{}.{}", encode64_nopad(header.as_bytes()), encode64_nopad(claims.as_bytes()));
        let signature: Signature = self.signing_key.sign(signing_input.as_bytes());
        format!("vapid t={signing_input}.{},k={}", encode64_nopad(&signature.to_bytes()), encode64_nopad(&self.public_key))
    }
}
