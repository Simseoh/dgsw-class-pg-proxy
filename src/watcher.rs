use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::lua::PluginSources;
use crate::config::PluginsConfig;

/// plugins/ 디렉토리를 감시하여 변경 시 Lua 소스 자동 리로드
pub fn spawn_watcher(
    plugins_cfg: PluginsConfig,
    srcs:        Arc<std::sync::RwLock<PluginSources>>,
) -> Result<()> {
    let (tx, mut rx) = mpsc::channel::<Event>(32);

    let mut watcher = RecommendedWatcher::new(
        move |res: notify::Result<Event>| {
            if let Ok(event) = res {
                let _ = tx.blocking_send(event);
            }
        },
        Config::default().with_poll_interval(Duration::from_millis(500)),
    )?;

    watcher.watch(Path::new(&plugins_cfg.dir), RecursiveMode::Recursive)?;

    tokio::spawn(async move {
        // watcher를 이 스코프에서 유지해야 drop되지 않음
        let _watcher = watcher;
        info!("File watcher started on '{}'", plugins_cfg.dir);

        let mut debounce = tokio::time::interval(Duration::from_millis(300));
        let mut pending = false;

        loop {
            tokio::select! {
                Some(event) = rx.recv() => {
                    use notify::EventKind::*;
                    match &event.kind {
                        Create(_) | Modify(_) | Remove(_) => {
                            // .lua 파일만 반응
                            if event.paths.iter().any(|p| {
                                p.extension().map(|e| e == "lua").unwrap_or(false)
                            }) {
                                pending = true;
                            }
                        }
                        _ => {}
                    }
                }
                _ = debounce.tick() => {
                    if pending {
                        pending = false;
                        match PluginSources::load(&plugins_cfg) {
                            Ok(new) => {
                                *srcs.write().unwrap() = new;
                                info!("Hot-reloaded Lua plugins");
                            }
                            Err(e) => warn!("Plugin reload failed: {}", e),
                        }
                    }
                }
            }
        }
    });

    Ok(())
}
