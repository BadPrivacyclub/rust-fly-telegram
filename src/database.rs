use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::{json, Map, Value};
use tokio::sync::RwLock;
use tracing::warn;

use crate::config::db_key;
use crate::core_settings::key::BOT_TOKEN;
use crate::crypto;

/// Credentials I never write to the database file in the clear. Without a master password the
/// file stays readable JSON, so a copied file or a backup no longer gives these away.
const SEALED_KEYS: &[&str] = &[
    db_key::API_HASH,
    db_key::PHONE,
    db_key::PROXY_URL,
    BOT_TOKEN,
];
/// Module settings hold API keys, so I seal the whole group.
const SEALED_PREFIXES: &[&str] = &["cfg."];
const SEALED_FIELD: &str = "$sealed";

pub struct Database {
    path: PathBuf,
    master_password: Arc<RwLock<Option<String>>>,
    data: RwLock<Map<String, Value>>,
    csv_values: RwLock<HashMap<String, HashSet<String>>>,
    /// The storage key for sealed values and the anti-delete archive.
    storage_key: RwLock<Option<[u8; 32]>>,
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
        let storage_key = read_storage_key(&key_path(&path), password_guard.as_deref()).await?;

        let mut data = if let Some(password) = password_guard.as_deref() {
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
        open_sealed_values(&mut data, storage_key.as_ref());

        let csv_values = build_csv_cache(&data);
        Ok(Self {
            path,
            master_password,
            data: RwLock::new(data),
            csv_values: RwLock::new(csv_values),
            storage_key: RwLock::new(storage_key),
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

    pub async fn keys_with_prefix(&self, prefix: &str) -> Vec<String> {
        let mut keys = self
            .data
            .read()
            .await
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        keys
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
        self.write_storage_key().await?;
        self.flush().await
    }

    /// I return the storage key and create it on first use. With a master password the key
    /// file itself is encrypted, so everything sealed with it needs that password too.
    pub async fn storage_key(&self) -> Result<[u8; 32]> {
        if let Some(key) = *self.storage_key.read().await {
            return Ok(key);
        }
        let mut guard = self.storage_key.write().await;
        if let Some(key) = *guard {
            return Ok(key);
        }
        let key = crypto::random_key();
        *guard = Some(key);
        drop(guard);
        self.write_storage_key().await?;
        Ok(key)
    }

    async fn write_storage_key(&self) -> Result<()> {
        let Some(key) = *self.storage_key.read().await else {
            return Ok(());
        };
        let text = hex(&key);
        let raw = match self.master_password.read().await.as_deref() {
            Some(password) => crypto::encrypt_with_password(text.as_bytes(), password)?,
            None => text.into_bytes(),
        };
        crate::storage::atomic_write(&key_path(&self.path), &raw).await
    }

    pub async fn encryption_enabled(&self) -> bool {
        self.master_password.read().await.is_some()
    }

    pub async fn master_password(&self) -> Option<String> {
        self.master_password.read().await.clone()
    }

    /// I write through a sibling file so readers never see a partial document.
    async fn flush(&self) -> Result<()> {
        let password = self.master_password.read().await.clone();
        if let Some(password) = password.as_deref() {
            let serialized = {
                let guard = self.data.read().await;
                serde_json::to_string_pretty(&*guard)?
            };
            let encrypted = crypto::encrypt_with_password(serialized.as_bytes(), password)?;
            crate::storage::atomic_write(&encrypted_path(&self.path), &encrypted).await?;
            if self.path.exists() {
                let _ = tokio::fs::remove_file(&self.path).await;
            }
        } else {
            let needs_key = self.data.read().await.keys().any(|key| is_sealed_key(key));
            let key = if needs_key {
                Some(self.storage_key().await?)
            } else {
                None
            };
            let serialized = {
                let guard = self.data.read().await;
                serde_json::to_string_pretty(&with_sealed_values(&guard, key.as_ref())?)?
            };
            crate::storage::atomic_write(&self.path, serialized.as_bytes()).await?;
        }
        Ok(())
    }
}

fn is_sealed_key(key: &str) -> bool {
    SEALED_KEYS.contains(&key) || SEALED_PREFIXES.iter().any(|prefix| key.starts_with(prefix))
}

fn seal_value(value: &Value, key: &[u8; 32]) -> Result<Value> {
    let sealed = crypto::seal_bytes(serde_json::to_string(value)?.as_bytes(), key)?;
    Ok(json!({ SEALED_FIELD: STANDARD.encode(sealed) }))
}

fn open_value(value: &Value, key: &[u8; 32]) -> Result<Value> {
    let sealed = value[SEALED_FIELD].as_str().context("not a sealed value")?;
    let plain = crypto::open_bytes(&STANDARD.decode(sealed)?, key)?;
    Ok(serde_json::from_slice(&plain)?)
}

fn is_sealed(value: &Value) -> bool {
    value.get(SEALED_FIELD).is_some_and(Value::is_string)
}

fn with_sealed_values(
    data: &Map<String, Value>,
    key: Option<&[u8; 32]>,
) -> Result<Map<String, Value>> {
    let mut out = Map::with_capacity(data.len());
    for (name, value) in data {
        let stored = match key {
            // A value I could not open stays sealed, so a missing key file loses nothing.
            Some(key) if is_sealed_key(name) && !is_sealed(value) => seal_value(value, key)?,
            _ => value.clone(),
        };
        out.insert(name.clone(), stored);
    }
    Ok(out)
}

fn open_sealed_values(data: &mut Map<String, Value>, key: Option<&[u8; 32]>) {
    let mut unreadable = 0;
    for value in data.values_mut().filter(|value| is_sealed(value)) {
        match key.map(|key| open_value(value, key)) {
            Some(Ok(opened)) => *value = opened,
            _ => unreadable += 1,
        }
    }
    if unreadable > 0 {
        warn!("{unreadable} sealed setting(s) could not be opened; the storage key is missing or different, enter them again");
    }
}

/// I keep the key beside the database, so it stays out of backups, which take named files only.
fn key_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".key");
    PathBuf::from(name)
}

async fn read_storage_key(path: &Path, password: Option<&str>) -> Result<Option<[u8; 32]>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = tokio::fs::read(path).await?;
    let text = if raw.first() == Some(&b'{') {
        let password =
            password.context("the storage key is encrypted; master password is required")?;
        crypto::decrypt_with_password(&raw, password)
            .context("failed to decrypt the storage key")?
    } else {
        raw
    };
    let text = String::from_utf8(text).context("the storage key is damaged")?;
    Ok(Some(
        unhex(text.trim()).context("the storage key is damaged")?,
    ))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 || !text.is_ascii() {
        return None;
    }
    let mut out = [0_u8; 32];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(out)
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
    async fn credentials_are_sealed_on_disk_and_open_on_load() {
        let (db, path) = temp_db().await;
        db.set(BOT_TOKEN, Value::String("123:secret-token".into()))
            .await
            .unwrap();
        db.set("cfg.ai.key", Value::String("sk-module".into()))
            .await
            .unwrap();
        db.set("core.language", Value::String("ru".into()))
            .await
            .unwrap();

        let raw = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(!raw.contains("secret-token") && !raw.contains("sk-module"));
        assert!(
            raw.contains("core.language"),
            "other settings stay readable"
        );

        let reloaded = Database::load(&path).await.unwrap();
        assert_eq!(reloaded.get(BOT_TOKEN).await, "123:secret-token");
        assert_eq!(reloaded.get("cfg.ai.key").await, "sk-module");
        let _ = tokio::fs::remove_file(key_path(&path)).await;
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn a_lost_key_keeps_sealed_values_on_disk() {
        let (db, path) = temp_db().await;
        db.set(BOT_TOKEN, Value::String("123:secret".into()))
            .await
            .unwrap();
        tokio::fs::remove_file(key_path(&path)).await.unwrap();

        let reloaded = Database::load(&path).await.unwrap();
        assert!(reloaded.get(BOT_TOKEN).await.as_str().is_none());
        reloaded.set("core.language", "ru".into()).await.unwrap();
        let raw = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(
            raw.contains(SEALED_FIELD),
            "the value survives for a later key restore"
        );
        let _ = tokio::fs::remove_file(key_path(&path)).await;
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn the_storage_key_is_wrapped_by_the_master_password() {
        let (db, path) = temp_db().await;
        let key = db.storage_key().await.unwrap();
        db.set_master_password(Some("master-pass".into()))
            .await
            .unwrap();
        let raw = tokio::fs::read(key_path(&path)).await.unwrap();
        assert_eq!(raw.first(), Some(&b'{'));
        assert!(Database::load(&path).await.is_err(), "needs the password");

        let opened = Database::load_with_password(&path, Some("master-pass".into()))
            .await
            .unwrap();
        assert_eq!(opened.storage_key().await.unwrap(), key);
        let _ = tokio::fs::remove_file(key_path(&path)).await;
        let _ = tokio::fs::remove_file(encrypted_path(&path)).await;
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
