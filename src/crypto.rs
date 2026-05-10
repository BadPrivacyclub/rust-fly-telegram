use anyhow::{Context, Result};
use argon2::Argon2;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
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
