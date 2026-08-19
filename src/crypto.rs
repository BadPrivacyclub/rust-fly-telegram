use std::path::Path;

use anyhow::{Context, Result};
use argon2::Argon2;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};

const VERSION: u8 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;

/// Symmetric master key derived from a user password.
#[derive(Clone)]
pub struct MasterKey([u8; 32]);

impl MasterKey {
    /// Derives the master key from a password and salt.
    pub fn derive(password: &str, salt: &[u8]) -> Result<Self> {
        let mut key = [0_u8; 32];
        Argon2::default()
            .hash_password_into(password.as_bytes(), salt, &mut key)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(Self(key))
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new((&self.0).into())
    }
}

#[derive(Deserialize, Serialize)]
struct EncryptedBlob {
    version: u8,
    kdf: String,
    cipher: String,
    salt: String,
    nonce: String,
    ciphertext: String,
}

/// Encrypts bytes with a password using Argon2id and XChaCha20-Poly1305.
pub fn encrypt_with_password(plain: &[u8], password: &str) -> Result<Vec<u8>> {
    let mut salt = [0_u8; SALT_LEN];
    let mut nonce = [0_u8; NONCE_LEN];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce);

    let key = MasterKey::derive(password, &salt)?;
    let ciphertext = key
        .cipher()
        .encrypt(XNonce::from_slice(&nonce), plain)
        .map_err(|_| anyhow::anyhow!("encryption failed"))?;

    let blob = EncryptedBlob {
        version: VERSION,
        kdf: "argon2id".to_string(),
        cipher: "xchacha20poly1305".to_string(),
        salt: STANDARD.encode(salt),
        nonce: STANDARD.encode(nonce),
        ciphertext: STANDARD.encode(ciphertext),
    };

    serde_json::to_vec_pretty(&blob).context("serializing encrypted blob")
}

/// Generates a fresh Ed25519 key pair.
pub fn generate_keypair() -> (SigningKey, VerifyingKey) {
    let sk = SigningKey::generate(&mut OsRng);
    let vk = sk.verifying_key();
    (sk, vk)
}

/// Signs `payload` with the given signing key; returns raw 64-byte signature.
pub fn sign_bytes(payload: &[u8], key: &SigningKey) -> [u8; 64] {
    key.sign(payload).to_bytes()
}

/// Returns true iff the raw 64-byte `sig` is a valid Ed25519 signature over `payload`.
#[allow(dead_code)]
pub fn verify_bytes(payload: &[u8], sig: &[u8; 64], key: &VerifyingKey) -> bool {
    let sig = Signature::from_bytes(sig);
    key.verify(payload, &sig).is_ok()
}

/// Encrypts and saves an Ed25519 key pair to disk.
///
/// - `enc_path`: private key encrypted via [`encrypt_with_password`]
/// - `pub_path`: verifying key as raw 32 bytes
pub fn save_keypair(
    sk: &SigningKey,
    vk: &VerifyingKey,
    password: &str,
    enc_path: &Path,
    pub_path: &Path,
) -> Result<()> {
    if let Some(parent) = enc_path.parent() {
        std::fs::create_dir_all(parent).context("creating keys directory")?;
    }
    let encrypted = encrypt_with_password(&sk.to_bytes(), password)?;
    std::fs::write(enc_path, &encrypted).with_context(|| format!("writing {enc_path:?}"))?;
    std::fs::write(pub_path, vk.to_bytes()).with_context(|| format!("writing {pub_path:?}"))?;
    Ok(())
}

/// Loads the verifying key from a file containing 32 raw bytes.
/// Returns `None` if the file does not exist.
pub fn load_verifying_key(path: &Path) -> Result<Option<VerifyingKey>> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(path).with_context(|| format!("reading {path:?}"))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("signing.pub must be exactly 32 bytes"))?;
    Ok(Some(VerifyingKey::from_bytes(&arr)?))
}

/// Loads and decrypts the signing key from an encrypted file.
pub fn load_signing_key(path: &Path, password: &str) -> Result<SigningKey> {
    let encrypted = std::fs::read(path).with_context(|| format!("reading {path:?}"))?;
    let raw = decrypt_with_password(&encrypted, password)?;
    let arr: [u8; 32] = raw
        .try_into()
        .map_err(|_| anyhow::anyhow!("signing key must be exactly 32 bytes"))?;
    Ok(SigningKey::from_bytes(&arr))
}

/// Decrypts bytes that were written by [`encrypt_with_password`].
pub fn decrypt_with_password(encrypted: &[u8], password: &str) -> Result<Vec<u8>> {
    let blob: EncryptedBlob =
        serde_json::from_slice(encrypted).context("reading encrypted blob")?;
    if blob.version != VERSION {
        anyhow::bail!("unsupported encrypted file version");
    }

    let salt = STANDARD.decode(blob.salt).context("decoding salt")?;
    let nonce = STANDARD.decode(blob.nonce).context("decoding nonce")?;
    let ciphertext = STANDARD
        .decode(blob.ciphertext)
        .context("decoding ciphertext")?;
    let key = MasterKey::derive(password, &salt)?;

    key.cipher()
        .decrypt(XNonce::from_slice(&nonce), ciphertext.as_ref())
        .map_err(|_| anyhow::anyhow!("invalid master password or corrupted file"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn temp_path(label: &str) -> std::path::PathBuf {
        env::temp_dir().join(format!("fly_telegram_{label}_{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn load_signing_key_decrypts_saved_key() {
        let path = temp_path("signing_key.enc");
        let (expected, _) = generate_keypair();
        let encrypted = encrypt_with_password(&expected.to_bytes(), "correct horse")
            .expect("key should encrypt");
        std::fs::write(&path, encrypted).expect("encrypted key fixture should be written");

        let actual =
            load_signing_key(&path, "correct horse").expect("encrypted signing key should load");

        assert_eq!(actual.to_bytes(), expected.to_bytes());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn load_signing_key_rejects_wrong_password() {
        let path = temp_path("wrong_password.enc");
        let (key, _) = generate_keypair();
        let encrypted =
            encrypt_with_password(&key.to_bytes(), "right").expect("key should encrypt");
        std::fs::write(&path, encrypted).expect("encrypted key fixture should be written");

        let error = load_signing_key(&path, "wrong").expect_err("wrong password should fail");

        assert!(error.to_string().contains("invalid master password"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn load_signing_key_rejects_invalid_key_length() {
        let path = temp_path("short_key.enc");
        let encrypted =
            encrypt_with_password(&[7_u8; 31], "password").expect("short fixture should encrypt");
        std::fs::write(&path, encrypted).expect("encrypted key fixture should be written");

        let error = load_signing_key(&path, "password").expect_err("short key should fail");

        assert!(error.to_string().contains("exactly 32 bytes"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn load_signing_key_reports_missing_file() {
        let path = temp_path("missing_key.enc");

        let error = load_signing_key(&path, "password").expect_err("missing key should fail");

        assert!(error.to_string().contains("reading"));
    }
}
