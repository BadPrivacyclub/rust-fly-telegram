use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use serde_json::{Map, Value};
use tokio::sync::RwLock;

use crate::crypto;

/// Persistent JSON key-value store. Thread-safe via RwLock.
pub struct Database {
    path: PathBuf,
    master_password: Arc<RwLock<Option<String>>>,
    data: RwLock<Map<String, Value>>,
}

impl Database {
    /// Loads the database from disk, creating an empty one if the file does not exist.
    #[allow(dead_code)]
    pub async fn load(path: impl AsRef<Path>) -> Result<Self> {
        Self::load_with_password(path, None).await
    }

    /// Loads the database, using encrypted storage when a password is provided.
    #[allow(dead_code)]
    pub async fn load_with_password(
        path: impl AsRef<Path>,
        master_password: Option<String>,
    ) -> Result<Self> {
        let master_password = Arc::new(RwLock::new(master_password));
        Self::load_with_state(path, master_password).await
    }

    /// Loads the database with a shared encryption password state.
    pub async fn load_with_state(
        path: impl AsRef<Path>,
        master_password: Arc<RwLock<Option<String>>>,
    ) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let encrypted_path = encrypted_path(&path);
        let password_guard = master_password.read().await;

        let data = if let Some(password) = password_guard.as_deref() {
            if encrypted_path.exists() {
                let encrypted = tokio::fs::read(&encrypted_path).await?;
                let plain = crypto::decrypt_with_password(&encrypted, password)?;
                serde_json::from_slice::<Map<String, Value>>(&plain)?
            } else if path.exists() {
                let raw = tokio::fs::read_to_string(&path).await?;
                serde_json::from_str::<Map<String, Value>>(&raw)?
            } else {
                Map::new()
            }
        } else if path.exists() {
            let raw = tokio::fs::read_to_string(&path).await?;
            serde_json::from_str::<Map<String, Value>>(&raw)?
        } else {
            Map::new()
        };
        drop(password_guard);

        Ok(Self {
            path,
            master_password,
            data: RwLock::new(data),
        })
    }

    /// Returns a clone of the value at `key`, or `Value::Null` if absent.
    pub async fn get(&self, key: &str) -> Value {
        self.data
            .read()
            .await
            .get(key)
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// Sets `key` to `value` and flushes to disk.
    pub async fn set(&self, key: impl Into<String>, value: Value) -> Result<()> {
        self.data.write().await.insert(key.into(), value);
        self.flush().await
    }

    /// Removes `key` and flushes to disk.
    #[allow(dead_code)]
    pub async fn remove(&self, key: &str) -> Result<()> {
        self.data.write().await.remove(key);
        self.flush().await
    }

    /// Updates the shared master password and rewrites the database.
    pub async fn set_master_password(&self, password: Option<String>) -> Result<()> {
        *self.master_password.write().await = password;
        self.flush().await
    }

    /// Returns true when encrypted storage is active.
    pub async fn encryption_enabled(&self) -> bool {
        self.master_password.read().await.is_some()
    }

    /// Returns the current master password used for encrypted side stores.
    pub async fn master_password(&self) -> Option<String> {
        self.master_password.read().await.clone()
    }

    /// Writes the in-memory map to disk atomically via a temp file + rename.
    ///
    /// A plain `write` leaves a window where a crash produces a truncated file.
    /// Writing to a sibling temp file and renaming replaces the file in one
    /// filesystem operation, so readers always see either the old or the new
    /// complete JSON.
    async fn flush(&self) -> Result<()> {
        let serialized = {
            let guard = self.data.read().await;
            serde_json::to_string_pretty(&*guard)?
        };

        let password = self.master_password.read().await.clone();
        if let Some(password) = password.as_deref() {
            let encrypted = crypto::encrypt_with_password(serialized.as_bytes(), password)?;
            let encrypted_path = encrypted_path(&self.path);
            let tmp = encrypted_path.with_extension("enc.tmp");
            tokio::fs::write(&tmp, encrypted).await?;
            tokio::fs::rename(&tmp, &encrypted_path).await?;
            if self.path.exists() {
                let _ = tokio::fs::remove_file(&self.path).await;
            }
        } else {
            let tmp = self.path.with_extension("json.tmp");
            tokio::fs::write(&tmp, &serialized).await?;
            tokio::fs::rename(&tmp, &self.path).await?;
        }
        Ok(())
    }
}

fn encrypted_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".enc");
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    async fn temp_db() -> (Database, PathBuf) {
        let path = env::temp_dir().join(format!("fly_telegram_test_{}.json", uuid::Uuid::new_v4()));
        let db = Database::load(&path)
            .await
            .expect("test database should be created in the temp directory");
        (db, path)
    }

    #[tokio::test]
    async fn set_and_get_roundtrip() {
        let (db, path) = temp_db().await;
        db.set("key", Value::String("hello".into()))
            .await
            .expect("test value should be persisted");
        assert_eq!(db.get("key").await, Value::String("hello".into()));
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn get_missing_returns_null() {
        let (db, path) = temp_db().await;
        assert_eq!(db.get("nope").await, Value::Null);
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn remove_deletes_key() {
        let (db, path) = temp_db().await;
        db.set("k", Value::Bool(true))
            .await
            .expect("test value should be persisted");
        db.remove("k").await.expect("test value should be removed");
        assert_eq!(db.get("k").await, Value::Null);
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn flush_persists_to_disk() {
        let (db, path) = temp_db().await;
        db.set("x", Value::Number(42.into()))
            .await
            .expect("test value should be persisted");

        // Reload from the same path; data must survive.
        let db2 = Database::load(&path)
            .await
            .expect("test database should reload from the same path");
        assert_eq!(db2.get("x").await, Value::Number(42.into()));
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn flush_uses_temp_file() {
        let (db, path) = temp_db().await;
        db.set("y", Value::Bool(false))
            .await
            .expect("test value should be persisted");

        // The `.json.tmp` sibling must be gone after a successful flush.
        let tmp = path.with_extension("json.tmp");
        assert!(!tmp.exists(), ".json.tmp should not exist after flush");
        let _ = tokio::fs::remove_file(&path).await;
    }
}
