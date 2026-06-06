use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ModuleManifest {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub checksum: Option<String>,
    #[serde(default)]
    pub min_core_version: Option<String>,
    #[serde(default)]
    pub trusted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ModuleInfo {
    pub name: String,
    pub version: String,
    pub description: String,
    pub commands: Vec<String>,
    pub permissions: Vec<String>,
    pub trusted: bool,
    pub audit: Vec<String>,
}

impl ModuleManifest {
    pub fn inferred(name: String, commands: Vec<String>, source: &str) -> Self {
        Self {
            name,
            version: default_version(),
            description: String::new(),
            commands,
            permissions: Vec::new(),
            dependencies: required_modules(source),
            source: None,
            checksum: None,
            min_core_version: None,
            trusted: false,
            signature: None,
        }
    }

    pub fn normalized(mut self, fallback_name: &str, commands: Vec<String>) -> Self {
        if self.name.trim().is_empty() {
            self.name = fallback_name.to_string();
        }
        if self.version.trim().is_empty() {
            self.version = default_version();
        }
        if self.commands.is_empty() {
            self.commands = commands;
        }
        self.permissions.sort();
        self.permissions.dedup();
        self
    }
}

pub async fn load_manifest(
    module_path: &Path,
    fallback_name: &str,
    commands: Vec<String>,
    source: &str,
    verifying_key: Option<&VerifyingKey>,
) -> Result<ModuleManifest> {
    let manifest_path = manifest_path(module_path);
    if !manifest_path.exists() {
        return Ok(ModuleManifest::inferred(
            fallback_name.to_string(),
            commands,
            source,
        ));
    }

    let raw = tokio::fs::read_to_string(&manifest_path)
        .await
        .with_context(|| format!("reading manifest {manifest_path:?}"))?;
    let mut manifest = serde_json::from_str::<ModuleManifest>(&raw)
        .with_context(|| format!("parsing manifest {manifest_path:?}"))?;
    manifest = manifest.normalized(fallback_name, commands);

    if manifest.trusted {
        let ok = verifying_key
            .map(|vk| verify_trusted(&manifest, source, vk))
            .unwrap_or(false);
        if !ok {
            tracing::warn!(
                "module '{}': trusted flag cleared — missing or invalid signature",
                manifest.name
            );
            manifest.trusted = false;
        }
    }

    Ok(manifest)
}

pub async fn write_generated_manifest(
    modules_dir: &Path,
    file_name: &str,
    source_url: Option<String>,
    source: &str,
) -> mlua::Result<PathBuf> {
    let module_path = modules_dir.join(file_name);
    let name = module_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("module")
        .to_string();
    let manifest = ModuleManifest {
        name,
        version: default_version(),
        description: "Installed public module. Grant permissions manually after review."
            .to_string(),
        commands: module_commands(source),
        permissions: Vec::new(),
        dependencies: required_modules(source),
        source: source_url,
        checksum: None,
        min_core_version: None,
        trusted: false,
        signature: None,
    };
    let path = manifest_path(&module_path);
    let raw = serde_json::to_string_pretty(&manifest)
        .map_err(|error| mlua::Error::runtime(error.to_string()))?;
    tokio::fs::write(&path, raw)
        .await
        .map_err(|error| mlua::Error::runtime(error.to_string()))?;
    Ok(path)
}

pub fn manifest_path(module_path: &Path) -> PathBuf {
    let mut name = module_path.as_os_str().to_os_string();
    name.push(".manifest.json");
    PathBuf::from(name)
}

pub fn module_info(
    manifest: &ModuleManifest,
    commands: &HashMap<String, String>,
    source: &str,
) -> ModuleInfo {
    let mut command_names = if manifest.commands.is_empty() {
        commands.keys().cloned().collect::<Vec<_>>()
    } else {
        manifest.commands.clone()
    };
    command_names.sort();
    command_names.dedup();

    ModuleInfo {
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        description: manifest.description.clone(),
        commands: command_names,
        permissions: manifest.permissions.clone(),
        trusted: manifest.trusted,
        audit: audit_source(source),
    }
}

pub fn audit_source(source: &str) -> Vec<String> {
    let lower = source.to_lowercase();
    let mut capabilities = Vec::new();
    push_capability(
        &mut capabilities,
        lower.contains("io.") || lower.contains("require(\"io\"") || lower.contains("require 'io'"),
        "filesystem",
    );
    push_capability(
        &mut capabilities,
        lower.contains("os.")
            || lower.contains("ctx:run_term")
            || lower.contains("powershell")
            || lower.contains("cmd /c"),
        "shell",
    );
    push_capability(
        &mut capabilities,
        lower.contains("http://") || lower.contains("https://") || lower.contains("ctx:http_"),
        "network",
    );
    push_capability(
        &mut capabilities,
        lower.contains("ctx:delete_last_own") || lower.contains("ctx:message_info"),
        "telegram.history",
    );
    push_capability(
        &mut capabilities,
        lower.contains("ctx:download_replied_media")
            || lower.contains("ctx:download_url")
            || lower.contains("ctx:send_file"),
        "telegram.media",
    );
    push_capability(&mut capabilities, lower.contains("ctx:env_get"), "secrets");
    push_capability(
        &mut capabilities,
        lower.contains("ctx:install_module") || lower.contains("ctx:install_replied_module"),
        "modules.install",
    );
    capabilities
}

pub fn module_commands(source: &str) -> Vec<String> {
    let mut commands = Vec::new();
    for line in source.lines() {
        let line = line.trim();
        if line.starts_with("--") || !line.contains('=') {
            continue;
        }
        if let Some((name, _handler)) = line.split_once('=') {
            let name = name.trim().trim_matches(['[', ']', '"', '\'']);
            if is_valid_command_name(name) && !commands.contains(&name.to_string()) {
                commands.push(name.to_string());
            }
        }
    }
    commands
}

pub fn required_modules(source: &str) -> Vec<String> {
    let mut modules = Vec::new();
    for line in source.lines() {
        let line = line.trim();
        for quote in ['"', '\''] {
            let Some(start) = line
                .find(&format!("require({quote}"))
                .or_else(|| line.find(&format!("require {quote}")))
            else {
                continue;
            };
            let after = &line[start..];
            let Some(first_quote) = after.find(quote) else {
                continue;
            };
            let rest = &after[first_quote + 1..];
            let Some(end_quote) = rest.find(quote) else {
                continue;
            };
            let module = rest[..end_quote].replace('.', "/");
            if !module.is_empty() && !modules.contains(&module) {
                modules.push(module);
            }
        }
    }
    modules
}

/// Returns the hex-encoded SHA-256 of the given source string.
pub fn source_sha256(source: &str) -> String {
    let hash = Sha256::digest(source.as_bytes());
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

/// Builds the deterministic JSON payload that is signed / verified.
///
/// Keys are sorted lexicographically (via BTreeMap) for a stable byte sequence.
pub fn signing_payload(manifest: &ModuleManifest, sha256: &str) -> Vec<u8> {
    let mut map: BTreeMap<&str, serde_json::Value> = BTreeMap::new();
    map.insert("name", serde_json::Value::String(manifest.name.clone()));
    map.insert(
        "permissions",
        serde_json::Value::Array(
            manifest
                .permissions
                .iter()
                .map(|p| serde_json::Value::String(p.clone()))
                .collect(),
        ),
    );
    map.insert(
        "source_sha256",
        serde_json::Value::String(sha256.to_string()),
    );
    map.insert("trusted", serde_json::Value::Bool(true));
    map.insert(
        "version",
        serde_json::Value::String(manifest.version.clone()),
    );
    serde_json::to_vec(&map).expect("BTreeMap serialization is infallible")
}

/// Returns `true` iff the manifest carries a valid Ed25519 signature that covers
/// the source file hash, name, version, permissions, and the trusted flag.
pub fn verify_trusted(manifest: &ModuleManifest, source: &str, vk: &VerifyingKey) -> bool {
    if !manifest.trusted {
        return false;
    }
    let Some(ref sig_b64) = manifest.signature else {
        return false;
    };
    let sha256 = source_sha256(source);
    let payload = signing_payload(manifest, &sha256);
    let Ok(sig_bytes) = STANDARD.decode(sig_b64) else {
        return false;
    };
    let Ok(sig_arr): Result<[u8; 64], _> = sig_bytes.try_into() else {
        return false;
    };
    let sig = Signature::from_bytes(&sig_arr);
    vk.verify(&payload, &sig).is_ok()
}

fn default_version() -> String {
    "1.0.0".to_string()
}

fn is_valid_command_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn push_capability(capabilities: &mut Vec<String>, condition: bool, name: &str) {
    if condition {
        capabilities.push(name.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modules_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("modules")
    }

    fn lua_module_paths() -> Vec<PathBuf> {
        let mut paths = std::fs::read_dir(modules_dir())
            .expect("modules directory should exist")
            .filter_map(|entry| {
                let path = entry.expect("module entry should load").path();
                path.extension()
                    .is_some_and(|extension| extension == "lua")
                    .then_some(path)
            })
            .collect::<Vec<_>>();
        paths.sort();
        paths
    }

    fn read_manifest(path: &Path) -> ModuleManifest {
        let manifest_path = manifest_path(path);
        let source = std::fs::read_to_string(&manifest_path).unwrap_or_else(|error| {
            panic!("manifest {manifest_path:?} should be readable: {error}")
        });
        serde_json::from_str::<ModuleManifest>(&source)
            .unwrap_or_else(|error| panic!("manifest {manifest_path:?} should parse: {error}"))
    }

    fn actual_commands(path: &Path, source: &str) -> Vec<String> {
        let lua = mlua::Lua::new();
        let table = lua
            .load(source)
            .set_name(path.to_string_lossy().as_ref())
            .eval::<mlua::Table>()
            .unwrap_or_else(|error| panic!("module {path:?} should return a table: {error}"));
        let commands = table
            .get::<mlua::Table>("commands")
            .unwrap_or_else(|error| panic!("module {path:?} should define commands: {error}"));
        let mut names = commands
            .pairs::<String, String>()
            .map(|pair| {
                let (name, _handler) = pair
                    .unwrap_or_else(|error| panic!("invalid commands entry in {path:?}: {error}"));
                name
            })
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    fn sandbox_env(lua: &mlua::Lua) -> mlua::Table {
        let globals = lua.globals();
        let env = lua.create_table().unwrap();
        for name in [
            "assert", "error", "ipairs", "next", "pairs", "pcall", "select", "tonumber",
            "tostring", "type", "xpcall",
        ] {
            env.set(name, globals.get::<mlua::Value>(name).unwrap()).unwrap();
        }
        for name in ["coroutine", "math", "string", "table", "utf8"] {
            env.set(name, globals.get::<mlua::Table>(name).unwrap()).unwrap();
        }
        env.set("_G", env.clone()).unwrap();
        env
    }

    #[test]
    fn bundled_modules_load_and_expose_commands_in_sandbox() {
        let lua = mlua::Lua::new();
        for path in lua_module_paths() {
            let manifest = read_manifest(&path);
            let source = std::fs::read_to_string(&path).expect("module should be readable");
            let env = sandbox_env(&lua);
            let table: mlua::Table = lua
                .load(&source)
                .set_name(path.to_string_lossy().as_ref())
                .set_environment(env)
                .eval()
                .unwrap_or_else(|e| panic!("sandbox load failed for {path:?}: {e}"));

            let cmds_table: mlua::Table = table
                .get("commands")
                .unwrap_or_else(|e| panic!("no commands table in {path:?}: {e}"));

            let actual: Vec<String> = cmds_table
                .pairs::<String, String>()
                .map(|p| p.expect("commands entry should be string→string").0)
                .collect();

            for expected_cmd in &manifest.commands {
                assert!(
                    actual.iter().any(|c| c == expected_cmd),
                    "sandbox-loaded {path:?} is missing command '{expected_cmd}' (got {actual:?})"
                );
            }
        }
    }

    #[test]
    fn bundled_lua_modules_have_valid_syntax() {
        let lua = mlua::Lua::new();
        for path in lua_module_paths() {
            let source = std::fs::read_to_string(&path).expect("module should be readable");
            lua.load(&source)
                .set_name(path.to_string_lossy().as_ref())
                .into_function()
                .unwrap_or_else(|error| panic!("invalid Lua syntax in {path:?}: {error}"));
        }
    }

    #[test]
    fn bundled_manifests_are_valid_json() {
        let mut count = 0;
        for entry in std::fs::read_dir(modules_dir()).expect("modules directory should exist") {
            let path = entry.expect("module entry should load").path();
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if !name.ends_with(".lua.manifest.json") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("manifest should be readable");
            let manifest =
                serde_json::from_str::<ModuleManifest>(&source).expect("manifest should parse");
            assert!(!manifest.name.trim().is_empty());
            count += 1;
        }
        assert!(count >= 1, "expected bundled manifests");
    }

    #[test]
    fn bundled_lua_modules_have_sibling_manifests() {
        let paths = lua_module_paths();
        assert!(!paths.is_empty(), "expected bundled Lua modules");
        for path in paths {
            let path = manifest_path(&path);
            assert!(path.exists(), "missing manifest {path:?}");
        }
    }

    #[test]
    fn bundled_manifests_match_lua_commands() {
        for path in lua_module_paths() {
            let source = std::fs::read_to_string(&path).expect("module should be readable");
            let mut manifest = read_manifest(&path);
            manifest.commands.sort();
            manifest.commands.dedup();

            let expected_name = path
                .file_stem()
                .and_then(|value| value.to_str())
                .expect("module should have a UTF-8 file stem");
            assert_eq!(
                manifest.name, expected_name,
                "manifest name should match module file stem for {path:?}"
            );
            assert_eq!(
                manifest.commands,
                actual_commands(&path, &source),
                "manifest commands should match M.commands in {path:?}"
            );
        }
    }

    #[test]
    fn bundled_manifest_permissions_cover_audited_capabilities() {
        for path in lua_module_paths() {
            let source = std::fs::read_to_string(&path).expect("module should be readable");
            let manifest = read_manifest(&path);
            for capability in audit_source(&source) {
                assert!(
                    manifest
                        .permissions
                        .iter()
                        .any(|value| value == &capability),
                    "manifest for {path:?} is missing permission {capability:?}"
                );
            }
        }
    }

    #[test]
    fn manifest_signature_roundtrip() {
        use ed25519_dalek::{Signer, SigningKey};
        use rand::rngs::OsRng;

        let sk = SigningKey::generate(&mut OsRng);
        let vk = sk.verifying_key();

        let source = "-- test\n";
        let sha256 = source_sha256(source);

        let mut manifest = ModuleManifest {
            name: "test".to_string(),
            version: "1.0.0".to_string(),
            description: String::new(),
            commands: Vec::new(),
            permissions: vec!["shell".to_string()],
            dependencies: Vec::new(),
            source: None,
            checksum: None,
            min_core_version: None,
            trusted: true,
            signature: None,
        };

        let payload = signing_payload(&manifest, &sha256);
        let sig = sk.sign(&payload).to_bytes();
        manifest.signature = Some(STANDARD.encode(sig));

        assert!(verify_trusted(&manifest, source, &vk), "valid sig should verify");

        // Tampered permissions invalidate the signature.
        manifest.permissions.push("network".to_string());
        assert!(!verify_trusted(&manifest, source, &vk), "bad perms should fail");

        // Restore permissions but change source.
        manifest.permissions.pop();
        assert!(
            !verify_trusted(&manifest, "-- different source\n", &vk),
            "different source should fail"
        );
    }
}
