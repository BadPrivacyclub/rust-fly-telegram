use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::RwLock;

use crate::crypto;

pub struct SessionSecurity {
    plain_path: PathBuf,
    encrypted_path: PathBuf,
    master_password: Arc<RwLock<Option<String>>>,
}

impl SessionSecurity {
    pub fn new(path: impl AsRef<Path>, master_password: Arc<RwLock<Option<String>>>) -> Self {
        let plain_path = path.as_ref().to_path_buf();
        let encrypted_path = encrypted_path(&plain_path);
        Self {
            plain_path,
            encrypted_path,
            master_password,
        }
    }

    pub async fn prepare(&self) -> Result<()> {
        let password = self.master_password.read().await.clone();
        let Some(password) = password.as_deref() else {
            return Ok(());
        };
        if !self.encrypted_path.exists() || self.plain_path.exists() {
            return Ok(());
        }

        let encrypted = tokio::fs::read(&self.encrypted_path).await?;
        let plain = crypto::decrypt_with_password(&encrypted, password)?;
        tokio::fs::write(&self.plain_path, plain).await?;
        Ok(())
    }

    pub async fn seal(&self) -> Result<()> {
        let password = self.master_password.read().await.clone();
        let Some(password) = password.as_deref() else {
            return Ok(());
        };
        if !self.plain_path.exists() {
            return Ok(());
        }

        let plain = tokio::fs::read(&self.plain_path).await?;
        let encrypted = crypto::encrypt_with_password(&plain, password)?;
        let tmp = self.encrypted_path.with_extension("session.enc.tmp");
        tokio::fs::write(&tmp, encrypted).await?;
        tokio::fs::rename(&tmp, &self.encrypted_path).await?;
        let _ = tokio::fs::remove_file(&self.plain_path).await;
        Ok(())
    }
}

fn encrypted_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".enc");
    PathBuf::from(name)
}
