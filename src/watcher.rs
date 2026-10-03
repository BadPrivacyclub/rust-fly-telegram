use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::loader::Loader;

pub async fn watch(dir: impl AsRef<Path>, loader: Arc<Loader>) -> Result<()> {
    let dir = dir.as_ref().to_path_buf();
    let (tx, mut rx) = mpsc::channel::<notify::Result<Event>>(32);

    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = tx.blocking_send(res);
    })?;

    watcher.watch(&dir, RecursiveMode::NonRecursive)?;
    info!("watching {:?} for changes", dir);

    while let Some(event) = rx.recv().await {
        match event {
            Ok(ev) if matches!(ev.kind, EventKind::Modify(_) | EventKind::Create(_)) => {
                for path in ev.paths {
                    let reload_path = if path.extension().is_some_and(|e| e == "lua") {
                        Some(path.clone())
                    } else {
                        manifest_module_path(&path)
                    };

                    if let Some(reload_path) = reload_path {
                        info!("reloading {:?}", reload_path);
                        if let Some(stem) = reload_path.file_stem().and_then(|s| s.to_str()) {
                            loader.unload(stem).await;
                        }
                        if let Err(e) = loader.load_file(&reload_path).await {
                            error!("reload failed for {:?}: {e}", reload_path);
                        }
                    }
                }
            }
            Ok(_) => {}
            Err(e) => error!("watcher error: {e}"),
        }
    }

    Ok(())
}

fn manifest_module_path(path: &Path) -> Option<std::path::PathBuf> {
    let file_name = path.file_name()?.to_str()?;
    let module_name = file_name.strip_suffix(".manifest.json")?;
    if !module_name.ends_with(".lua") {
        return None;
    }
    Some(path.with_file_name(module_name))
}
