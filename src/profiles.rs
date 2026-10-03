//! Desktop device profiles: the device model, OS and app version an account reports to
//! Telegram. Profiles are generated or created by hand and pinned to accounts, so every
//! account keeps one stable "desktop" identity. Nothing is assigned until the feature is
//! switched on, which keeps existing installations unchanged.

use std::collections::BTreeMap;

use anyhow::{bail, Result};
use grammers_mtsender::ConnectionParams;
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::database::Database;

pub const KEY_ENABLED: &str = "device_profiles.enabled";
const KEY_LIST: &str = "device_profiles.list";
const KEY_ASSIGNED: &str = "device_profiles.assigned";
const MAX_FIELD_LEN: usize = 64;

static WRITE_LOCK: Mutex<()> = Mutex::const_new(());

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DeviceProfile {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub device_model: String,
    pub system_version: String,
    pub app_version: String,
    /// Empty keeps the machine's own language.
    #[serde(default)]
    pub system_lang_code: String,
    #[serde(default)]
    pub lang_code: String,
    /// True for profiles made by the generator rather than typed in.
    #[serde(default)]
    pub generated: bool,
}

pub async fn enabled(db: &Database) -> bool {
    db.get(KEY_ENABLED).await.as_bool().unwrap_or(false)
}

pub async fn list(db: &Database) -> Vec<DeviceProfile> {
    serde_json::from_value(db.get(KEY_LIST).await).unwrap_or_default()
}

pub async fn assignments(db: &Database) -> BTreeMap<String, String> {
    serde_json::from_value(db.get(KEY_ASSIGNED).await).unwrap_or_default()
}

pub async fn profile_for(db: &Database, session_file: &str) -> Option<DeviceProfile> {
    let id = assignments(db).await.remove(&key(session_file))?;
    list(db).await.into_iter().find(|profile| profile.id == id)
}

fn key(session_file: &str) -> String {
    session_file.trim().replace('\\', "/")
}

async fn store_list(db: &Database, profiles: &[DeviceProfile]) -> Result<()> {
    db.set(KEY_LIST, serde_json::to_value(profiles)?).await
}

async fn store_assignments(db: &Database, assigned: &BTreeMap<String, String>) -> Result<()> {
    db.set(KEY_ASSIGNED, serde_json::to_value(assigned)?).await
}

/// Profile an account should connect with. A new account gets a generated profile when the
/// feature is on; an existing one only has a profile if the owner assigned it.
pub async fn resolve(
    db: &Database,
    session_file: &str,
    new_session: bool,
) -> Option<DeviceProfile> {
    if let Some(profile) = profile_for(db, session_file).await {
        return Some(profile);
    }
    if new_session && enabled(db).await {
        return assign_generated(db, session_file).await.ok();
    }
    None
}

pub async fn assign_generated(db: &Database, session_file: &str) -> Result<DeviceProfile> {
    let _guard = WRITE_LOCK.lock().await;
    let mut profiles = list(db).await;
    let profile = generate(&profiles);
    profiles.push(profile.clone());
    store_list(db, &profiles).await?;
    let mut assigned = assignments(db).await;
    assigned.insert(key(session_file), profile.id.clone());
    store_assignments(db, &assigned).await?;
    Ok(profile)
}

pub async fn assign(db: &Database, session_file: &str, profile_id: Option<&str>) -> Result<()> {
    let _guard = WRITE_LOCK.lock().await;
    let mut assigned = assignments(db).await;
    match profile_id {
        Some(id) => {
            if !list(db).await.iter().any(|profile| profile.id == id) {
                bail!("unknown profile");
            }
            assigned.insert(key(session_file), id.to_string());
        }
        None => {
            assigned.remove(&key(session_file));
        }
    }
    store_assignments(db, &assigned).await
}

pub async fn save(db: &Database, mut profile: DeviceProfile) -> Result<DeviceProfile> {
    for field in [
        &mut profile.name,
        &mut profile.device_model,
        &mut profile.system_version,
        &mut profile.app_version,
        &mut profile.system_lang_code,
        &mut profile.lang_code,
    ] {
        *field = field.trim().to_string();
        if field.chars().count() > MAX_FIELD_LEN || field.chars().any(char::is_control) {
            bail!("profile fields must be at most {MAX_FIELD_LEN} characters");
        }
    }
    if profile.name.is_empty()
        || profile.device_model.is_empty()
        || profile.system_version.is_empty()
        || profile.app_version.is_empty()
    {
        bail!("name, device, system and app version are required");
    }
    let _guard = WRITE_LOCK.lock().await;
    let mut profiles = list(db).await;
    if profile.id.is_empty() {
        profile.id = new_id();
    }
    match profiles
        .iter_mut()
        .find(|existing| existing.id == profile.id)
    {
        Some(existing) => *existing = profile.clone(),
        None => profiles.push(profile.clone()),
    }
    store_list(db, &profiles).await?;
    Ok(profile)
}

/// Removes a profile and unpins it from every account that used it.
pub async fn delete(db: &Database, id: &str) -> Result<()> {
    let _guard = WRITE_LOCK.lock().await;
    let mut profiles = list(db).await;
    profiles.retain(|profile| profile.id != id);
    store_list(db, &profiles).await?;
    let mut assigned = assignments(db).await;
    assigned.retain(|_, profile_id| profile_id != id);
    store_assignments(db, &assigned).await
}

pub async fn set_enabled(db: &Database, on: bool) -> Result<()> {
    db.set(KEY_ENABLED, Value::Bool(on)).await
}

pub fn apply(params: &mut ConnectionParams, profile: &DeviceProfile) {
    params.device_model = profile.device_model.clone();
    params.system_version = profile.system_version.clone();
    params.app_version = profile.app_version.clone();
    if !profile.system_lang_code.is_empty() {
        params.system_lang_code = profile.system_lang_code.clone();
    }
    if !profile.lang_code.is_empty() {
        params.lang_code = profile.lang_code.clone();
    }
}

fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_string()
}

struct Platform {
    label: &'static str,
    devices: &'static [&'static str],
    systems: &'static [&'static str],
}

const PLATFORMS: &[Platform] = &[
    Platform {
        label: "Windows",
        devices: &[
            "PC 64bit",
            "Dell Inc. XPS 15 9520",
            "Dell Inc. Inspiron 15 3520",
            "LENOVO 82JU",
            "LENOVO 21A1",
            "ASUSTeK COMPUTER INC. ROG Strix G513",
            "ASUSTeK COMPUTER INC. TUF Gaming A15",
            "HP Victus 16-d1000",
            "HP Pavilion 15-eg2000",
            "Micro-Star International MS-7D25",
            "Acer Aspire A515-57",
            "Gigabyte Technology B550 AORUS ELITE",
        ],
        systems: &[
            "Windows 10",
            "Windows 11",
            "Windows 11 23H2",
            "Windows 10 22H2",
        ],
    },
    Platform {
        label: "macOS",
        devices: &[
            "MacBookPro18,3",
            "MacBookPro17,1",
            "MacBookAir10,1",
            "MacBookAir15,12",
            "Macmini9,1",
            "iMac21,1",
            "Mac14,2",
        ],
        systems: &["macOS 13.6", "macOS 14.2", "macOS 14.5", "macOS 15.0"],
    },
    Platform {
        label: "Linux",
        devices: &[
            "PC 64bit",
            "Dell Inc. Latitude 5420",
            "LENOVO 20XW",
            "LENOVO 21CB",
            "System76 Lemur Pro",
            "ASUSTeK COMPUTER INC. ZenBook UX425",
            "Framework Laptop 13",
        ],
        systems: &[
            "Ubuntu 22.04",
            "Ubuntu 24.04",
            "Fedora Linux 40",
            "Debian GNU/Linux 12",
            "Arch Linux",
            "Linux Mint 21.3",
        ],
    },
];

const APP_VERSIONS: &[&str] = &[
    "5.1.7", "5.2.3", "5.3.1", "5.4.1", "5.5.5", "5.6.3", "5.7.2", "5.8.3",
];

fn generate(existing: &[DeviceProfile]) -> DeviceProfile {
    let mut rng = rand::thread_rng();
    // Mostly Windows, as on real desktop installs, with some macOS and Linux.
    let platform = [&PLATFORMS[0], &PLATFORMS[0], &PLATFORMS[1], &PLATFORMS[2]]
        .choose(&mut rng)
        .copied()
        .unwrap_or(&PLATFORMS[0]);
    let device_model = platform
        .devices
        .choose(&mut rng)
        .copied()
        .unwrap_or("PC 64bit");
    let system_version = platform
        .systems
        .choose(&mut rng)
        .copied()
        .unwrap_or("Windows 10");
    let app_version = APP_VERSIONS.choose(&mut rng).copied().unwrap_or("5.2.3");
    let number = existing.iter().filter(|profile| profile.generated).count() + 1;
    DeviceProfile {
        id: new_id(),
        name: format!("{} #{number}", platform.label),
        device_model: device_model.to_string(),
        system_version: system_version.to_string(),
        app_version: format!("{app_version} x64"),
        system_lang_code: String::new(),
        lang_code: String::new(),
        generated: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_profiles_are_complete_and_unique_ids() {
        let a = generate(&[]);
        let b = generate(std::slice::from_ref(&a));
        assert_ne!(a.id, b.id);
        for p in [&a, &b] {
            assert!(!p.device_model.is_empty() && !p.system_version.is_empty());
            assert!(p.app_version.ends_with(" x64"));
        }
        assert!(b.name.ends_with("#2"));
    }

    #[test]
    fn apply_keeps_machine_language_when_profile_has_none() {
        let mut params = ConnectionParams::default();
        let language = params.lang_code.clone();
        let mut profile = generate(&[]);
        apply(&mut params, &profile);
        assert_eq!(params.device_model, profile.device_model);
        assert_eq!(params.lang_code, language);
        profile.lang_code = "ru".into();
        apply(&mut params, &profile);
        assert_eq!(params.lang_code, "ru");
    }
}
