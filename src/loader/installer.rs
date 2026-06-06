use std::path::Path;

use grammers_client::media::Media;
use grammers_client::message::Message as FullMessage;

use super::context::Ctx;
use super::manifest;

const MODULE_DOWNLOAD_DIR: &str = "module-downloads";

pub(super) async fn install_module(
    ctx: &Ctx,
    source: String,
    name: Option<String>,
) -> mlua::Result<String> {
    let content = read_module_source(&source).await?;
    let file_name = module_file_name(&source, name.as_deref())?;
    let source_url = if source.starts_with("https://") || source.starts_with("http://") {
        Some(source)
    } else {
        None
    };
    install_module_content(&ctx.modules_dir, &file_name, &content, source_url).await
}

pub(super) async fn install_replied_module(
    ctx: Ctx,
    name: Option<String>,
) -> anyhow::Result<String> {
    let guard = ctx.message.lock().await;
    let Some(msg) = guard.as_ref() else {
        anyhow::bail!("No message context.");
    };
    let Some(reply) = msg.get_reply().await? else {
        anyhow::bail!("Reply to a .lua file or pass a file path/URL.");
    };
    let file_name = replied_module_file_name(&reply, name.as_deref())?;
    let download_path = Path::new("data")
        .join(MODULE_DOWNLOAD_DIR)
        .join(format!("{}-{file_name}", uuid::Uuid::new_v4()));

    if let Some(parent) = download_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    if !reply.download_media(&download_path).await? {
        anyhow::bail!("Replied message does not contain a downloadable file.");
    }

    let content = tokio::fs::read_to_string(&download_path).await?;
    let _ = tokio::fs::remove_file(&download_path).await;
    install_module_content(&ctx.modules_dir, &file_name, &content, None)
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))
}

fn module_file_name(source: &str, name: Option<&str>) -> mlua::Result<String> {
    let raw_name = name
        .filter(|value| !value.trim().is_empty())
        .map(str::trim)
        .or_else(|| {
            Path::new(source)
                .file_name()
                .and_then(|value| value.to_str())
        })
        .ok_or_else(|| mlua::Error::runtime("module name is missing"))?;

    let mut file_name = raw_name.replace('\\', "/");
    if let Some(last) = file_name.rsplit('/').next() {
        file_name = last.to_string();
    }
    if !file_name.ends_with(".lua") {
        file_name.push_str(".lua");
    }
    if file_name.contains("..") || file_name.contains('/') {
        return Err(mlua::Error::runtime(
            "module name must be a plain file name",
        ));
    }

    Ok(file_name)
}

async fn read_module_source(source: &str) -> mlua::Result<String> {
    if source.starts_with("https://") || source.starts_with("http://") {
        return reqwest::get(source)
            .await
            .map_err(|error| mlua::Error::runtime(error.to_string()))?
            .error_for_status()
            .map_err(|error| mlua::Error::runtime(error.to_string()))?
            .text()
            .await
            .map_err(|error| mlua::Error::runtime(error.to_string()));
    }

    tokio::fs::read_to_string(source)
        .await
        .map_err(|error| mlua::Error::runtime(error.to_string()))
}

fn replied_module_file_name(msg: &FullMessage, name: Option<&str>) -> anyhow::Result<String> {
    let source_name = name
        .filter(|value| !value.trim().is_empty())
        .map(str::trim)
        .map(str::to_string)
        .or_else(|| document_file_name(msg));
    let Some(source_name) = source_name else {
        anyhow::bail!("Replied file has no file name.");
    };
    if !source_name.ends_with(".lua") {
        anyhow::bail!("Only .lua modules can be installed.");
    }
    let file_name = module_file_name(&source_name, None).map_err(|error| anyhow::anyhow!(error))?;
    Ok(file_name)
}

fn document_file_name(msg: &FullMessage) -> Option<String> {
    match msg.media()? {
        Media::Document(document) => document.name().map(str::to_string),
        Media::Sticker(sticker) => sticker.document.name().map(str::to_string),
        _ => None,
    }
    .filter(|value| !value.trim().is_empty())
}

async fn install_module_content(
    modules_dir: &Path,
    file_name: &str,
    content: &str,
    source_url: Option<String>,
) -> mlua::Result<String> {
    validate_module_syntax(file_name, content)?;
    validate_module_dependencies(modules_dir, content).await?;

    tokio::fs::create_dir_all(modules_dir)
        .await
        .map_err(|error| mlua::Error::runtime(error.to_string()))?;

    let target = modules_dir.join(file_name);
    tokio::fs::write(&target, content)
        .await
        .map_err(|error| mlua::Error::runtime(error.to_string()))?;
    let manifest_path =
        manifest::write_generated_manifest(modules_dir, file_name, source_url, content).await?;

    let summary = summarize_module(file_name, content);
    Ok(format!(
        "**Module installed**  \nPath: `{}`  \nManifest: `{}`\n\n{}",
        target.to_string_lossy(),
        manifest_path.to_string_lossy(),
        summary
    ))
}

fn validate_module_syntax(file_name: &str, content: &str) -> mlua::Result<()> {
    let lua = mlua::Lua::new();
    lua.load(content)
        .set_name(file_name)
        .into_function()
        .map(drop)
        .map_err(|error| mlua::Error::runtime(format!("Lua syntax error: {error}")))
}

async fn validate_module_dependencies(modules_dir: &Path, source: &str) -> mlua::Result<()> {
    let mut missing = Vec::new();
    for dependency in manifest::required_modules(source) {
        if is_lua_builtin(&dependency) {
            continue;
        }
        let file_path = modules_dir.join(format!("{dependency}.lua"));
        let init_path = modules_dir.join(&dependency).join("init.lua");
        if !file_path.exists() && !init_path.exists() {
            missing.push(dependency);
        }
    }

    if missing.is_empty() {
        Ok(())
    } else {
        Err(mlua::Error::runtime(format!(
            "missing Lua dependencies: {}",
            missing.join(", ")
        )))
    }
}

fn summarize_module(file_name: &str, source: &str) -> String {
    let commands = manifest::module_commands(source);
    let capabilities = manifest::audit_source(source);
    let commands = if commands.is_empty() {
        "**Commands:** `not declared`".to_string()
    } else {
        format!("**Commands:** `{}`", commands.join("`, `"))
    };
    let capabilities = if capabilities.is_empty() {
        "**Capabilities:** `no risky capabilities detected`".to_string()
    } else {
        format!(
            "**Requested permissions:** `{}`  \nReview the generated manifest before granting them.",
            capabilities.join("`, `")
        )
    };
    format!("**Name:** `{file_name}`  \n{commands}  \n{capabilities}")
}

fn is_lua_builtin(name: &str) -> bool {
    matches!(
        name,
        "coroutine" | "debug" | "io" | "math" | "os" | "package" | "string" | "table" | "utf8"
    )
}
