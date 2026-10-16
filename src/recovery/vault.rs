//! The recovery vault. Entries are sealed to an X25519 public key, so I can add passkeys for
//! new accounts without the vault password; opening an entry needs the password, which
//! unlocks the private key.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};

use super::webauthn::StoredCredential;
use crate::crypto;

pub const VAULT_FILE: &str = "data/recovery/vault.json";
const MIN_PASSWORD_CHARS: usize = 8;
const KDF_CONTEXT: &[u8] = b"fly-telegram recovery vault v1";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Vault {
    pub version: u8,
    pub created_at: i64,
    public_key: String,
    sealed_secret: String,
    #[serde(default)]
    pub entries: BTreeMap<String, Entry>,
}

/// What I show in the panel without the password, plus the sealed credential.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub user_id: i64,
    pub name: String,
    pub username: Option<String>,
    pub created_at: i64,
    pub passkey_id: String,
    pub session_file: String,
    /// The authenticator that made the key, e.g. "Fly-Telegram v.0.2.2"; empty for older keys.
    #[serde(default)]
    pub provider: String,
    sealed: SealedBox,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SealedBox {
    ephemeral: String,
    nonce: String,
    ciphertext: String,
}

/// The secret half of an entry: the credential and the phone number I use to find the account.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Secret {
    pub credential: StoredCredential,
    pub phone: Option<String>,
}

impl Vault {
    pub fn path() -> PathBuf {
        PathBuf::from(VAULT_FILE)
    }

    pub async fn load() -> Result<Option<Self>> {
        load_from(&Self::path()).await
    }

    pub async fn create(password: &str, now: i64) -> Result<Self> {
        if password.chars().count() < MIN_PASSWORD_CHARS {
            bail!("the vault password needs at least {MIN_PASSWORD_CHARS} characters");
        }
        if Self::path().exists() {
            bail!("the vault already exists");
        }
        let secret = StaticSecret::random_from_rng(rand::rngs::OsRng);
        let public = PublicKey::from(&secret);
        let sealed_secret = crypto::encrypt_with_password(secret.as_bytes(), password)?;
        let vault = Self {
            version: 1,
            created_at: now,
            public_key: B64.encode(public.as_bytes()),
            sealed_secret: B64.encode(sealed_secret),
            entries: BTreeMap::new(),
        };
        vault.save().await?;
        Ok(vault)
    }

    pub async fn save(&self) -> Result<()> {
        save_to(self, &Self::path()).await
    }

    fn public_key(&self) -> Result<PublicKey> {
        let bytes: [u8; 32] = B64
            .decode(&self.public_key)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("vault public key is damaged"))?;
        Ok(PublicKey::from(bytes))
    }

    fn unlock(&self, password: &str) -> Result<StaticSecret> {
        let sealed = B64.decode(&self.sealed_secret)?;
        let plain = crypto::decrypt_with_password(&sealed, password)
            .map_err(|_| anyhow::anyhow!("wrong vault password"))?;
        let bytes: [u8; 32] = plain
            .try_into()
            .map_err(|_| anyhow::anyhow!("vault key is damaged"))?;
        Ok(StaticSecret::from(bytes))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn put(
        &mut self,
        user_id: i64,
        name: String,
        username: Option<String>,
        passkey_id: String,
        session_file: String,
        secret: &Secret,
        now: i64,
    ) -> Result<()> {
        let sealed = seal(&self.public_key()?, &serde_json::to_vec(secret)?)?;
        self.entries.insert(
            user_id.to_string(),
            Entry {
                user_id,
                name,
                username,
                created_at: now,
                passkey_id,
                session_file,
                provider: super::webauthn::provider_name(),
                sealed,
            },
        );
        Ok(())
    }

    pub fn open(&self, user_id: i64, password: &str) -> Result<Secret> {
        let entry = self
            .entries
            .get(&user_id.to_string())
            .context("this account has no access key in the vault")?;
        let secret = self.unlock(password)?;
        let plain = open(&secret, &entry.sealed)?;
        serde_json::from_slice(&plain).context("vault entry is damaged")
    }

    /// I re-seal an entry after use and keep its public metadata.
    pub fn update_secret(&mut self, user_id: i64, secret: &Secret) -> Result<()> {
        let public = self.public_key()?;
        let entry = self
            .entries
            .get_mut(&user_id.to_string())
            .context("no vault entry to update")?;
        entry.sealed = seal(&public, &serde_json::to_vec(secret)?)?;
        Ok(())
    }

    pub fn remove(&mut self, user_id: i64) -> Option<Entry> {
        self.entries.remove(&user_id.to_string())
    }

    pub fn change_password(&mut self, current: &str, next: &str) -> Result<()> {
        if next.chars().count() < MIN_PASSWORD_CHARS {
            bail!("the vault password needs at least {MIN_PASSWORD_CHARS} characters");
        }
        let secret = self.unlock(current)?;
        self.sealed_secret = B64.encode(crypto::encrypt_with_password(secret.as_bytes(), next)?);
        Ok(())
    }
}

fn box_key(shared: &[u8; 32], ephemeral: &PublicKey, recipient: &PublicKey) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(KDF_CONTEXT);
    hasher.update(shared);
    hasher.update(ephemeral.as_bytes());
    hasher.update(recipient.as_bytes());
    hasher.finalize().into()
}

fn seal(recipient: &PublicKey, plain: &[u8]) -> Result<SealedBox> {
    let ephemeral = StaticSecret::random_from_rng(rand::rngs::OsRng);
    let ephemeral_public = PublicKey::from(&ephemeral);
    let shared = ephemeral.diffie_hellman(recipient);
    let key = box_key(shared.as_bytes(), &ephemeral_public, recipient);
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let cipher = ChaCha20Poly1305::new((&key).into());
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), plain)
        .map_err(|_| anyhow::anyhow!("sealing the vault entry failed"))?;
    Ok(SealedBox {
        ephemeral: B64.encode(ephemeral_public.as_bytes()),
        nonce: B64.encode(nonce),
        ciphertext: B64.encode(ciphertext),
    })
}

fn open(secret: &StaticSecret, sealed: &SealedBox) -> Result<Vec<u8>> {
    let ephemeral: [u8; 32] = B64
        .decode(&sealed.ephemeral)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("vault entry is damaged"))?;
    let ephemeral = PublicKey::from(ephemeral);
    let recipient = PublicKey::from(secret);
    let shared = secret.diffie_hellman(&ephemeral);
    let key = box_key(shared.as_bytes(), &ephemeral, &recipient);
    let nonce = B64.decode(&sealed.nonce)?;
    if nonce.len() != 12 {
        bail!("vault entry is damaged");
    }
    ChaCha20Poly1305::new((&key).into())
        .decrypt(
            Nonce::from_slice(&nonce),
            B64.decode(&sealed.ciphertext)?.as_slice(),
        )
        .map_err(|_| anyhow::anyhow!("vault entry could not be opened"))
}

async fn load_from(path: &Path) -> Result<Option<Vault>> {
    match tokio::fs::read(path).await {
        Ok(raw) => Ok(Some(
            serde_json::from_slice(&raw).context("reading the recovery vault")?,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

async fn save_to(vault: &Vault, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = path.with_extension("json.tmp");
    tokio::fs::write(&tmp, serde_json::to_vec_pretty(vault)?).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credential() -> StoredCredential {
        StoredCredential {
            credential_id: "abc".into(),
            secret: "c2VjcmV0".into(),
            user_handle: "dXNlcg".into(),
            rp_id: "telegram.org".into(),
            origin: "https://telegram.org".into(),
            sign_count: 3,
        }
    }

    fn vault(password: &str) -> Vault {
        let secret = StaticSecret::random_from_rng(rand::rngs::OsRng);
        Vault {
            version: 1,
            created_at: 0,
            public_key: B64.encode(PublicKey::from(&secret).as_bytes()),
            sealed_secret: B64
                .encode(crypto::encrypt_with_password(secret.as_bytes(), password).unwrap()),
            entries: BTreeMap::new(),
        }
    }

    #[test]
    fn entries_seal_without_password_and_open_with_it() {
        let mut vault = vault("correct horse");
        let secret = Secret {
            credential: credential(),
            phone: Some("+100".into()),
        };
        vault
            .put(
                7,
                "Alex".into(),
                None,
                "pk".into(),
                "s.session".into(),
                &secret,
                1,
            )
            .unwrap();
        assert!(vault.open(7, "wrong password").is_err());
        let opened = vault.open(7, "correct horse").unwrap();
        assert_eq!(opened.credential.sign_count, 3);
        assert_eq!(opened.phone.as_deref(), Some("+100"));
        vault
            .change_password("correct horse", "battery staple")
            .unwrap();
        assert!(vault.open(7, "battery staple").is_ok());
    }

    #[tokio::test]
    async fn vault_round_trips_on_disk() {
        let path = std::env::temp_dir().join(format!("fly_vault_{}.json", uuid::Uuid::new_v4()));
        let mut vault = vault("long enough");
        vault
            .put(
                1,
                "A".into(),
                None,
                "pk".into(),
                "a".into(),
                &Secret {
                    credential: credential(),
                    phone: None,
                },
                1,
            )
            .unwrap();
        save_to(&vault, &path).await.unwrap();
        let loaded = load_from(&path).await.unwrap().unwrap();
        assert!(loaded.open(1, "long enough").is_ok());
        let _ = tokio::fs::remove_file(path).await;
    }
}
