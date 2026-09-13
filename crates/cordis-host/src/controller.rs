//! Serializes plugin actions and replacement on a background thread.
//! Artifact preparation runs separately so compilation cannot block native input.
use anyhow::Result;
use cordis_core::Generation;
use cordis_host::{runtime::Runtime, storage::Database};
use cordis_wire::{Action, Node};
use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
};

pub(crate) type Views = Vec<(String, Generation, Node)>;

pub(crate) enum Command {
    Action(Generation, Action),
    Reload(String),
    Candidate(Box<cordis_host::watcher::BuildResult>),
}

pub(crate) struct Update {
    pub views: Option<Views>,
    pub status: String,
}

pub(crate) fn run(
    root: PathBuf,
    storage: Arc<Database>,
    controller: mpsc::SyncSender<Command>,
    commands: mpsc::Receiver<Command>,
    updates: mpsc::Sender<Update>,
) {
    let start = (|| -> Result<Runtime> {
        let mut runtime = Runtime::new(&root, storage)?;
        for id in ["issues", "calendar"] {
            runtime.mount(runtime.prepare(id)?)?;
        }
        Ok(runtime)
    })();
    let mut runtime = match start {
        Ok(runtime) => runtime,
        Err(e) => {
            let _ = updates.send(Update {
                views: None,
                status: format!("Startup failed: {e:#}"),
            });
            return;
        }
    };
    cordis_host::telemetry::start(root.clone(), runtime.core.clone());
    let (builds, built) = mpsc::channel();
    let (loader_root, loader_engine) = runtime.loader();
    cordis_host::watcher::watch(
        loader_root.clone(),
        loader_engine.clone(),
        runtime.active.keys().cloned().collect(),
        builds,
    );
    let forward = controller.clone();
    std::thread::spawn(move || {
        for result in built {
            if forward.send(Command::Candidate(Box::new(result))).is_err() {
                break;
            }
        }
    });
    let initial_views = runtime.views();
    let initial_status = match &initial_views {
        Ok(_) => format!("Host PID {} · two Wasm workers", std::process::id()),
        Err(error) => format!("UI startup failed: {error:#}"),
    };
    let _ = updates.send(Update {
        views: initial_views.ok(),
        status: initial_status,
    });
    for command in commands {
        let result = match command {
            Command::Action(generation, action) => runtime.action(generation, action),
            Command::Reload(id) => {
                let (root, engine) = (loader_root.clone(), loader_engine.clone());
                let controller = controller.clone();
                std::thread::spawn(move || {
                    let start = std::time::Instant::now();
                    let artifact = cordis_host::runtime::prepare(&root, &engine, &id)
                        .map_err(|e| format!("{e:#}"));
                    let _ = controller.send(Command::Candidate(Box::new(
                        cordis_host::watcher::BuildResult {
                            id,
                            artifact,
                            elapsed_ms: start.elapsed().as_millis(),
                        },
                    )));
                });
                continue;
            }
            Command::Candidate(result) => {
                let id = result.id.clone();
                runtime
                    .core
                    .lock()
                    .unwrap()
                    .record(format!("build:{id}:{}ms", result.elapsed_ms));
                result
                    .artifact
                    .map_err(anyhow::Error::msg)
                    .and_then(|candidate| runtime.replace(candidate))
            }
        };
        let status = match result {
            Ok(()) => format!("Ready · host PID {}", std::process::id()),
            Err(e) => format!("{e:#}"),
        };
        let views = runtime.views();
        let status = match &views {
            Ok(_) => status,
            Err(error) => format!("{status}; UI update failed: {error:#}"),
        };
        let _ = updates.send(Update {
            views: views.ok(),
            status,
        });
    }
}
