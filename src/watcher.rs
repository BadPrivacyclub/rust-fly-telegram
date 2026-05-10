use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::loader::Loader;

/// Watches `dir` for `.lua` file changes and hot-reloads affected modules.
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
                    if path.extension().is_some_and(|e| e == "lua") {
                        info!("reloading {:?}", path);
                        // Unload old version first.
                        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                            loader.unload(stem).await;
                        }
                        if let Err(e) = loader.load_file(&path).await {
                            error!("reload failed for {:?}: {e}", path);
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
