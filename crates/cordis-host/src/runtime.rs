use anyhow::{Result, ensure};
use cordis_core::{Core, Generation};
use cordis_wasm::{Operation, Storage, Workers};
use cordis_wire::{Action, Node, decode_ui};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use wasmtime::{Engine, component::Component};

#[derive(Clone)]
pub struct Artifact {
    pub id: String,
    pub hash: String,
    pub manifest: cordis_wire::Manifest,
    pub component: Component,
    pub requires: Vec<String>,
    pub provides: Vec<String>,
    pub storage: bool,
    pub config: String,
}
pub struct Runtime {
    pub core: Arc<Mutex<Core>>,
    pub workers: Workers,
    engine: Engine,
    storage: Arc<dyn Storage>,
    pub active: BTreeMap<String, Generation>,
    artifacts: BTreeMap<String, Artifact>,
    pub root: PathBuf,
}
impl Runtime {
    pub fn new(root: &Path, storage: Arc<dyn Storage>) -> Result<Self> {
        Ok(Self {
            core: Arc::new(Mutex::new(Core::default())),
            workers: Arc::new(Mutex::new(BTreeMap::new())),
            engine: cordis_wasm::engine()?,
            storage,
            active: BTreeMap::new(),
            artifacts: BTreeMap::new(),
            root: root.into(),
        })
    }
    pub fn prepare(&self, id: &str) -> Result<Artifact> {
        prepare(&self.root, &self.engine, id)
    }
    pub fn loader(&self) -> (PathBuf, Engine) {
        (self.root.clone(), self.engine.clone())
    }
    pub fn mount(&mut self, artifact: Artifact) -> Result<Generation> {
        ensure!(
            !self.active.contains_key(&artifact.id),
            "plugin identity already active"
        );
        for (key, contract) in &artifact.manifest.requires {
            ensure!(
                self.active
                    .keys()
                    .any(|id| self.artifacts[id].manifest.provides.get(key) == Some(contract)),
                "service schema mismatch or provider unavailable: {key}"
            );
        }
        let generation = self.core.lock().unwrap().begin(
            &artifact.id,
            "poc-session",
            &artifact.requires,
            &artifact.provides,
        )?;
        self.core
            .lock()
            .unwrap()
            .record(format!("{generation}:artifact:{}", artifact.hash));
        let result = (|| -> Result<()> {
            let worker = cordis_wasm::spawn(
                self.engine.clone(),
                artifact.component.clone(),
                generation,
                self.core.clone(),
                self.workers.clone(),
                self.storage.clone(),
                artifact.storage,
                artifact.manifest.clone(),
            )?;
            self.workers
                .lock()
                .unwrap()
                .insert(generation, worker.clone());
            worker.request(Operation::Activate(artifact.config.clone()))?;
            self.core.lock().unwrap().activate(generation)?;
            Ok(())
        })();
        if let Err(error) = result {
            self.unload(generation)?;
            return Err(error);
        }
        self.active.insert(artifact.id.clone(), generation);
        self.artifacts.insert(artifact.id.clone(), artifact);
        Ok(generation)
    }
    pub fn unload(&mut self, generation: Generation) -> Result<()> {
        let closure = self.core.lock().unwrap().retire(generation)?;
        for id in closure {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !self.core.lock().unwrap().can_recover(id) && std::time::Instant::now() < deadline
            {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            ensure!(
                self.core.lock().unwrap().can_recover(id),
                "recovery guard blocked"
            );
            let worker = self.workers.lock().unwrap().get(&id).cloned();
            if let Some(worker) = &worker {
                // Advisory hook failure cannot prevent host-owned cleanup.
                if let Err(e) = worker.request(Operation::Deactivate) {
                    self.core
                        .lock()
                        .unwrap()
                        .record(format!("{id}:deactivate-error:{e}"));
                }
            }
            let effects: Vec<_> = self.core.lock().unwrap().episodes[&id]
                .effects
                .keys()
                .copied()
                .rev()
                .collect();
            for effect in effects {
                self.core.lock().unwrap().release(id, effect)?;
            }
            self.core.lock().unwrap().finish(id)?;
            if let Some(worker) = worker {
                worker.request(Operation::Stop)?;
            }
            self.workers.lock().unwrap().remove(&id);
            self.active.retain(|_, value| *value != id);
        }
        Ok(())
    }
    pub fn replace(&mut self, candidate: Artifact) -> Result<()> {
        let old_generation = *self
            .active
            .get(&candidate.id)
            .ok_or_else(|| anyhow::anyhow!("plugin not active"))?;
        let old_closure: Vec<Artifact> = {
            let core = self.core.lock().unwrap();
            let mut ids = std::collections::BTreeSet::from([old_generation]);
            loop {
                let previous = ids.len();
                for generation in self.active.values() {
                    if core.episodes[generation]
                        .bindings
                        .values()
                        .any(|g| ids.contains(g))
                    {
                        ids.insert(*generation);
                    }
                }
                if previous == ids.len() {
                    break;
                }
            }
            ids.into_iter()
                .map(|generation| self.artifacts[&core.episodes[&generation].plugin].clone())
                .collect()
        };
        let root_id = candidate.id.clone();
        self.unload(old_generation)?;
        let attempt = (|| -> Result<()> {
            self.mount(candidate)?;
            for dependent in old_closure.iter().skip(1) {
                self.mount(dependent.clone())?;
            }
            Ok(())
        })();
        if let Err(error) = attempt {
            if let Some(generation) = self.active.get(&root_id).copied() {
                self.unload(generation)?;
            }
            for artifact in old_closure {
                self.mount(artifact).map_err(|recovery| {
                    anyhow::anyhow!("replacement: {error}; recovery also failed: {recovery}")
                })?;
            }
            anyhow::bail!("replacement failed; prior artifact remounted: {error}");
        }
        Ok(())
    }
    pub fn request(&self, generation: Generation, operation: Operation) -> Result<String> {
        self.core.lock().unwrap().admit(generation)?;
        let worker = self
            .workers
            .lock()
            .unwrap()
            .get(&generation)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing worker"))?;
        worker.request(operation)
    }
    pub fn action(&self, generation: Generation, action: Action) -> Result<()> {
        self.request(
            generation,
            Operation::Action(serde_json::to_string(&action)?),
        )?;
        Ok(())
    }
    pub fn views(&self) -> Result<Vec<(String, Generation, Node)>> {
        let mut views = Vec::new();
        for (id, generation) in &self.active {
            let input = self.request(
                *generation,
                Operation::Dispatch(self.artifacts[id].manifest.view_query.clone(), "{}".into()),
            )?;
            let context = serde_json::json!({
                "data": serde_json::from_str::<serde_json::Value>(&input)?,
                "today": chrono::Local::now().format("%Y-%m-%d").to_string(),
            });
            let encoded = self.request(*generation, Operation::Render(context.to_string()))?;
            views.push((id.clone(), *generation, decode_ui(&encoded)?));
        }
        Ok(views)
    }
}

pub fn prepare(root: &Path, engine: &Engine, id: &str) -> Result<Artifact> {
    let manifest: cordis_wire::Manifest = serde_json::from_slice(&std::fs::read(
        root.join(format!("plugins/{id}/plugin.json")),
    )?)?;
    ensure!(
        manifest.abi == 1 && manifest.id == id,
        "manifest identity/ABI mismatch"
    );
    ensure!(
        manifest.capabilities.iter().all(|c| c == "storage"),
        "unsupported capability"
    );
    let grants: BTreeMap<String, Vec<String>> =
        serde_json::from_slice(&std::fs::read(root.join("grants.json"))?)?;
    ensure!(
        manifest
            .capabilities
            .iter()
            .all(|cap| grants.get(id).is_some_and(|allowed| allowed.contains(cap))),
        "host grant denied"
    );
    let bytes = std::fs::read(root.join(format!("target/wasm32-wasip2/debug/{id}_plugin.wasm")))?;
    ensure!(bytes.len() <= 8 * 1024 * 1024, "artifact byte limit");
    let hash = format!("{:x}", Sha256::digest(&bytes));
    let component = Component::new(engine, &bytes)?;
    cordis_wasm::validate_component(engine, &component)?;
    Ok(Artifact {
        id: id.into(),
        hash,
        component,
        requires: manifest.requires.keys().cloned().collect(),
        provides: manifest.provides.keys().cloned().collect(),
        storage: manifest.capabilities.iter().any(|c| c == "storage"),
        config: String::new(),
        manifest,
    })
}
