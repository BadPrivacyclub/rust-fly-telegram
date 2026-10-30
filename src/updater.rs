//! Self-update from GitHub Releases for installs that use a prebuilt binary.
//! Source checkouts I keep updating with `git pull` and `cargo build --release`.
//!
//! A release is installed only when it is newer and its binary matches `sha256sums.txt`. Once
//! `assets/release-signing.pub` holds a key, that file must also carry a valid signature, which
//! covers the tag too, so an old signed release can't be served as a new one.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use ed25519_dalek::VerifyingKey;
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const REPOSITORY: &str = "BadPrivacyclub/rust-fly-telegram";
pub const TARGET: &str = env!("FLY_TARGET");
const MAX_BINARY_BYTES: usize = 150 * 1024 * 1024;
const CHECKSUMS_ASSET: &str = "sha256sums.txt";
const SIGNATURE_ASSET: &str = "sha256sums.txt.sig";
/// The public half of the key that signs releases, base64. Empty means releases aren't signed yet.
const RELEASE_PUBLIC_KEY: &str = include_str!("../assets/release-signing.pub");

#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    pub tag: String,
    pub version: String,
    pub notes: String,
    pub page_url: String,
    pub binary_url: Option<String>,
    pub checksums_url: Option<String>,
    pub signature_url: Option<String>,
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

/// Release assets include the bare executable for each target; I name it like this.
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
    let asset_url = |name: &str| {
        release
            .assets
            .iter()
            .find(|asset| asset.name == name)
            .map(|asset| asset.browser_download_url.clone())
    };
    let binary_url = asset_url(&binary_asset_name());
    let checksums_url = asset_url(CHECKSUMS_ASSET);
    let signature_url = asset_url(SIGNATURE_ASSET);
    Ok(ReleaseInfo {
        version: release.tag_name.trim_start_matches('v').to_string(),
        tag: release.tag_name,
        notes: release.body.unwrap_or_default(),
        page_url: release.html_url,
        binary_url,
        checksums_url,
        signature_url,
    })
}

/// I compare dotted numeric versions and ignore pre-release suffixes.
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

fn release_key() -> Result<Option<VerifyingKey>> {
    let encoded = RELEASE_PUBLIC_KEY.trim();
    if encoded.is_empty() {
        return Ok(None);
    }
    let bytes: [u8; 32] = STANDARD
        .decode(encoded)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .context("the built-in release key is damaged")?;
    Ok(Some(VerifyingKey::from_bytes(&bytes)?))
}

/// The signature covers the tag and the checksum list, as signed in `release.yml`.
fn signature_is_valid(tag: &str, checksums: &[u8], signature: &str, key: &VerifyingKey) -> bool {
    let Some(signature) = STANDARD
        .decode(signature.trim())
        .ok()
        .and_then(|bytes| <[u8; 64]>::try_from(bytes).ok())
    else {
        return false;
    };
    let mut payload = format!("{tag}\n").into_bytes();
    payload.extend_from_slice(checksums);
    crate::crypto::verify_bytes(&payload, &signature, key)
}

/// I find the checksum of `asset` in `sha256sum` output.
fn expected_sha256(checksums: &str, asset: &str) -> Option<String> {
    checksums.lines().find_map(|line| {
        let (hash, name) = line.split_once(char::is_whitespace)?;
        (name.trim().trim_start_matches('*') == asset).then(|| hash.to_ascii_lowercase())
    })
}

fn binary_matches(checksums: &str, asset: &str, binary: &[u8]) -> Result<()> {
    let expected = expected_sha256(checksums, asset)
        .with_context(|| format!("{CHECKSUMS_ASSET} does not list {asset}"))?;
    if crate::database::hex(&Sha256::digest(binary)) != expected {
        anyhow::bail!("the downloaded binary does not match its checksum; nothing was installed");
    }
    Ok(())
}

async fn download(url: &str) -> Result<Vec<u8>> {
    let response = http()?.get(url).send().await?.error_for_status()?;
    Ok(response.bytes().await?.to_vec())
}

/// I fetch and check the checksum list before downloading anything executable.
async fn verified_checksums(release: &ReleaseInfo) -> Result<String> {
    let url = release.checksums_url.as_ref().with_context(|| {
        format!(
            "release {} has no {CHECKSUMS_ASSET}, so it can't be verified; download it from {}",
            release.tag, release.page_url
        )
    })?;
    let checksums = download(url).await?;
    if let Some(key) = release_key()? {
        let url = release.signature_url.as_ref().with_context(|| {
            format!(
                "release {} is not signed; nothing was installed",
                release.tag
            )
        })?;
        let signature = String::from_utf8(download(url).await?).unwrap_or_default();
        if !signature_is_valid(&release.tag, &checksums, &signature, &key) {
            anyhow::bail!(
                "the signature of release {} is invalid; nothing was installed",
                release.tag
            );
        }
    }
    String::from_utf8(checksums).context("the checksum list is damaged")
}

/// I verify the release, then atomically swap it in for the running executable.
pub async fn install(release: &ReleaseInfo) -> Result<PathBuf> {
    if !is_newer(&release.version, crate::VERSION) {
        anyhow::bail!(
            "release {} is not newer than {}; I do not downgrade",
            release.version,
            crate::VERSION
        );
    }
    let url = release.binary_url.as_ref().with_context(|| {
        format!(
            "release {} has no binary for {TARGET}; download it from {}",
            release.tag, release.page_url
        )
    })?;
    let checksums = verified_checksums(release).await?;
    let bytes = download(url).await?;
    if bytes.len() > MAX_BINARY_BYTES {
        anyhow::bail!("release binary is unexpectedly large");
    }
    if bytes.len() < 1024 * 1024 {
        anyhow::bail!("release binary is unexpectedly small");
    }
    binary_matches(&checksums, &binary_asset_name(), &bytes)?;

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
        // A running executable cannot be overwritten on Windows, but it can be renamed, so I do that.
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

    fn signed(tag: &str, checksums: &[u8], key: &ed25519_dalek::SigningKey) -> String {
        let mut payload = format!("{tag}\n").into_bytes();
        payload.extend_from_slice(checksums);
        STANDARD.encode(crate::crypto::sign_bytes(&payload, key))
    }

    #[test]
    fn signatures_bind_the_tag_and_the_checksums() {
        let (secret, public) = crate::crypto::generate_keypair();
        let (other, _) = crate::crypto::generate_keypair();
        let list = b"abc  fly-telegram-x\n";
        let good = signed("v1.0.0", list, &secret);
        assert!(signature_is_valid("v1.0.0", list, &good, &public));
        assert!(
            !signature_is_valid("v9.0.0", list, &good, &public),
            "replayed tag"
        );
        assert!(!signature_is_valid("v1.0.0", b"other", &good, &public));
        let forged = signed("v1.0.0", list, &other);
        assert!(!signature_is_valid("v1.0.0", list, &forged, &public));
        assert!(!signature_is_valid("v1.0.0", list, "not base64!", &public));
        assert!(!signature_is_valid("v1.0.0", list, "", &public));
    }

    #[test]
    fn the_binary_must_match_its_listed_checksum() {
        let hash = crate::database::hex(&Sha256::digest(b"binary"));
        let list = format!("{hash}  fly-telegram-x\n0000  other\n");
        assert!(binary_matches(&list, "fly-telegram-x", b"binary").is_ok());
        assert!(binary_matches(&list, "fly-telegram-x", b"tampered").is_err());
        assert!(binary_matches(&list, "fly-telegram-y", b"binary").is_err());
        let binary_mode = format!("{hash} *fly-telegram-x\n");
        assert!(binary_matches(&binary_mode, "fly-telegram-x", b"binary").is_ok());
    }

    #[test]
    fn the_built_in_key_is_valid_or_empty() {
        assert!(release_key().is_ok());
    }

    #[tokio::test]
    async fn an_older_release_is_refused_before_any_download() {
        let release = ReleaseInfo {
            tag: "v0.0.1".into(),
            version: "0.0.1".into(),
            notes: String::new(),
            page_url: String::new(),
            binary_url: Some("http://127.0.0.1:9/never".into()),
            checksums_url: None,
            signature_url: None,
        };
        let error = install(&release).await.unwrap_err();
        assert!(error.to_string().contains("downgrade"), "{error}");
    }

    #[tokio::test]
    async fn a_release_without_checksums_is_refused() {
        let release = ReleaseInfo {
            tag: "v99.0.0".into(),
            version: "99.0.0".into(),
            notes: String::new(),
            page_url: "https://example.invalid".into(),
            binary_url: Some("http://127.0.0.1:9/never".into()),
            checksums_url: None,
            signature_url: None,
        };
        let error = install(&release).await.unwrap_err();
        assert!(error.to_string().contains("can't be verified"), "{error}");
    }

    #[test]
    fn asset_name_mentions_target() {
        assert!(binary_asset_name().contains(TARGET));
    }
}
