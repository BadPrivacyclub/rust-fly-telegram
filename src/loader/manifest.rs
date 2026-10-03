use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Permissions a module can request. `shell`, `secrets`, `modules.install`, and
/// `core.admin` effectively grant control of the host or the account.
pub const KNOWN_PERMISSIONS: &[&str] = &[
    "network",
    "telegram.read",
    "telegram.history",
    "telegram.media",
    "modules.install",
    "secrets",
    "shell",
    "core.admin",
];

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
    pristine_bundled: bool,
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
        let ok = pristine_bundled
            || verifying_key
                .map(|vk| verify_trusted(&manifest, source, vk))
                .unwrap_or(false);
        if !ok {
            tracing::warn!(
                "module '{}': trusted flag cleared because its signature is missing or invalid",
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
    // The loaded command table is authoritative; the manifest list is only a fallback
    // for modules that failed to declare one.
    let mut command_names = if commands.is_empty() {
        manifest.commands.clone()
    } else {
        commands.keys().cloned().collect::<Vec<_>>()
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
            let (parenthesized, spaced) = match quote {
                '"' => ("require(\"", "require \""),
                '\'' => ("require('", "require '"),
                _ => unreachable!(),
            };
            let Some(start) = line.find(parenthesized).or_else(|| line.find(spaced)) else {
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

pub fn source_sha256(source: &str) -> String {
    let hash = Sha256::digest(source.as_bytes());
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

/// Builds a canonical signing payload with lexicographically sorted keys.
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
    use std::hint::black_box;
    use std::time::Instant;

    fn modules_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("modules")
    }

    #[test]
    fn required_modules_supports_both_quotes_and_call_styles() {
        let source = r#"
            local alpha = require("alpha.core")
            local beta = require 'beta.util'
            local duplicate = require "alpha.core"
        "#;

        assert_eq!(required_modules(source), ["alpha/core", "beta/util"]);
    }

    #[test]
    #[ignore = "manual performance benchmark"]
    fn benchmark_required_modules_without_temporary_patterns() {
        let source = (0..10_000)
            .map(|value| format!("local value_{value} = {value}"))
            .collect::<Vec<_>>()
            .join("\n");

        let legacy_started = Instant::now();
        for _ in 0..100 {
            black_box(legacy_required_modules(&source));
        }
        let legacy_elapsed = legacy_started.elapsed();

        let optimized_started = Instant::now();
        for _ in 0..100 {
            black_box(required_modules(&source));
        }
        let optimized_elapsed = optimized_started.elapsed();

        eprintln!(
            "required_modules 100 iterations: legacy={legacy_elapsed:?}, static={optimized_elapsed:?}"
        );
    }

    fn legacy_required_modules(source: &str) -> Vec<String> {
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
            env.set(name, globals.get::<mlua::Value>(name).unwrap())
                .unwrap();
        }
        for name in ["coroutine", "math", "string", "table", "utf8"] {
            env.set(name, globals.get::<mlua::Table>(name).unwrap())
                .unwrap();
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

        assert!(
            verify_trusted(&manifest, source, &vk),
            "valid sig should verify"
        );

        manifest.permissions.push("network".to_string());
        assert!(
            !verify_trusted(&manifest, source, &vk),
            "bad perms should fail"
        );

        manifest.permissions.pop();
        assert!(
            !verify_trusted(&manifest, "-- different source\n", &vk),
            "different source should fail"
        );
    }

    /// Preserves the Lua API shape while replacing external effects with deterministic values.
    fn make_mock_ctx(lua: &mlua::Lua) -> mlua::Table {
        lua.load(
            r#"
            local ctx = {}
            ctx._edits = {}
            function ctx:edit(text)      table.insert(self._edits, tostring(text or "")) end
            function ctx:reply(text)     end
            function ctx:delete()        end
            function ctx:sleep_ms(ms)    end
            function ctx:sleep(s)        end
            function ctx:db_get(key)     return nil end
            function ctx:db_set(k, v)    end
            function ctx:sanitize(t)     return tostring(t or "") end
            function ctx:message_text()  return "" end
            function ctx:replied_text()  return "" end
            function ctx:message_info()  return "Chat ID: 0\nMessage ID: 0" end
            function ctx:module_info()   return {name="test", permissions={}, trusted=true} end
            function ctx:runtime_stats() return {uptime_seconds=0, cpu_percent=0.0} end
            function ctx:now_ms()        return 0 end
            function ctx:uptime_seconds() return 0 end
            function ctx:delete_last_own(n) end
            function ctx:update_project()   end
            function ctx:install_module(src, name) return "ok" end
            function ctx:install_replied_module(name) error("no reply") end
            function ctx:install_plugin(src, name)  return "ok" end
            function ctx:install_replied_plugin(name) error("no reply") end
            function ctx:env_get(key)    return nil end
            function ctx:download_replied_media(name) return "data/test.bin" end
            function ctx:download_url(url, name) return "data/test.bin" end
            function ctx:send_file(path, caption) end
            function ctx:run_term(cmd) end
            function ctx:http_get(url)   return "" end
            function ctx:http_json_get(url) return {} end
            function ctx:http_request(method, url, body, headers) return "" end
            function ctx:http_json_request(method, url, body, headers) return {} end
            function ctx:http_json_multipart_file_request(method, url, field, path, fields, headers) return {} end
            function ctx:prefix()        return "." end
            function ctx:prefixes()      return { "." } end
            function ctx:set_prefixes(v) return v end
            function ctx:lang()          return "en" end
            function ctx:set_lang(c)     return c end
            -- Like the real ctx:cfg: the module's declared default when nothing is stored.
            function ctx:cfg(key)
                local config = self._module and self._module.config or {}
                for _, field in ipairs(config) do
                    if field.key == key then return field.default end
                end
                return nil
            end
            function ctx:cfg_set(k, v)   return v end
            function ctx:help(q)         return "help" end
            function ctx:inline_help(q)  return false end
            function ctx:message()       return { id = 1, chat_id = -100, text = "", is_group = true, is_private = false, is_saved = false, reply_to_id = 5 } end
            function ctx:replied()       return { id = 5, text = "replied", sender_id = 42 } end
            function ctx:answer(t)       end
            function ctx:schedule(s, t, target) end
            function ctx:send_saved(t)   end
            function ctx:pin()           end
            function ctx:unpin()         end
            function ctx:purge()         return 3 end
            function ctx:tag_all(t, n)   return 0 end
            function ctx:react(e)        end
            function ctx:notify(t)       end
            function ctx:db_keys(p)      return {} end
            function ctx:modules_list()  return { { name = "core", version = "2.0", enabled = true, trusted = true, bundled = true, category = "core", description = "", commands = { "ping" }, settings = 1 } } end
            function ctx:module_set_enabled(n, e) end
            function ctx:module_reload(n) end
            function ctx:module_remove(n) end
            function ctx:module_restore(n) return { "core.lua" } end
            function ctx:module_config(n) return { { key = "k", type = "bool", value = "on", default = "off", description = "d", options = {}, secret = false } } end
            function ctx:module_config_set(n, k, v) return "on" end
            function ctx:sudo_list()     return { 1 } end
            function ctx:sudo_set(v)     end
            function ctx:panel_link()    return "http://127.0.0.1:8080/auth/token?t=x" end
            function ctx:restart()       end
            function ctx:check_update()  return { current = "0.2.0", latest = "0.3.0", newer = true, notes = "n", url = "u", has_binary = true, source_checkout = false } end
            function ctx:self_update()   return true end
            function ctx:run_process(p, a) end
            function ctx:backup()        return "backup.tar.gz" end
            function ctx:restore_backup() return { "database.json" } end
            function ctx:account()       return { id = 1, name = "Me", session = "s" } end
            function ctx:account_counts() return { users = 1, bots = 1, groups = 1, channels = 1, contacts = 1, archived = 0, unread = 3 } end
            function ctx:cleanup_preview(o) return { code = "ABC123", contacts = 1, users = 2, bots = 0, groups = 1, channels = 1, kept = 1, owned = 1, total = 5, sample = { "a", "b" } } end
            function ctx:cleanup_run(c)  return "job1" end
            function ctx:account_bulk(a, d) return "job2" end
            function ctx:jobs()          return { { id = "j", title = "T", account = "Me", status = "running", done = 1, total = 2, log = { "x" } } } end
            function ctx:job_cancel(id)  return false end
            function ctx:profile_update(u) end
            function ctx:auto_rules()    return { { id = "a1", name = "Night", enabled = true, type = "away_reply", from = "23:00", to = "08:00" }, { id = "s", name = "S", enabled = false, type = "schedule", every_minutes = 5, chat = "me" } } end
            function ctx:auto_save(r)    r.id = r.id or "new1"; return r end
            function ctx:auto_toggle(i, e) end
            function ctx:auto_delete(i)  end
            function ctx:sha256(t)       return "ba7816bf" end
            function ctx:uuid()          return "00000000-0000-4000-8000-000000000000" end
            return ctx
            "#,
        )
        .eval()
        .expect("mock ctx should evaluate")
    }

    #[tokio::test]
    async fn module_command_handlers_run_with_mock_ctx() {
        let lua = mlua::Lua::new();
        let ui: mlua::Table = lua.load(super::super::UI_LIBRARY).eval().unwrap();
        lua.globals().set("ui", ui).unwrap();
        lua.globals()
            .set("json", super::super::json_library(&lua).unwrap())
            .unwrap();

        // Prevent updater and restart handlers from terminating the test process.
        let os: mlua::Table = lua.globals().get("os").unwrap();
        os.set(
            "exit",
            lua.create_function(|_, _: mlua::Value| Ok(())).unwrap(),
        )
        .unwrap();

        // These commands start external processes that cannot be isolated in this test.
        let skip: std::collections::HashSet<&str> = ["term", "restart"].iter().copied().collect();

        let args_map: std::collections::HashMap<&str, Vec<&str>> = [
            ("type", vec!["", "hello", "hello world"]),
            ("scroll", vec!["", "hello"]),
            ("magic", vec!["", "test"]),
            ("heart", vec!["", "love u"]),
            ("ping", vec![""]),
            ("eval", vec!["", "1 + 1", "return 42"]),
            ("note", vec!["", "get", "set some note", "clear"]),
            ("alias", vec!["", "get x", "set x hello", "del x"]),
            ("del", vec!["", "3"]),
            ("info", vec![""]),
            ("sd", vec!["", "5 test"]),
            ("afk", vec!["", "on away", "off"]),
            ("autoread", vec!["", "on", "off"]),
            ("antidelete", vec!["", "on", "off"]),
            ("cleanjoins", vec!["", "on", "off", "status"]),
            ("captcha", vec!["", "on", "off", "status"]),
            ("group", vec!["", "status"]),
            ("pmguard", vec!["", "on", "off", "status"]),
            ("ip", vec!["", "1.1.1.1"]),
            ("domain", vec!["", "example.com"]),
            ("rdap", vec!["", "example.com"]),
            ("ai", vec!["provider openai"]),
            ("ask", vec!["", "hi"]),
            ("summarize", vec![""]),
            ("translate", vec!["en hi"]),
            ("transcribe", vec![""]),
            ("play", vec!["test"]),
            ("queue", vec![""]),
            ("skip", vec![""]),
            ("stop", vec![""]),
            ("market", vec!["", "search"]),
            ("install", vec!["http://example.com/test.lua"]),
            ("update", vec![""]),
            ("gifts", vec![""]),
            ("taskbot", vec![""]),
            ("dl", vec![""]),
            ("sendfile", vec!["data/test.txt caption"]),
            ("urlupload", vec!["http://example.com/file.txt"]),
            ("rename", vec!["old.txt new.txt"]),
            ("ytdl", vec!["", "http://example.com/video"]),
            ("help", vec!["", "notes"]),
            ("stats", vec![""]),
            ("prefix", vec!["", ". !"]),
            ("lang", vec!["", "ru"]),
            ("modules", vec!["", "co"]),
            (
                "module",
                vec![
                    "",
                    "core",
                    "on core",
                    "off core",
                    "reload core",
                    "remove core",
                    "restore",
                    "on ghost",
                ],
            ),
            (
                "cfg",
                vec!["", "core", "core k", "core k on", "core k reset"],
            ),
            ("sudo", vec!["", "add 5", "del 5", "add"]),
            ("panel", vec![""]),
            ("remind", vec!["", "10m tea", "18:30 call", "bogus text"]),
            ("later", vec!["", "1h hello"]),
            ("filter", vec!["", "hello | hi", "word"]),
            ("unfilter", vec!["", "hello"]),
            ("filters", vec![""]),
            ("welcome", vec!["", "on Hi {mention}", "off", "status"]),
            ("weather", vec!["", "Berlin"]),
            ("cur", vec!["", "100 usd eur", "eur rub"]),
            ("qr", vec!["", "hello"]),
            (
                "calc",
                vec!["", "2 * (3 + 4) ^ 2", "sqrt(16) + pi", "1 +", "os.exit()"],
            ),
            ("paste", vec!["", "text"]),
            ("purge", vec![""]),
            ("pin", vec![""]),
            ("unpin", vec![""]),
            ("tagall", vec!["", "hey"]),
            ("approve", vec!["", "42"]),
            ("block", vec!["", "42"]),
            ("notes", vec![""]),
            ("backup", vec![""]),
            ("acc", vec![""]),
            ("coin", vec![""]),
            ("dice", vec!["", "20", "6 3"]),
            ("random", vec!["", "1 100", "10 1", "a, b, c", "a"]),
            ("countdown", vec!["", "2 go"]),
            ("time", vec!["", "berlin", "Europe/Paris", "atlantis"]),
            ("pass", vec!["", "4", "32"]),
            ("b64", vec!["", "enc hello", "dec aGVsbG8="]),
            ("hash", vec!["", "abc"]),
            ("uuid", vec![""]),
            ("short", vec!["", "https://example.com"]),
            ("id", vec![""]),
            ("hello", vec!["", "Ann"]),
            (
                "todo",
                vec!["", "add milk", "done 1", "done x", "clear", "list"],
            ),
            ("watch", vec!["", "add sale", "del sale", "list"]),
            ("price", vec!["", "bitcoin eur"]),
            (
                "auto",
                vec![
                    "",
                    "on x",
                    "off x",
                    "del x",
                    "every 60 me hi",
                    "every",
                    "daily 09:00 me hi",
                    "daily x",
                    "clock name A {time}",
                    "clock bad",
                    "online",
                    "offline",
                    "presence off",
                ],
            ),
            ("away", vec!["", "off", "23:00-08:00 asleep", "bad"]),
            (
                "cleanup",
                vec![
                    "",
                    "groups channels inactive=30 keep=@a,5",
                    "bogus",
                    "run",
                    "run ABC123",
                    "all revoke admins pinned archived",
                ],
            ),
            ("readall", vec![""]),
            ("archive", vec!["", "10"]),
            ("muteall", vec![""]),
            ("export", vec![""]),
            ("jobs", vec!["", "cancel abc"]),
            ("profile", vec!["", "name A B", "bio hi", "username x"]),
            ("restore", vec![""]),
        ]
        .into_iter()
        .collect();

        let mut errors: Vec<String> = Vec::new();

        // Example modules from the authoring guide must work as documented, too.
        let examples_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/modules");
        let mut example_paths = std::fs::read_dir(&examples_dir)
            .expect("examples/modules should exist")
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "lua"))
            .collect::<Vec<_>>();
        example_paths.sort();
        assert!(example_paths.len() >= 4);

        for path in lua_module_paths().into_iter().chain(example_paths) {
            let module_name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("?")
                .to_string();
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("cannot read {path:?}: {e}"));

            let module: mlua::Table = lua
                .load(&source)
                .set_name(&module_name)
                .eval()
                .unwrap_or_else(|e| panic!("module '{module_name}' load error: {e}"));

            if let Ok(on_message) = module.get::<mlua::Function>("on_message") {
                for event in [
                    r#"return { id = 2, chat_id = -100, text = "hello there", outgoing = false, from_self = false, is_command = false, is_group = true }"#,
                    r#"return { id = 3, chat_id = -100, text = "", action = "join", action_users = { 7, 8 }, sender_id = 7, sender_name = "Ann", chat_title = "G", is_group = true, from_self = false, is_command = false }"#,
                ] {
                    let ctx = make_mock_ctx(&lua);
                    let msg: mlua::Table = lua.load(event).eval().unwrap();
                    if let Err(e) = on_message.call_async::<()>((ctx, msg)).await {
                        errors.push(format!("ERROR {module_name}.on_message: {e}"));
                    }
                }
            }

            let Ok(commands) = module.get::<mlua::Table>("commands") else {
                continue;
            };

            let pairs: Vec<(String, String)> = commands
                .pairs::<String, String>()
                .map(|p| p.expect("commands table must be string→string"))
                .collect();

            for (cmd, handler_name) in pairs {
                if skip.contains(cmd.as_str()) {
                    continue;
                }

                let handler: mlua::Function =
                    match module.get::<mlua::Function>(handler_name.as_str()) {
                        Ok(f) => f,
                        Err(e) => {
                            errors.push(format!("LOAD  {module_name}.{handler_name}: {e}"));
                            continue;
                        }
                    };

                let test_args = args_map
                    .get(cmd.as_str())
                    .map(|v| v.as_slice())
                    .unwrap_or(&["", "test"]);

                for &args in test_args {
                    let ctx = make_mock_ctx(&lua);
                    ctx.set("_module", module.clone()).unwrap();
                    if let Err(e) = handler.call_async::<()>((ctx, args.to_string())).await {
                        errors.push(format!("ERROR {module_name}.{cmd}({args:?}): {e}"));
                    }
                }
            }
        }

        if !errors.is_empty() {
            panic!("{} handler error(s):\n{}", errors.len(), errors.join("\n"));
        }
    }
}
