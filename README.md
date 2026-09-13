# Native GPUI + Wasm harness PoC

A small native issue tracker and completion calendar implementing the boundaries
in [arch.md](../arch.md). Both application views and their behavior come from
separately compiled Rust/Wasm components. The host does not contain either view.

## Run on this Mac

Prerequisites: Rust, the `wasm32-wasip2` target, Xcode's native build tools, and the
adjacent `../gpui-kit` checkout. Dependencies are pinned in `Cargo.lock`.

```sh
cd /Users/ozzy/project/one/harness-poc
rustup target add wasm32-wasip2
./scripts/run.sh
```

`run.sh` builds both plugins and an optimized native host, then creates
`target/Harness POC.app`. Set `POC_PROFILE=debug` for a faster native development build. To launch an
already built app, open that bundle. Quit with Cmd-Q. The native binary can also
be run with `cargo run -p cordis-host`.

Create an issue, edit/save its title, mark it complete, or delete it. The calendar
queries the issues service, groups completions by date, and composes a six-week
grid with month navigation and day details. It uses rows, columns, text, inputs,
and buttons; there is no native calendar primitive.

## Try hot reload

Edit `plugins/calendar/src/lib.rs`, for example its heading or arrangement of
nodes, and save. The watcher builds and compiles the replacement in the background.
The running plugin continues serving during preparation. The host then drains the
affected dependency closure and activates fresh generations. The displayed host
PID stays unchanged. Reload buttons perform replacement from the current artifact.

A Rust compile error reports a build failure and leaves the old generation active.
Fixing the source triggers another build. This watcher is for **trusted local
source development**: Cargo builds are not an untrusted-code sandbox. Runtime
isolation applies to the compiled Wasm components.

`Delay guest 1.5s` lets you type while the guest is waiting. `Test guest budget`
intentionally traps the issues guest by exhausting fuel. The native UI keeps
working; use **Reload issues** to recover the provider and its calendar consumer.
The frozen UI is not a live retired guest: generation checks reject its actions.

## What lives where

| Code | Responsibility |
|---|---|
| `crates/cordis-core` | Generations, committed bindings, publication, owned effects, pending acquisitions and dependent-before-provider recovery guards; no GPUI or Wasmtime dependencies |
| `crates/cordis-wire` | Bounded UI descriptions, actions, manifests and a small typed value-schema language |
| `crates/cordis-wasm` | Fixed WIT bindings, one store/worker per generation, bounded queues, fuel, host-call limits, checked service calls and cancellation |
| `crates/cordis-host` | Native materializer and input entities, SQLite document transactions, artifact loader, replacement, source watching and telemetry |
| `plugins/issues` | Issue behavior, schema-aware data use, service implementation and UI tree |
| `plugins/calendar` | Broker client, date grouping, month navigation, selection and UI tree |
| `wit/plugin.wit` | Actual compiled host/guest ABI |
| `plugins/*/plugin.json` | Application service requirements/provisions, method schemas, view query and requested capabilities |
| `grants.json` | Host-side capability grants, checked separately from manifest requests |

For a first read, follow the host in this order:

1. `crates/cordis-host/src/main.rs` opens the database, starts the controller, and opens the native window.
2. `crates/cordis-host/src/controller.rs` handles actions, build results, and reload requests away from the UI thread.
3. `crates/cordis-host/src/ui.rs` applies snapshots and turns nodes into native controls; its `ui/tests.rs` checks IME and reload behavior.
4. `crates/cordis-host/src/runtime.rs` mounts generations, gathers views, and drains/replaces dependent plugins.
5. `crates/cordis-wasm/src/lib.rs` executes guest calls and enforces the host API; `cordis-core` owns the lifecycle rules it consults.

The materializer is a small new adapter over GPUI Kit's public primitives and
controls. It does not depend on an imaginary public Wasmtime engine in gpui-shell.
All tree nodes are guest data, validated before materialization. Rust source is
compiled at build time, not converted on each frame.

Service names are runtime metadata. The broker validates method inputs and outputs
and resolves calls through committed provider generations. The extension test adds
`new.stats` to two artifact manifests and calls it without a native interface change.
PoC broker services are read-only queries; durable mutation occurs through the
calling plugin's explicitly granted storage API, outside render/service dispatch.

## Persistence and reload semantics

`data/issues.sqlite` contains plugin-namespaced JSON documents in SQLite, with
transactional batches and host database schema version 1. Unload never deletes
these records. Delete is an explicit application action. Unsupported newer host
database versions are rejected rather than silently downgraded.

The native adapter retains edit buffers, revisions, focus/IME handling and control
semantics. An ordinary snapshot supplies **initial** input values and never writes
over an existing native edit buffer. On generation replacement, plain draft text
is copied into a fresh input entity; guest handles and native callbacks are not
reused. Removed input nodes release their state and subscriptions. This PoC does
not expose an application-driven set-value operation.

Replacement compiles the candidate before retirement and retains the prior
compiled artifact in memory. SHA-256 identifies the loaded Wasm bytes in traces.
Consumers drain before their provider resources are recovered. If activation fails,
partial effects are cleaned and the prior artifact is remounted when possible.
Failure of that remount is reported. In-flight host waits observe retirement, and
unresolved admitted work blocks cleanup rather than being treated as finished.

Reload preserves durable records, panel positions and input text. Guest-local
calendar selection/month state resets. It is not heap snapshotting, atomic closure
publication, zero-downtime service switching, or rollback of saved records.

## Validate

```sh
./scripts/verify.sh
```

This builds actual Wasm components, runs headless lifecycle tests, real-component
integration tests, the native GPUI/IME regression, formatting, and Clippy for both native crates and Wasm plugins. A
successful headless test alone is not evidence of native rendering. The running
UI was also exercised through macOS accessibility controls; see
[VALIDATION.md](VALIDATION.md) for the evidence and measurement scope.

The integration tests cover persistence, CRUD, a new service contract, input/output
schema checks, denied storage, stale generations, provider replacement during a
broker call, activation traps, prior-artifact recovery, failed recovery, and fuel
exhaustion. The GPUI test sends actual composition updates through InputState,
redraws a stale snapshot, commits text, and verifies a fresh input after reload.

## Observe

While the application runs:

- `data/telemetry.json`: host PID, guest call count, live effect/generation counts,
  and GPUI draw-duration samples (microseconds).
- `data/trace.json`: lifecycle, artifact hashes, release ordering, guest operation
  durations and build timings. The trace retains a bounded recent history.

Use **Repaint native snapshot (no guest call)** and compare telemetry before and
after. GPUI draw count should increase while guest call count stays unchanged.
Scrolling and accessibility queries likewise use retained native data. Telemetry
reads GPUI's completed draw records without forcing an animation loop. Draw cost
is not end-to-end display latency or an unconditional 60 FPS guarantee.

## Deliberate PoC limits

This implements the agreed two-plugin validation slice, not the full harness:

- One host session and two fixed panel shells; no docking editor or plugin marketplace.
- Small, fully retained lists; no virtualization, incremental tree patches or drag/drop.
- Five UI node types and standard native accessibility; no browser-equivalent vocabulary.
- A small explicit schema language, not arbitrary JSON Schema or dynamic WIT forwarding.
- Host-managed diagnostic pool/lease registrations exercise ownership and teardown;
  there is no real database connection pool exposed to guests.
- No ambient WASI capabilities. Unsupported WASI calls trap; filesystem, networking,
  process execution and unbounded WASI polling are not silently available.
- SQLite durability is exercised locally; distributed storage, multi-user concurrency,
  future plugin data migrations and external side-effect compensation are not implemented.
- No model driver, Git adapter, remote build sandbox or second execution engine.

These omissions do not replace the Wasm boundary, native UI path, service broker,
managed lifecycle, persistence, or hot-reload mechanisms with mocks.
