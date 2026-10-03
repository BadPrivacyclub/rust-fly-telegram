//! Built-in modules compiled into the binary.
//!
//! On startup missing bundled files are written to the modules directory. A lock file records
//! the hash of every file written this way, so later releases can upgrade files the operator
//! has not edited while leaving customized files alone.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use tracing::{info, warn};

use crate::loader::manifest::source_sha256;

include!(concat!(env!("OUT_DIR"), "/bundled_modules.rs"));

const LOCK_FILE: &str = ".bundled.json";

pub fn bundled_source(file_name: &str) -> Option<&'static str> {
    BUNDLED_FILES
        .iter()
        .find(|(name, _)| *name == file_name)
        .map(|(_, content)| *content)
}

/// A module whose source is byte-identical to the embedded copy carries the same trust as
/// the binary itself, so it does not need an operator signature to run unsandboxed.
pub fn is_pristine(file_name: &str, source: &str) -> bool {
    bundled_source(file_name).is_some_and(|bundled| normalize(bundled) == normalize(source))
}

/// Git on Windows may check files out with CRLF line endings.
fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n")
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub installed: Vec<String>,
    pub upgraded: Vec<String>,
    pub kept_customized: Vec<String>,
}

pub fn sync(modules_dir: &Path) -> Result<SyncReport> {
    std::fs::create_dir_all(modules_dir)?;
    let lock_path = modules_dir.join(LOCK_FILE);
    let mut lock: BTreeMap<String, String> = std::fs::read_to_string(&lock_path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();

    let mut report = SyncReport::default();
    for (name, content) in BUNDLED_FILES {
        let path = modules_dir.join(name);
        let bundled_hash = source_sha256(&normalize(content));
        match std::fs::read_to_string(&path) {
            Err(_) => {
                std::fs::write(&path, content)?;
                lock.insert(name.to_string(), bundled_hash);
                report.installed.push(name.to_string());
            }
            Ok(existing) => {
                let existing_hash = source_sha256(&normalize(&existing));
                if existing_hash == bundled_hash {
                    lock.insert(name.to_string(), bundled_hash);
                } else if lock.get(*name) == Some(&existing_hash) {
                    std::fs::write(&path, content)?;
                    lock.insert(name.to_string(), bundled_hash);
                    report.upgraded.push(name.to_string());
                } else {
                    report.kept_customized.push(name.to_string());
                }
            }
        }
    }

    std::fs::write(&lock_path, serde_json::to_string_pretty(&lock)?)?;

    if !report.installed.is_empty() {
        info!(
            "installed {} bundled module file(s)",
            report.installed.len()
        );
    }
    if !report.upgraded.is_empty() {
        info!("upgraded bundled modules: {}", report.upgraded.join(", "));
    }
    let customized = report
        .kept_customized
        .iter()
        .filter(|name| name.ends_with(".lua"))
        .cloned()
        .collect::<Vec<_>>();
    if !customized.is_empty() {
        warn!(
            "kept locally modified bundled modules (they run sandboxed unless signed): {}",
            customized.join(", ")
        );
    }
    Ok(report)
}

/// Overwrites every bundled file with the embedded copy. Used by `.modules restore`.
pub fn restore(modules_dir: &Path, only: Option<&str>) -> Result<Vec<String>> {
    let mut restored = Vec::new();
    for (name, content) in BUNDLED_FILES {
        if let Some(module) = only {
            let stem = name.split('.').next().unwrap_or_default();
            if stem != module {
                continue;
            }
        }
        std::fs::write(modules_dir.join(name), content)?;
        restored.push(name.to_string());
    }
    // Re-record hashes so future upgrades apply to the restored files.
    sync(modules_dir)?;
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("fly_bundled_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn bundle_contains_core_modules() {
        assert!(bundled_source("core.lua").is_some());
        assert!(bundled_source("help.lua").is_some());
    }

    #[test]
    fn sync_installs_upgrades_and_keeps_custom_files() {
        let dir = temp_dir();
        let report = sync(&dir).unwrap();
        assert_eq!(report.installed.len(), BUNDLED_FILES.len());

        // Simulate an older pristine release file: the lock hash matches the file contents.
        let core = dir.join("core.lua");
        std::fs::write(&core, "-- old release").unwrap();
        let lock_path = dir.join(LOCK_FILE);
        let mut lock: BTreeMap<String, String> =
            serde_json::from_str(&std::fs::read_to_string(&lock_path).unwrap()).unwrap();
        lock.insert("core.lua".into(), source_sha256("-- old release"));
        std::fs::write(&lock_path, serde_json::to_string(&lock).unwrap()).unwrap();

        // An operator edit has no matching lock hash and must be preserved.
        let notes = dir.join("notes.lua");
        std::fs::write(&notes, "-- customized").unwrap();

        let report = sync(&dir).unwrap();
        assert_eq!(report.upgraded, vec!["core.lua".to_string()]);
        assert!(report.kept_customized.contains(&"notes.lua".to_string()));
        assert!(is_pristine(
            "core.lua",
            &std::fs::read_to_string(&core).unwrap()
        ));
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), "-- customized");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn pristine_check_ignores_line_endings() {
        let source = bundled_source("core.lua").unwrap().replace('\n', "\r\n");
        assert!(is_pristine("core.lua", &source));
        assert!(!is_pristine("core.lua", "return {}"));
    }
}
