//! A serialized worker owns each component instance. No engine types escape to UI/core.
use anyhow::{Result, anyhow, ensure};
use cordis_core::{Core, Generation, Phase};
use cordis_wire::{MAX_BYTES, Manifest};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};

wasmtime::component::bindgen!({ path: "../../wit", world: "plugin" });

const GUEST_FUEL_PER_CALL: u64 = 10_000_000;
const HOST_CALLS_PER_ENTRY: usize = 512;
const MAX_GUEST_MEMORY_BYTES: usize = 32 * 1024 * 1024;
const WORKER_QUEUE_CAPACITY: usize = 64;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub trait Storage: Send + Sync {
    fn execute(&self, owner: &str, operation: &str, payload: &str) -> Result<String>;
}
pub type Workers = Arc<Mutex<BTreeMap<Generation, Worker>>>;
pub struct HostState {
    pub generation: Generation,
    pub manifest: Manifest,
    pub core: Arc<Mutex<Core>>,
    pub workers: Workers,
    pub storage: Arc<dyn Storage>,
    pub storage_allowed: bool,
    pub rendering: bool,
    pub dispatching: bool,
    host_calls_remaining: usize,
    limits: StoreLimits,
}
impl HostState {
    fn begin_host_call(&mut self) -> Result<()> {
        ensure!(self.host_calls_remaining > 0, "host-call budget exhausted");
        self.host_calls_remaining -= 1;
        ensure!(!self.rendering, "render cannot invoke host effects");
        Ok(())
    }
}
impl poc::harness::host::Host for HostState {
    fn pause(&mut self, milliseconds: u32) -> Result<(), String> {
        (|| {
            self.begin_host_call()?;
            ensure!(milliseconds <= 2000, "pause budget");
            let deadline = std::time::Instant::now() + Duration::from_millis(milliseconds as u64);
            while std::time::Instant::now() < deadline {
                self.core.lock().unwrap().admit(self.generation)?;
                thread::sleep(Duration::from_millis(5));
            }
            Ok(())
        })()
        .map_err(|e: anyhow::Error| e.to_string())
    }
    fn storage(&mut self, operation: String, payload: String) -> Result<String, String> {
        (|| {
            self.begin_host_call()?;
            ensure!(
                !self.dispatching || operation == "scan",
                "broker service methods are read-only in this POC"
            );
            ensure!(self.storage_allowed, "storage capability denied");
            ensure!(payload.len() <= MAX_BYTES, "payload limit");
            let plugin = self.core.lock().unwrap().episodes[&self.generation]
                .plugin
                .clone();
            self.storage.execute(&plugin, &operation, &payload)
        })()
        .map_err(|e| e.to_string())
    }
    fn call(&mut self, service: String, method: String, payload: String) -> Result<String, String> {
        (|| {
            self.begin_host_call()?;
            ensure!(payload.len() <= MAX_BYTES, "payload limit");
            let contract = self
                .manifest
                .requires
                .get(&service)
                .ok_or_else(|| anyhow!("undeclared service"))?;
            let schema = contract
                .methods
                .get(&method)
                .ok_or_else(|| anyhow!("undeclared method"))?;
            schema.input.check(&serde_json::from_str(&payload)?)?;
            let provider = self
                .core
                .lock()
                .unwrap()
                .binding(self.generation, &service)?;
            // Only declared older-provider edges are callable. No reverse guest callback lane.
            ensure!(provider < self.generation, "reverse/reentrant call denied");
            let worker = self
                .workers
                .lock()
                .unwrap()
                .get(&provider)
                .cloned()
                .ok_or_else(|| anyhow!("provider unavailable"))?;
            let output = worker.request(Operation::Dispatch(method, payload))?;
            schema.output.check(&serde_json::from_str(&output)?)?;
            Ok(output)
        })()
        .map_err(|e| e.to_string())
    }
    fn acquire(&mut self, kind: String) -> Result<u64, String> {
        (|| {
            self.begin_host_call()?;
            ensure!(
                !self.dispatching && matches!(kind.as_str(), "pool" | "lease"),
                "unsupported managed resource"
            );
            let mut core = self.core.lock().unwrap();
            let id = core.acquire_pending(self.generation, &kind)?;
            ensure!(
                !core.commit_acquisition(self.generation, id)?,
                "acquisition cancelled"
            );
            Ok(id)
        })()
        .map_err(|e| e.to_string())
    }
    fn release(&mut self, handle: u64) -> Result<(), String> {
        (|| {
            self.begin_host_call()?;
            self.core.lock().unwrap().release(self.generation, handle)
        })()
        .map_err(|e| e.to_string())
    }
}
#[derive(Clone, Debug)]
pub enum Operation {
    Activate(String),
    Deactivate,
    Dispatch(String, String),
    Render(String),
    Action(String),
    Stop,
}
struct Request {
    operation: Operation,
    reply: mpsc::Sender<Result<String>>,
    admission: Option<Admission>,
}
// Count queued as well as executing requests. A caller timeout must not make
// cleanup believe that the worker has finished using its generation's resources.
struct Admission {
    core: Arc<Mutex<Core>>,
    generation: Generation,
}
impl Drop for Admission {
    fn drop(&mut self) {
        self.core
            .lock()
            .unwrap()
            .episodes
            .get_mut(&self.generation)
            .unwrap()
            .calls -= 1;
    }
}
#[derive(Clone)]
pub struct Worker {
    sender: mpsc::SyncSender<Request>,
    core: Arc<Mutex<Core>>,
    generation: Generation,
}
impl Worker {
    pub fn request(&self, operation: Operation) -> Result<String> {
        match &operation {
            Operation::Activate(s) | Operation::Render(s) | Operation::Action(s) => {
                ensure!(s.len() <= MAX_BYTES, "request byte limit")
            }
            Operation::Dispatch(method, payload) => ensure!(
                method.len() <= 128 && payload.len() <= MAX_BYTES,
                "request limit"
            ),
            _ => {}
        }
        let ordinary = !matches!(operation, Operation::Deactivate | Operation::Stop);
        let admission = if ordinary {
            let mut core = self.core.lock().unwrap();
            if matches!(operation, Operation::Activate(_)) {
                ensure!(
                    core.episodes[&self.generation].phase == Phase::Loading,
                    "not loading"
                );
            } else {
                core.admit(self.generation)?;
            }
            core.episodes.get_mut(&self.generation).unwrap().calls += 1;
            Some(Admission {
                core: self.core.clone(),
                generation: self.generation,
            })
        } else {
            None
        };
        let (reply, rx) = mpsc::channel();
        self.sender
            .try_send(Request {
                operation,
                reply,
                admission,
            })
            .map_err(|e| anyhow!("worker unavailable or queue full: {e}"))?;
        rx.recv_timeout(REQUEST_TIMEOUT)
            .map_err(|e| anyhow!("worker deadline: {e}"))?
    }
}
pub fn engine() -> Result<Engine> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.consume_fuel(true);
    Ok(Engine::new(&config)?)
}
pub fn compile(engine: &Engine, path: &Path) -> Result<Component> {
    Ok(Component::from_file(engine, path)?)
}
pub fn validate_component(engine: &Engine, component: &Component) -> Result<()> {
    let linker = component_linker(engine, component)?;
    PluginPre::new(linker.instantiate_pre(component)?)?;
    Ok(())
}
fn component_linker(engine: &Engine, component: &Component) -> Result<Linker<HostState>> {
    let mut linker = Linker::new(engine);
    Plugin::add_to_linker::<_, wasmtime::component::HasSelf<_>>(&mut linker, |state| state)?;
    // No ambient WASI grants, including blocking poll. Only known host ABI imports.
    for (name, _) in component.component_type().imports(engine) {
        ensure!(
            name.starts_with("wasi:") || name == "poc:harness/host@0.1.0",
            "unsupported import: {name}"
        );
    }
    linker.define_unknown_imports_as_traps(component)?;
    Ok(linker)
}
#[allow(clippy::too_many_arguments)] // Explicit driver dependencies; no hidden globals.
pub fn spawn(
    engine: Engine,
    component: Component,
    generation: Generation,
    core: Arc<Mutex<Core>>,
    workers: Workers,
    storage: Arc<dyn Storage>,
    storage_allowed: bool,
    manifest: Manifest,
) -> Result<Worker> {
    let (sender, receiver) = mpsc::sync_channel::<Request>(WORKER_QUEUE_CAPACITY);
    let (ready, ready_rx) = mpsc::channel();
    let worker_core = core.clone();
    core.lock()
        .unwrap()
        .episodes
        .get_mut(&generation)
        .unwrap()
        .calls += 1;
    let initialization = Admission {
        core: core.clone(),
        generation,
    };
    thread::Builder::new()
        .name(format!("wasm-{generation}"))
        .spawn(move || {
            let setup = (|| -> Result<_> {
                let state = HostState {
                    generation,
                    manifest,
                    core,
                    workers,
                    storage,
                    storage_allowed,
                    rendering: false,
                    dispatching: false,
                    host_calls_remaining: HOST_CALLS_PER_ENTRY,
                    limits: StoreLimitsBuilder::new()
                        .memory_size(MAX_GUEST_MEMORY_BYTES)
                        .instances(8)
                        .tables(8)
                        .build(),
                };
                let mut store = Store::new(&engine, state);
                store.limiter(|s| &mut s.limits);
                store.set_fuel(GUEST_FUEL_PER_CALL)?;
                let linker = component_linker(&engine, &component)?;
                let plugin = Plugin::instantiate(&mut store, &component, &linker)?;
                Ok((store, plugin))
            })();
            drop(initialization);
            let (mut store, plugin) = match setup {
                Ok(x) => {
                    let _ = ready.send(Ok(()));
                    x
                }
                Err(e) => {
                    let _ = ready.send(Err(e));
                    return;
                }
            };
            let mut stopped = None;
            for request in receiver {
                if matches!(request.operation, Operation::Stop) {
                    stopped = Some(request.reply);
                    break;
                }
                let started = std::time::Instant::now();
                let label = match &request.operation {
                    Operation::Activate(_) => "activate",
                    Operation::Deactivate => "deactivate",
                    Operation::Dispatch(..) => "dispatch",
                    Operation::Render(_) => "render",
                    Operation::Action(_) => "action",
                    Operation::Stop => "stop",
                };
                let mut trapped = false;
                let result = (|| -> Result<String> {
                    if matches!(
                        request.operation,
                        Operation::Dispatch(..) | Operation::Render(_) | Operation::Action(_)
                    ) {
                        store.data().core.lock().unwrap().admit(generation)?;
                    }
                    store.set_fuel(GUEST_FUEL_PER_CALL)?;
                    store.data_mut().host_calls_remaining = HOST_CALLS_PER_ENTRY;
                    store.data_mut().rendering = matches!(request.operation, Operation::Render(_));
                    store.data_mut().dispatching =
                        matches!(request.operation, Operation::Dispatch(..));
                    let value = match request.operation {
                        Operation::Activate(config) => plugin
                            .call_activate(&mut store, &config)
                            .inspect_err(|_| {
                                trapped = true;
                            })?
                            .map(|()| String::new()),
                        Operation::Deactivate => plugin
                            .call_deactivate(&mut store)
                            .inspect_err(|_| {
                                trapped = true;
                            })?
                            .map(|()| String::new()),
                        Operation::Dispatch(method, payload) => plugin
                            .call_dispatch(&mut store, &method, &payload)
                            .inspect_err(|_| {
                                trapped = true;
                            })?,
                        Operation::Render(input) => {
                            plugin.call_render(&mut store, &input).inspect_err(|_| {
                                trapped = true;
                            })?
                        }
                        Operation::Action(action) => plugin
                            .call_handle_action(&mut store, &action)
                            .inspect_err(|_| {
                                trapped = true;
                            })?
                            .map(|()| String::new()),
                        Operation::Stop => unreachable!(),
                    }
                    .map_err(|e| anyhow!(e))?;
                    ensure!(value.len() <= MAX_BYTES, "response byte limit");
                    Ok(value)
                })();
                store.data_mut().rendering = false;
                store.data_mut().dispatching = false;
                store.data().core.lock().unwrap().record(format!(
                    "{generation}:guest:{label}:{}us:{}",
                    started.elapsed().as_micros(),
                    if result.is_ok() { "ok" } else { "error" }
                ));
                if trapped {
                    let _ = store.data().core.lock().unwrap().retire(generation);
                }
                drop(request.admission);
                let _ = request.reply.send(result);
            }
            drop(store);
            if let Some(reply) = stopped {
                let _ = reply.send(Ok(String::new()));
            }
        })?;
    ready_rx.recv_timeout(Duration::from_secs(30))??;
    Ok(Worker {
        sender,
        core: worker_core,
        generation,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn compilation_is_not_enough_to_accept_an_incompatible_component() {
        let engine = super::engine().unwrap();
        let empty = wasmtime::component::Component::new(&engine, b"\0asm\x0d\x00\x01\x00").unwrap();
        assert!(super::validate_component(&engine, &empty).is_err());
    }
}
