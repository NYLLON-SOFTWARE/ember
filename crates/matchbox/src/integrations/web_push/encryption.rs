//! `WebPush::Encryption.encrypt` (web-push 3.1.0): RFC 8291 message encryption with the
//! RFC 8188 `aes128gcm` content coding, as the gem lays it out: a single record whose size
//! field is the ciphertext length, and a plaintext padded with the 0x02 delimiter and one zero.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes128Gcm, Nonce};
use hkdf::Hkdf;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::{PublicKey, SecretKey};
use sha2::Sha256;

use super::decode64;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EncryptionError {
    /// `ArgumentError`: a blank argument, bad Base64, or a payload over 4096 bytes.
    #[error("{0}")]
    Argument(String),
    /// `OpenSSL::PKey::EC::Point::Error` and friends: the subscription's key isn't a P-256 point.
    #[error("{0}")]
    InvalidKey(String),
}

/// Encrypts `message` for the subscription's `p256dh` key and `auth` secret (both urlsafe
/// Base64), with a fresh server key and salt.
pub fn encrypt(message: &[u8], p256dh: Option<&str>, auth: Option<&str>) -> Result<Vec<u8>, EncryptionError> {
    let server_key = SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
    let salt: [u8; 16] = rand::random();
    encrypt_with(message, p256dh, auth, &server_key, &salt, Layout::GEM)
}

/// How the record is framed: the gem's, or RFC 8291's example (for its test vector).
#[derive(Clone, Copy)]
pub struct Layout {
    /// `None`: the ciphertext length, as the gem writes it.
    pub record_size: Option<u32>,
    pub padding: &'static [u8],
}

impl Layout {
    pub const GEM: Layout = Layout { record_size: None, padding: &[2, 0] };
}

pub fn encrypt_with(
    message: &[u8],
    p256dh: Option<&str>,
    auth: Option<&str>,
    server_key: &SecretKey,
    salt: &[u8; 16],
    layout: Layout,
) -> Result<Vec<u8>, EncryptionError> {
    let blank = |value: Option<&str>| value.is_none_or(str::is_empty);
    if message.is_empty() {
        return Err(EncryptionError::Argument("message cannot be blank".into()));
    }
    if blank(p256dh) {
        return Err(EncryptionError::Argument("p256dh cannot be blank".into()));
    }
    if blank(auth) {
        return Err(EncryptionError::Argument("auth cannot be blank".into()));
    }

    // OpenSSL::BN.new(bytes, 2) drops leading zero bytes before the point is decoded
    let client_public_bytes = strip_leading_zeros(decode64(p256dh.unwrap())?);
    let client_public =
        PublicKey::from_sec1_bytes(&client_public_bytes).map_err(|_| EncryptionError::InvalidKey("invalid encoding".into()))?;
    let auth = decode64(auth.unwrap())?;

    let server_public = server_key.public_key().to_encoded_point(false);
    let shared_secret = p256::ecdh::diffie_hellman(server_key.to_nonzero_scalar(), client_public.as_affine());

    let mut info = b"WebPush: info\0".to_vec();
    info.extend_from_slice(&client_public_bytes);
    info.extend_from_slice(server_public.as_bytes());
    let prk = hkdf(&auth, shared_secret.raw_secret_bytes(), &info, 32);
    let content_encryption_key = hkdf(salt, &prk, b"Content-Encoding: aes128gcm\0", 16);
    let nonce = hkdf(salt, &prk, b"Content-Encoding: nonce\0", 12);

    let mut plaintext = message.to_vec();
    plaintext.extend_from_slice(layout.padding);
    let cipher = Aes128Gcm::new_from_slice(&content_encryption_key).expect("16-byte key");
    let ciphertext = cipher.encrypt(Nonce::from_slice(&nonce), plaintext.as_slice()).expect("AES-GCM encrypts");

    let record_size = ciphertext.len();
    if record_size > 4096 {
        return Err(EncryptionError::Argument("encrypted payload is too big".into()));
    }
    let key_id = server_public.as_bytes();
    let mut out = Vec::with_capacity(16 + 4 + 1 + key_id.len() + ciphertext.len());
    out.extend_from_slice(salt);
    out.extend_from_slice(&layout.record_size.unwrap_or(record_size as u32).to_be_bytes());
    out.push(key_id.len() as u8);
    out.extend_from_slice(key_id);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn hkdf(salt: &[u8], ikm: &[u8], info: &[u8], length: usize) -> Vec<u8> {
    let mut okm = vec![0; length];
    Hkdf::<Sha256>::new(Some(salt), ikm).expand(info, &mut okm).expect("valid HKDF length");
    okm
}

fn strip_leading_zeros(mut bytes: Vec<u8>) -> Vec<u8> {
    let zeros = bytes.iter().take_while(|b| **b == 0).count();
    bytes.drain(..zeros);
    bytes
}

/// What a user agent does with the message (RFC 8291 section 3.4): for the tests.
#[cfg(test)]
pub fn decrypt(body: &[u8], receiver: &SecretKey, auth: &[u8]) -> Option<(u32, Vec<u8>)> {
    let salt = &body[..16];
    let record_size = u32::from_be_bytes(body[16..20].try_into().ok()?);
    let id_len = body[20] as usize;
    let server_public_bytes = &body[21..21 + id_len];
    let ciphertext = &body[21 + id_len..];
    let server_public = PublicKey::from_sec1_bytes(server_public_bytes).ok()?;
    let shared_secret = p256::ecdh::diffie_hellman(receiver.to_nonzero_scalar(), server_public.as_affine());
    let mut info = b"WebPush: info\0".to_vec();
    info.extend_from_slice(receiver.public_key().to_encoded_point(false).as_bytes());
    info.extend_from_slice(server_public_bytes);
    let prk = hkdf(auth, shared_secret.raw_secret_bytes(), &info, 32);
    let key = hkdf(salt, &prk, b"Content-Encoding: aes128gcm\0", 16);
    let nonce = hkdf(salt, &prk, b"Content-Encoding: nonce\0", 12);
    let plaintext = Aes128Gcm::new_from_slice(&key).ok()?.decrypt(Nonce::from_slice(&nonce), ciphertext).ok()?;
    Some((record_size, plaintext))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrations::web_push::encode64_nopad;

    fn b64(s: &str) -> Vec<u8> {
        decode64(s).unwrap()
    }

    // RFC 8291, section 5.
    const PLAINTEXT: &str = "V2hlbiBJIGdyb3cgdXAsIEkgd2FudCB0byBiZSBhIHdhdGVybWVsb24";
    const AS_PRIVATE: &str = "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw";
    const UA_PUBLIC: &str = "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
    const UA_PRIVATE: &str = "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94";
    const SALT: &str = "DGv6ra1nlYgDCS1FRnbzlw";
    const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";
    const MESSAGE: &str = "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN";

    #[test]
    fn matches_the_rfc_8291_test_vector() {
        let server_key = SecretKey::from_slice(&b64(AS_PRIVATE)).unwrap();
        let salt: [u8; 16] = b64(SALT).try_into().unwrap();
        let layout = Layout { record_size: Some(4096), padding: &[2] };
        let body = encrypt_with(&b64(PLAINTEXT), Some(UA_PUBLIC), Some(AUTH), &server_key, &salt, layout).unwrap();
        assert_eq!(encode64_nopad(&body), MESSAGE);

        let receiver = SecretKey::from_slice(&b64(UA_PRIVATE)).unwrap();
        let (record_size, plaintext) = decrypt(&b64(MESSAGE), &receiver, &b64(AUTH)).unwrap();
        assert_eq!(record_size, 4096);
        assert_eq!(plaintext, [b64(PLAINTEXT), vec![2]].concat());
    }

    #[test]
    fn round_trips_with_the_gems_framing() {
        let receiver = SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let p256dh = encode64_nopad(receiver.public_key().to_encoded_point(false).as_bytes());
        let auth: [u8; 16] = rand::random();
        let body = encrypt(b"{\"title\":\"hi\"}", Some(&p256dh), Some(&encode64_nopad(&auth))).unwrap();
        let (record_size, plaintext) = decrypt(&body, &receiver, &auth).unwrap();
        assert_eq!(plaintext, b"{\"title\":\"hi\"}\x02\x00");
        assert_eq!(record_size as usize, body.len() - 86);
    }

    #[test]
    fn decrypts_what_the_gem_encrypted() {
        let expected: serde_json::Value = serde_json::from_str(include_str!("../testdata/web_push_expected.json")).unwrap();
        let receiver = SecretKey::from_slice(&b64(expected["receiver_private_key"].as_str().unwrap())).unwrap();
        let body = b64(expected["ciphertext"].as_str().unwrap());
        let (record_size, plaintext) = decrypt(&body, &receiver, &b64(expected["auth"].as_str().unwrap())).unwrap();
        assert_eq!(record_size as usize, body.len() - 86);
        let message = expected["message"].as_str().unwrap();
        assert_eq!(plaintext, [message.as_bytes(), b"\x02\x00"].concat());
    }

    #[test]
    fn rejects_what_the_gem_rejects() {
        let key = SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let ok = encode64_nopad(key.public_key().to_encoded_point(false).as_bytes());
        assert!(matches!(encrypt(b"m", None, Some("YXV0aA")), Err(EncryptionError::Argument(_))));
        assert!(matches!(encrypt(b"m", Some(&ok), Some("")), Err(EncryptionError::Argument(_))));
        assert!(matches!(encrypt(b"m", Some("not base64!"), Some("YXV0aA")), Err(EncryptionError::Argument(_))));
        assert!(matches!(encrypt(b"m", Some("dGVzdF9rZXk"), Some("YXV0aA")), Err(EncryptionError::InvalidKey(_))));
        assert!(matches!(encrypt(&[b'x'; 4100], Some(&ok), Some("YXV0aA")), Err(EncryptionError::Argument(_))));
    }
}
