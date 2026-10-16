//! Device profiles: what an account tells Telegram about itself when it connects (the system,
//! the device model and the app with its version), as shown under "Active sessions".
//!
//! I keep one profile per session file. With the generator on, every new account gets a
//! realistic desktop profile no other account uses; with it off, new accounts report
//! `Linux · VPS · Fly Telegram v.{version}`. The operator can also keep their own profiles and
//! pick or change the profile of any account; a change applies on the next connection.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use anyhow::{bail, Context, Result};
use rand::seq::SliceRandom;
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::database::Database;

const ENABLED_KEY: &str = "device_profiles.enabled";
const USE_CUSTOM_KEY: &str = "device_profiles.use_custom";
const CUSTOM_KEY: &str = "device_profiles.custom";
const ASSIGNED_KEY: &str = "device_profiles.assigned";
const MAX_FIELD_CHARS: usize = 64;
const MAX_CUSTOM: usize = 200;
const GENERATE_ATTEMPTS: usize = 64;

/// I serialize changes to the stored lists so two accounts signing in at once cannot get the
/// same generated profile or overwrite each other's assignment.
static LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub system: String,
    pub device: String,
    pub app: String,
    pub version: String,
}

impl Profile {
    /// What new accounts report while the generator is off.
    pub fn fallback() -> Self {
        Self {
            system: "Linux".into(),
            device: "VPS".into(),
            app: "Fly Telegram".into(),
            version: format!("v.{}", crate::VERSION),
        }
    }

    /// Telegram has a single `app_version` field, so I put the app name in front of the version.
    pub fn app_version(&self) -> String {
        [self.app.trim(), self.version.trim()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn label(&self) -> String {
        format!("{} · {} · {}", self.system, self.device, self.app_version())
    }

    /// I trim the fields and refuse empty, overlong or control characters.
    pub fn cleaned(self) -> Result<Self> {
        let field = |name: &str, value: String, required: bool| -> Result<String> {
            let value = value.trim().to_string();
            if required && value.is_empty() {
                bail!("{name} is empty");
            }
            if value.chars().count() > MAX_FIELD_CHARS {
                bail!("{name} is longer than {MAX_FIELD_CHARS} characters");
            }
            if value.chars().any(char::is_control) {
                bail!("{name} contains control characters");
            }
            Ok(value)
        };
        let profile = Self {
            system: field("the system", self.system, true)?,
            device: field("the device", self.device, true)?,
            app: field("the app", self.app, false)?,
            version: field("the version", self.version, false)?,
        };
        if profile.app_version().is_empty() {
            bail!("the app and its version are both empty");
        }
        Ok(profile)
    }

    fn same_as(&self, other: &Profile) -> bool {
        self.system.eq_ignore_ascii_case(&other.system)
            && self.device.eq_ignore_ascii_case(&other.device)
            && self.app_version().eq_ignore_ascii_case(&other.app_version())
    }
}

/// A profile the operator added to the list.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CustomProfile {
    pub id: String,
    #[serde(flatten)]
    pub profile: Profile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Generated,
    Custom,
    Manual,
}

/// The profile stored for one session file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Assigned {
    #[serde(flatten)]
    pub profile: Profile,
    pub source: Source,
    #[serde(default)]
    pub assigned_at: i64,
}

/// How the operator wants an account's profile chosen.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Choice {
    /// A generated profile while the generator is on, the default one otherwise.
    #[default]
    Auto,
    /// A generated profile; I keep the shown preview when one is passed.
    Generate {
        #[serde(default)]
        profile: Option<Profile>,
    },
    Default,
    Custom {
        id: String,
    },
    Manual {
        profile: Profile,
    },
}

/// A chosen profile before I store it. `source: None` means the default profile.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub profile: Profile,
    pub source: Option<Source>,
}

// ---------------------------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------------------------

pub async fn enabled(db: &Database) -> bool {
    db.get(ENABLED_KEY).await.as_bool().unwrap_or(false)
}

/// I offer the operator's own profiles to the generator unless they turned that off.
pub async fn use_custom(db: &Database) -> bool {
    db.get(USE_CUSTOM_KEY).await.as_bool().unwrap_or(true)
}

pub async fn set_options(
    db: &Database,
    enabled: Option<bool>,
    use_custom: Option<bool>,
) -> Result<()> {
    if let Some(value) = enabled {
        db.set(ENABLED_KEY, Value::Bool(value)).await?;
    }
    if let Some(value) = use_custom {
        db.set(USE_CUSTOM_KEY, Value::Bool(value)).await?;
    }
    Ok(())
}

pub async fn custom(db: &Database) -> Vec<CustomProfile> {
    serde_json::from_value(db.get(CUSTOM_KEY).await).unwrap_or_default()
}

pub async fn add_custom(db: &Database, profile: Profile) -> Result<CustomProfile> {
    let profile = profile.cleaned()?;
    let _lock = LOCK.lock().await;
    let mut list = custom(db).await;
    if list.iter().any(|item| item.profile.same_as(&profile)) {
        bail!("this profile is already in the list");
    }
    if list.len() >= MAX_CUSTOM {
        bail!("the list already holds {MAX_CUSTOM} profiles");
    }
    let item = CustomProfile {
        id: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(),
        profile,
    };
    list.push(item.clone());
    db.set(CUSTOM_KEY, serde_json::to_value(&list)?).await?;
    Ok(item)
}

pub async fn remove_custom(db: &Database, id: &str) -> Result<bool> {
    let _lock = LOCK.lock().await;
    let mut list = custom(db).await;
    let before = list.len();
    list.retain(|item| item.id != id);
    if list.len() == before {
        return Ok(false);
    }
    db.set(CUSTOM_KEY, serde_json::to_value(&list)?).await?;
    Ok(true)
}

// ---------------------------------------------------------------------------------------------
// Assignments
// ---------------------------------------------------------------------------------------------

fn session_key(session_file: &str) -> String {
    session_file.trim().replace('\\', "/")
}

async fn assignments(db: &Database) -> BTreeMap<String, Assigned> {
    serde_json::from_value(db.get(ASSIGNED_KEY).await).unwrap_or_default()
}

pub async fn assigned(db: &Database, session_file: &str) -> Option<Assigned> {
    assignments(db).await.remove(&session_key(session_file))
}

/// The profile an existing session connects with.
pub async fn for_session(db: &Database, session_file: &str) -> Profile {
    assigned(db, session_file)
        .await
        .map(|a| a.profile)
        .unwrap_or_else(Profile::fallback)
}

/// The profile for a session that is about to sign in for the first time. I keep a profile the
/// panel already picked for it; otherwise I generate one while the generator is on.
pub async fn for_new_session(db: &Database, session_file: &str) -> Result<Profile> {
    if let Some(existing) = assigned(db, session_file).await {
        return Ok(existing.profile);
    }
    let resolved = resolve(db, Choice::Auto).await?;
    store(db, session_file, &resolved).await?;
    Ok(resolved.profile)
}

/// I turn a choice into a profile without storing it, so a sign-in that never finishes leaves
/// nothing behind.
pub async fn resolve(db: &Database, choice: Choice) -> Result<Resolved> {
    let resolved = match choice {
        Choice::Auto if enabled(db).await => Resolved {
            profile: generate(db).await,
            source: Some(Source::Generated),
        },
        Choice::Auto | Choice::Default => Resolved {
            profile: Profile::fallback(),
            source: None,
        },
        Choice::Generate { profile } => Resolved {
            profile: match profile {
                Some(profile) => profile.cleaned()?,
                None => generate(db).await,
            },
            source: Some(Source::Generated),
        },
        Choice::Custom { id } => Resolved {
            profile: custom(db)
                .await
                .into_iter()
                .find(|item| item.id == id)
                .map(|item| item.profile)
                .context("this profile is no longer in the list")?,
            source: Some(Source::Custom),
        },
        Choice::Manual { profile } => Resolved {
            profile: profile.cleaned()?,
            source: Some(Source::Manual),
        },
    };
    Ok(resolved)
}

/// I store the profile for a session file; the default profile is stored as no entry, so it
/// follows the version after an update.
pub async fn store(db: &Database, session_file: &str, resolved: &Resolved) -> Result<()> {
    let _lock = LOCK.lock().await;
    let mut map = assignments(db).await;
    let key = session_key(session_file);
    match resolved.source {
        Some(source) => {
            map.insert(
                key,
                Assigned {
                    profile: resolved.profile.clone(),
                    source,
                    assigned_at: crate::account_tools::now_secs(),
                },
            );
        }
        None => {
            map.remove(&key);
        }
    }
    db.set(ASSIGNED_KEY, serde_json::to_value(&map)?).await
}

/// A profile no account uses yet: one of the operator's unused profiles when they allow it,
/// otherwise a random one from the built-in catalog.
pub async fn generate(db: &Database) -> Profile {
    let taken: Vec<Profile> = assignments(db)
        .await
        .into_values()
        .map(|a| a.profile)
        .collect();
    let unused = |profile: &Profile| !taken.iter().any(|t| t.same_as(profile));
    let own: Vec<Profile> = if use_custom(db).await {
        custom(db).await.into_iter().map(|item| item.profile).collect()
    } else {
        Vec::new()
    };
    // The thread RNG is not Send, so I take it only after the last await.
    let mut rng = rand::thread_rng();
    let free: Vec<&Profile> = own.iter().filter(|p| unused(p)).collect();
    if let Some(profile) = free.choose(&mut rng) {
        return (*profile).clone();
    }
    let mut candidate = random_builtin(&mut rng);
    for _ in 0..GENERATE_ATTEMPTS {
        if unused(&candidate) {
            break;
        }
        candidate = random_builtin(&mut rng);
    }
    candidate
}

// ---------------------------------------------------------------------------------------------
// Built-in catalog
// ---------------------------------------------------------------------------------------------

/// macOS releases with their major number, so I never put an old system on a newer Mac.
const MAC_SYSTEMS: &[(&str, u32)] = &[
    ("macOS 13.7", 13),
    ("macOS 14.7", 14),
    ("macOS 15.5", 15),
    ("macOS 15.6", 15),
    ("macOS 26.0", 26),
    ("macOS 26.1", 26),
];

/// Macs with the first macOS major they shipped with.
const MAC_DEVICES: &[(&str, u32)] = &[
    ("MacBook Air M1", 11),
    ("MacBook Air M2", 12),
    ("MacBook Air M3", 14),
    ("MacBook Air M4", 15),
    ("MacBook Pro 13 M1", 11),
    ("MacBook Pro 14 M1 Pro", 12),
    ("MacBook Pro 16 M1 Max", 12),
    ("MacBook Pro 14 M3 Pro", 14),
    ("MacBook Pro 16 M3 Max", 14),
    ("MacBook Pro 14 M4", 15),
    ("MacBook Pro 16 M4 Pro", 15),
    ("iMac 24 M1", 11),
    ("iMac 24 M3", 14),
    ("iMac 24 M4", 15),
    ("Mac mini M2", 13),
    ("Mac mini M4", 15),
    ("Mac Studio M2 Max", 13),
];

const WINDOWS_SYSTEMS: &[&str] = &["Windows 11", "Windows 11", "Windows 10"];

const PC_DEVICES: &[&str] = &[
    "ASUS TUF Gaming F15 FX507ZC",
    "ASUS ROG Strix G16 G614JV",
    "ASUS Vivobook 15 X1504ZA",
    "ASUS ZenBook 14 UX3402VA",
    "Lenovo Legion 5 15ACH6H",
    "Lenovo IdeaPad Slim 5 14IAH8",
    "Lenovo ThinkPad T14 Gen 3",
    "Lenovo ThinkPad X1 Carbon Gen 11",
    "Lenovo ThinkCentre M70q Gen 3",
    "HP Pavilion Gaming Laptop 15-ec2xxx",
    "HP EliteBook 840 G9",
    "HP ProBook 450 G9",
    "HP Victus 16-d1xxx",
    "Dell XPS 15 9520",
    "Dell XPS 13 9315",
    "Dell Latitude 5430",
    "Dell Inspiron 16 5630",
    "Dell OptiPlex 7090",
    "Acer Nitro AN515-58",
    "Acer Aspire 5 A515-57",
    "MSI Katana GF66 11UC",
    "MSI MS-7C56",
    "MSI MS-7D91",
    "Gigabyte B550 AORUS ELITE V2",
    "Gigabyte B650 AORUS ELITE AX",
    "ASUS PRIME B660M-A D4",
    "ASUS TUF GAMING B550-PLUS",
    "Huawei MateBook D 15",
    "Xiaomi RedmiBook Pro 15",
    "Microsoft Surface Laptop 5",
];

/// Machines people commonly run Linux on; I mix them with the PC list.
const LINUX_DEVICES: &[&str] = &[
    "Framework Laptop 13",
    "Framework Laptop 16",
    "System76 Lemur Pro",
    "Lenovo ThinkPad T480",
    "Lenovo ThinkPad X1 Carbon Gen 9",
    "Dell XPS 13 9310",
    "TUXEDO InfinityBook Pro 14",
];

const LINUX_SYSTEMS: &[&str] = &[
    "Ubuntu 24.04 LTS",
    "Ubuntu 24.04 LTS",
    "Ubuntu 22.04 LTS",
    "Ubuntu 25.04",
    "Fedora Linux 42",
    "Debian GNU/Linux 12",
    "Debian GNU/Linux 13",
    "Linux Mint 22.1",
    "Arch Linux",
    "Pop!_OS 22.04 LTS",
    "openSUSE Tumbleweed",
    "Manjaro Linux",
];

const DESKTOP_VERSIONS: &[&str] = &["5.12.3", "5.13.1", "5.14.3", "5.15.4", "5.16.4"];
const MACOS_APP_VERSIONS: &[&str] = &["11.9", "11.11", "11.13"];
const UNIGRAM_VERSIONS: &[&str] = &["11.8", "11.10", "11.12"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    Windows,
    Mac,
    Linux,
}

fn pick<'a>(rng: &mut impl Rng, items: &[&'a str]) -> &'a str {
    items.choose(rng).copied().unwrap_or_default()
}

/// A consistent desktop profile: the device fits the system and the app runs on it.
pub fn random_builtin(rng: &mut impl Rng) -> Profile {
    let family = match rng.gen_range(0..100) {
        0..=59 => Family::Windows,
        60..=84 => Family::Mac,
        _ => Family::Linux,
    };
    let (system, device) = match family {
        Family::Windows => (
            pick(rng, WINDOWS_SYSTEMS).to_string(),
            pick(rng, PC_DEVICES).to_string(),
        ),
        Family::Mac => {
            let (device, min_major) = *MAC_DEVICES.choose(rng).expect("the Mac list is not empty");
            let systems: Vec<&str> = MAC_SYSTEMS
                .iter()
                .filter(|(_, major)| *major >= min_major)
                .map(|(name, _)| *name)
                .collect();
            (pick(rng, &systems).to_string(), device.to_string())
        }
        Family::Linux => {
            let device = if rng.gen_bool(0.4) {
                pick(rng, LINUX_DEVICES)
            } else {
                pick(rng, PC_DEVICES)
            };
            (pick(rng, LINUX_SYSTEMS).to_string(), device.to_string())
        }
    };
    let (app, version) = match family {
        Family::Mac if rng.gen_bool(0.5) => ("Telegram macOS", pick(rng, MACOS_APP_VERSIONS).to_string()),
        Family::Mac => ("Telegram Desktop", pick(rng, DESKTOP_VERSIONS).to_string()),
        Family::Windows if rng.gen_bool(0.15) => ("Unigram", pick(rng, UNIGRAM_VERSIONS).to_string()),
        Family::Windows | Family::Linux => (
            "Telegram Desktop",
            format!("{} x64", pick(rng, DESKTOP_VERSIONS)),
        ),
    };
    Profile {
        system,
        device,
        app: app.to_string(),
        version,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_reports_linux_vps_and_this_version() {
        let profile = Profile::fallback();
        assert_eq!(profile.system, "Linux");
        assert_eq!(profile.device, "VPS");
        assert_eq!(
            profile.app_version(),
            format!("Fly Telegram v.{}", crate::VERSION)
        );
    }

    #[test]
    fn generated_profiles_are_consistent_desktops() {
        let mut rng = rand::thread_rng();
        for _ in 0..2000 {
            let p = random_builtin(&mut rng);
            assert!(p.clone().cleaned().is_ok(), "{p:?}");
            let mac_system = p.system.starts_with("macOS");
            let mac_device = MAC_DEVICES.iter().any(|(d, _)| *d == p.device);
            assert_eq!(mac_system, mac_device, "{p:?}");
            if p.app == "Telegram macOS" {
                assert!(mac_system, "{p:?}");
            }
            if p.app == "Unigram" {
                assert!(p.system.starts_with("Windows"), "{p:?}");
            }
            if let Some((_, min)) = MAC_DEVICES.iter().find(|(d, _)| *d == p.device) {
                let major = MAC_SYSTEMS.iter().find(|(s, _)| *s == p.system).unwrap().1;
                assert!(major >= *min, "{p:?}");
            }
        }
    }

    #[test]
    fn cleaning_trims_and_refuses_bad_fields() {
        let profile = Profile {
            system: "  Windows 11 ".into(),
            device: "PC".into(),
            app: "".into(),
            version: "5.0".into(),
        }
        .cleaned()
        .unwrap();
        assert_eq!(profile.system, "Windows 11");
        assert_eq!(profile.app_version(), "5.0");
        let empty = Profile {
            system: "".into(),
            ..profile.clone()
        };
        assert!(empty.cleaned().is_err());
        let control = Profile {
            device: "a\nb".into(),
            ..profile.clone()
        };
        assert!(control.cleaned().is_err());
        let long = Profile {
            device: "x".repeat(MAX_FIELD_CHARS + 1),
            ..profile
        };
        assert!(long.cleaned().is_err());
    }

    #[test]
    fn choices_parse_from_panel_json() {
        let auto: Choice = serde_json::from_str(r#"{"mode":"auto"}"#).unwrap();
        assert!(matches!(auto, Choice::Auto));
        let custom: Choice = serde_json::from_str(r#"{"mode":"custom","id":"abc"}"#).unwrap();
        assert!(matches!(custom, Choice::Custom { id } if id == "abc"));
        let manual: Choice = serde_json::from_str(
            r#"{"mode":"manual","profile":{"system":"Windows 10","device":"PC","app":"Telegram Desktop","version":"5.12.3 x64"}}"#,
        )
        .unwrap();
        assert!(matches!(manual, Choice::Manual { .. }));
    }

    #[test]
    fn assignments_round_trip_with_flattened_profile() {
        let assigned = Assigned {
            profile: Profile::fallback(),
            source: Source::Manual,
            assigned_at: 5,
        };
        let value = serde_json::to_value(&assigned).unwrap();
        assert_eq!(value["device"], "VPS");
        assert_eq!(value["source"], "manual");
        let back: Assigned = serde_json::from_value(value).unwrap();
        assert_eq!(back.profile, Profile::fallback());
    }
}
