use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use serde_json::{Map, Value};
use tokio::sync::RwLock;

use crate::crypto;

pub struct Database {
    path: PathBuf,
    master_password: Arc<RwLock<Option<String>>>,
    data: RwLock<Map<String, Value>>,
    csv_values: RwLock<HashMap<String, HashSet<String>>>,
}

impl Database {
    #[allow(dead_code)]
    pub async fn load(path: impl AsRef<Path>) -> Result<Self> {
        Self::load_with_password(path, None).await
    }

    #[allow(dead_code)]
    pub async fn load_with_password(
        path: impl AsRef<Path>,
        master_password: Option<String>,
    ) -> Result<Self> {
        let master_password = Arc::new(RwLock::new(master_password));
        Self::load_with_state(path, master_password).await
    }

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

        let csv_values = build_csv_cache(&data);
        Ok(Self {
            path,
            master_password,
            data: RwLock::new(data),
            csv_values: RwLock::new(csv_values),
        })
    }

    pub async fn get(&self, key: &str) -> Value {
        self.data
            .read()
            .await
            .get(key)
            .cloned()
            .unwrap_or(Value::Null)
    }

    pub async fn set(&self, key: impl Into<String>, value: Value) -> Result<()> {
        let key = key.into();
        let mut data = self.data.write().await;
        let mut csv_values = self.csv_values.write().await;
        update_csv_cache(&mut csv_values, &key, &value);
        data.insert(key, value);
        drop(csv_values);
        drop(data);
        self.flush().await
    }

    pub async fn csv_contains(&self, key: &str, needle: &str) -> bool {
        self.csv_values
            .read()
            .await
            .get(key)
            .is_some_and(|values| values.contains(needle))
    }

    #[allow(dead_code)]
    pub async fn remove(&self, key: &str) -> Result<()> {
        let mut data = self.data.write().await;
        let mut csv_values = self.csv_values.write().await;
        data.remove(key);
        csv_values.remove(key);
        drop(csv_values);
        drop(data);
        self.flush().await
    }

    pub async fn set_master_password(&self, password: Option<String>) -> Result<()> {
        *self.master_password.write().await = password;
        self.flush().await
    }

    pub async fn encryption_enabled(&self) -> bool {
        self.master_password.read().await.is_some()
    }

    pub async fn master_password(&self) -> Option<String> {
        self.master_password.read().await.clone()
    }

    /// Writes through a sibling file so readers cannot observe a partial document.
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

fn build_csv_cache(data: &Map<String, Value>) -> HashMap<String, HashSet<String>> {
    data.iter()
        .filter(|(key, _)| is_cached_csv_key(key))
        .filter_map(|(key, value)| {
            value
                .as_str()
                .map(|value| (key.clone(), parse_csv_values(value)))
        })
        .collect()
}

fn update_csv_cache(cache: &mut HashMap<String, HashSet<String>>, key: &str, value: &Value) {
    if !is_cached_csv_key(key) {
        return;
    }
    if let Some(value) = value.as_str() {
        cache.insert(key.to_string(), parse_csv_values(value));
    } else {
        cache.remove(key);
    }
}

fn is_cached_csv_key(key: &str) -> bool {
    matches!(key, "pmguard.allow" | "pmguard.deny")
}

fn parse_csv_values(value: &str) -> HashSet<String> {
    value
        .split(',')
        .map(str::trim)
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::hint::black_box;
    use std::time::Instant;

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
    async fn csv_cache_tracks_loaded_and_updated_values() {
        let path = env::temp_dir().join(format!(
            "fly_telegram_csv_test_{}.json",
            uuid::Uuid::new_v4()
        ));
        tokio::fs::write(&path, r#"{"pmguard.allow":"1, 2,3"}"#)
            .await
            .expect("CSV fixture should be written");
        let db = Database::load(&path)
            .await
            .expect("CSV fixture should load");

        assert!(db.csv_contains("pmguard.allow", "2").await);
        assert!(!db.csv_contains("pmguard.allow", "4").await);

        db.set("pmguard.allow", Value::String("4, 5".into()))
            .await
            .expect("CSV update should persist");
        assert!(!db.csv_contains("pmguard.allow", "2").await);
        assert!(db.csv_contains("pmguard.allow", "5").await);

        db.remove("pmguard.allow")
            .await
            .expect("CSV removal should persist");
        assert!(!db.csv_contains("pmguard.allow", "5").await);
        let _ = tokio::fs::remove_file(path).await;
    }

    #[test]
    #[ignore = "manual performance benchmark"]
    fn benchmark_csv_membership() {
        let source = (0..1_000)
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let cached = parse_csv_values(&source);
        let needle = "999";

        let split_started = Instant::now();
        for _ in 0..100_000 {
            black_box(
                source
                    .split(',')
                    .map(str::trim)
                    .any(|value| value == needle),
            );
        }
        let split_elapsed = split_started.elapsed();

        let cached_started = Instant::now();
        for _ in 0..100_000 {
            black_box(cached.contains(needle));
        }
        let cached_elapsed = cached_started.elapsed();

        eprintln!(
            "CSV membership 100k iterations: split={split_elapsed:?}, cached={cached_elapsed:?}"
        );
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

        let tmp = path.with_extension("json.tmp");
        assert!(!tmp.exists(), ".json.tmp should not exist after flush");
        let _ = tokio::fs::remove_file(&path).await;
    }
}
