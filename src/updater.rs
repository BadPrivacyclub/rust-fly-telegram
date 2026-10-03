//! Self-update from GitHub Releases for installs that use a prebuilt binary.
//! Source checkouts keep updating with `git pull` and `cargo build --release`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

pub const REPOSITORY: &str = "BadPrivacyclub/rust-fly-telegram";
pub const TARGET: &str = env!("FLY_TARGET");
const MAX_BINARY_BYTES: usize = 150 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    pub tag: String,
    pub version: String,
    pub notes: String,
    pub page_url: String,
    pub binary_url: Option<String>,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    html_url: String,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

/// Release assets include the bare executable for each target, named like this.
pub fn binary_asset_name() -> String {
    if cfg!(windows) {
        format!("fly-telegram-{TARGET}.exe")
    } else {
        format!("fly-telegram-{TARGET}")
    }
}

fn http() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .user_agent(concat!("fly-telegram/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

pub async fn latest_release() -> Result<ReleaseInfo> {
    let url = format!("https://api.github.com/repos/{REPOSITORY}/releases/latest");
    let release: GithubRelease = http()?
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()
        .context("GitHub has no published release yet")?
        .json()
        .await?;
    let asset_name = binary_asset_name();
    let binary_url = release
        .assets
        .iter()
        .find(|asset| asset.name == asset_name)
        .map(|asset| asset.browser_download_url.clone());
    Ok(ReleaseInfo {
        version: release.tag_name.trim_start_matches('v').to_string(),
        tag: release.tag_name,
        notes: release.body.unwrap_or_default(),
        page_url: release.html_url,
        binary_url,
    })
}

/// Compares dotted numeric versions; pre-release suffixes are ignored.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    fn parts(version: &str) -> Vec<u64> {
        version
            .trim_start_matches('v')
            .split(['.', '-', '+'])
            .take(3)
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    }
    parts(candidate) > parts(current)
}

pub fn is_source_checkout() -> bool {
    Path::new(".git").exists() && Path::new("Cargo.toml").exists()
}

/// Downloads the release binary and atomically swaps it in for the running executable.
pub async fn install(release: &ReleaseInfo) -> Result<PathBuf> {
    let url = release.binary_url.as_ref().with_context(|| {
        format!(
            "release {} has no binary for {TARGET}; download it from {}",
            release.tag, release.page_url
        )
    })?;
    let bytes = http()?
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    if bytes.len() > MAX_BINARY_BYTES {
        anyhow::bail!("release binary is unexpectedly large");
    }
    if bytes.len() < 1024 * 1024 {
        anyhow::bail!("release binary is unexpectedly small");
    }

    let exe = std::env::current_exe()?;
    let staged = exe.with_extension("new");
    tokio::fs::write(&staged, &bytes).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).await?;
    }
    #[cfg(windows)]
    {
        // A running executable cannot be overwritten on Windows, but it can be renamed.
        let old = exe.with_extension("old.exe");
        let _ = tokio::fs::remove_file(&old).await;
        tokio::fs::rename(&exe, &old).await?;
    }
    tokio::fs::rename(&staged, &exe).await?;
    Ok(exe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.3.0", "0.2.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        assert!(is_newer("0.2.1-beta", "0.2.0"));
    }

    #[test]
    fn asset_name_mentions_target() {
        assert!(binary_asset_name().contains(TARGET));
    }
}
