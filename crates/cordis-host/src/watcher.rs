use crate::runtime::{Artifact, prepare};
use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    path::Path,
    sync::mpsc,
    thread,
    time::Duration,
};
use wasmtime::Engine;

pub fn fingerprint(root: &Path, id: &str) -> u64 {
    fn walk(path: &Path, hash: &mut std::collections::hash_map::DefaultHasher) {
        if path.is_dir() {
            let mut entries: Vec<_> = std::fs::read_dir(path)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .collect();
            entries.sort();
            for path in entries {
                walk(&path, hash);
            }
        } else {
            path.hash(hash);
            if let Ok(bytes) = std::fs::read(path) {
                bytes.hash(hash);
            }
        }
    }
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    for path in [
        root.join(format!("plugins/{id}")),
        root.join("wit"),
        root.join("crates/cordis-wire"),
        root.join("grants.json"),
    ] {
        walk(&path, &mut hash);
    }
    hash.finish()
}
pub struct BuildResult {
    pub id: String,
    pub artifact: Result<Artifact, String>,
    pub elapsed_ms: u128,
}
pub fn watch(
    root: std::path::PathBuf,
    engine: Engine,
    ids: Vec<String>,
    send: mpsc::Sender<BuildResult>,
) {
    thread::spawn(move || {
        let mut seen: BTreeMap<_, _> = ids
            .iter()
            .map(|id| (id.clone(), fingerprint(&root, id)))
            .collect();
        loop {
            thread::sleep(Duration::from_millis(400));
            for id in &ids {
                let before = fingerprint(&root, id);
                if seen[id] == before {
                    continue;
                }
                seen.insert(id.clone(), before);
                let start = std::time::Instant::now();
                let artifact = (|| -> anyhow::Result<Artifact> {
                    let output = std::process::Command::new("cargo")
                        .current_dir(&root)
                        .args([
                            "build",
                            "-p",
                            &format!("{id}-plugin"),
                            "--target",
                            "wasm32-wasip2",
                        ])
                        .output()?;
                    anyhow::ensure!(
                        output.status.success(),
                        "source build failed:\n{}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                    prepare(&root, &engine, id)
                })()
                .map_err(|e| format!("{e:#}"));
                if before != fingerprint(&root, id) {
                    continue;
                }
                if send
                    .send(BuildResult {
                        id: id.clone(),
                        artifact,
                        elapsed_ms: start.elapsed().as_millis(),
                    })
                    .is_err()
                {
                    return;
                }
            }
        }
    });
}
