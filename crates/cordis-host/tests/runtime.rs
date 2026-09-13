use cordis_host::{runtime::Runtime, storage::Database};
use cordis_wasm::Operation;
use cordis_wire::Action;
use std::{path::PathBuf, sync::Arc};
#[test]
fn real_components_persist_reconnect_and_recover() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let path = std::env::temp_dir().join(format!("cordis-poc-test-{}.sqlite", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let database = Arc::new(Database::open(&path).unwrap());
    let mut runtime = Runtime::new(&root, database).unwrap();
    runtime.mount(runtime.prepare("issues").unwrap()).unwrap();
    runtime.mount(runtime.prepare("calendar").unwrap()).unwrap();
    let issues = runtime.active["issues"];
    let calendar = runtime.active["calendar"];
    runtime
        .action(
            issues,
            Action {
                id: "create".into(),
                value: Some(
                    r#"{"inputs":{"new-title":"Persists across reload"},"today":"2026-09-12"}"#
                        .into(),
                ),
                revision: 1,
            },
        )
        .unwrap();
    runtime
        .action(
            issues,
            Action {
                id: "toggle:1".into(),
                value: Some(r#"{"today":"2026-09-12"}"#.into()),
                revision: 2,
            },
        )
        .unwrap();
    runtime
        .action(
            issues,
            Action {
                id: "save:1".into(),
                value: Some(r#"{"inputs":{"title-1":"Persists across reload"}}"#.into()),
                revision: 3,
            },
        )
        .unwrap();
    runtime
        .action(
            issues,
            Action {
                id: "create".into(),
                value: Some(r#"{"inputs":{"new-title":"temporary test issue"}}"#.into()),
                revision: 4,
            },
        )
        .unwrap();
    runtime
        .action(
            issues,
            Action {
                id: "delete:2".into(),
                value: Some("{}".into()),
                revision: 5,
            },
        )
        .unwrap();
    let documents = runtime
        .request(issues, Operation::Dispatch("list".into(), "{}".into()))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Vec<serde_json::Value>>(&documents)
            .unwrap()
            .len(),
        1
    );
    let views = runtime.views().unwrap();
    assert_eq!(views.len(), 2);
    assert!(
        serde_json::to_string(&views[0].2)
            .unwrap()
            .contains("week-5")
    );
    runtime
        .replace(runtime.prepare("calendar").unwrap())
        .unwrap();
    assert_ne!(runtime.active["calendar"], calendar);
    assert!(
        runtime
            .request(calendar, Operation::Render("[]".into()))
            .is_err()
    );
    for method in ["bad-payload", "unknown-method", "read-private"] {
        assert!(
            runtime
                .request(
                    runtime.active["calendar"],
                    Operation::Dispatch(method.into(), "{}".into())
                )
                .is_err(),
            "{method} must be rejected"
        );
    }
    let replacement = runtime.prepare("issues").unwrap();
    let live_calendar = runtime.active["calendar"];
    let worker = runtime.workers.lock().unwrap()[&live_calendar].clone();
    let call = std::thread::spawn(move || {
        worker.request(Operation::Dispatch("diagnostic-delay".into(), "{}".into()))
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while runtime.core.lock().unwrap().episodes[&runtime.active["issues"]].calls == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "provider call did not start"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    runtime.replace(replacement).unwrap();
    assert!(
        call.join().unwrap().is_err(),
        "retirement must cancel the in-flight host wait"
    );
    let mut faulty = runtime.prepare("issues").unwrap();
    faulty.config = "trap-after-acquire".into();
    let error = runtime.replace(faulty).unwrap_err();
    assert!(
        error.to_string().contains("prior artifact remounted"),
        "{error:#}"
    );
    assert_ne!(runtime.active["issues"], issues);
    assert!(runtime.views().is_ok());
    let trace = runtime.core.lock().unwrap().trace.clone();
    let lease = trace
        .iter()
        .position(|s| s.contains(":released:") && s.ends_with(":lease"))
        .unwrap();
    let pool = trace
        .iter()
        .position(|s| s.contains(":released:") && s.ends_with(":pool"))
        .unwrap();
    assert!(lease < pool);
    let current = runtime.active["issues"];
    let result = runtime.action(
        current,
        Action {
            id: "loop".into(),
            value: None,
            revision: 0,
        },
    );
    assert!(
        result.is_err(),
        "infinite guest computation must exhaust fuel"
    );
    runtime.unload(current).unwrap();
    assert!(
        runtime
            .core
            .lock()
            .unwrap()
            .episodes
            .values()
            .all(|e| e.effects.is_empty())
    );
    drop(runtime);
    let mut restarted = Runtime::new(&root, Arc::new(Database::open(&path).unwrap())).unwrap();
    restarted
        .mount(restarted.prepare("issues").unwrap())
        .unwrap();
    let result = restarted
        .request(
            restarted.active["issues"],
            Operation::Dispatch("list".into(), "{}".into()),
        )
        .unwrap();
    assert!(result.contains("Persists across reload"));
    assert!(result.contains("2026-09-12"));
    restarted.unload(restarted.active["issues"]).unwrap();
    drop(restarted);
    let _ = std::fs::remove_file(path);
}

#[test]
fn new_service_contract_requires_no_host_interface() {
    use cordis_wire::{Contract, Method, Schema};
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut runtime = Runtime::new(
        &root,
        Arc::new(Database::open(std::path::Path::new(":memory:")).unwrap()),
    )
    .unwrap();
    let contract = Contract {
        version: 1,
        methods: std::collections::BTreeMap::from([(
            "count".into(),
            Method {
                input: Schema::Object {
                    fields: Default::default(),
                },
                output: Schema::Integer,
            },
        )]),
    };
    let mut provider = runtime.prepare("issues").unwrap();
    provider.provides.push("new.stats".into());
    provider
        .manifest
        .provides
        .insert("new.stats".into(), contract.clone());
    let mut consumer = runtime.prepare("calendar").unwrap();
    consumer.requires.push("new.stats".into());
    consumer
        .manifest
        .requires
        .insert("new.stats".into(), contract);
    runtime.mount(provider).unwrap();
    runtime.mount(consumer).unwrap();
    let result = runtime
        .request(
            runtime.active["calendar"],
            Operation::Dispatch(
                "forward".into(),
                r#"{"service":"new.stats","method":"count","payload":"{}"}"#.into(),
            ),
        )
        .unwrap();
    assert_eq!(result, "0");
    runtime.unload(runtime.active["issues"]).unwrap();
}

#[test]
fn failed_replacement_and_failed_remount_leave_no_false_success() {
    struct FailingStorage {
        database: Database,
        fail: std::sync::atomic::AtomicBool,
    }
    impl cordis_wasm::Storage for FailingStorage {
        fn execute(&self, owner: &str, operation: &str, payload: &str) -> anyhow::Result<String> {
            anyhow::ensure!(
                !self.fail.load(std::sync::atomic::Ordering::SeqCst),
                "injected storage failure"
            );
            self.database.execute(owner, operation, payload)
        }
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let storage = Arc::new(FailingStorage {
        database: Database::open(std::path::Path::new(":memory:")).unwrap(),
        fail: std::sync::atomic::AtomicBool::new(false),
    });
    let mut runtime = Runtime::new(&root, storage.clone()).unwrap();
    runtime.mount(runtime.prepare("issues").unwrap()).unwrap();
    runtime.mount(runtime.prepare("calendar").unwrap()).unwrap();
    let replacement = runtime.prepare("issues").unwrap();
    storage
        .fail
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let error = runtime.replace(replacement).unwrap_err();
    assert!(
        error.to_string().contains("recovery also failed"),
        "{error:#}"
    );
    assert!(runtime.active.is_empty());
    assert!(runtime.workers.lock().unwrap().is_empty());
    assert!(
        runtime
            .core
            .lock()
            .unwrap()
            .episodes
            .values()
            .all(|e| e.effects.is_empty())
    );
}
