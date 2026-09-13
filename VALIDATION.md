# Validation record — September 12, 2026

This validates the two-plugin PoC against the agreed proof-of-concept scenarios.
It is not a proof of the full architecture or a browser-equivalent UI platform.

## Readability cleanup revalidation

The readability refactor separates native startup, controller orchestration, and UI
materialization; expands action handling; and names calendar state and Wasm limits.
All **12 existing tests** passed after the changes, including real-component CRUD,
persistence, cancellation/recovery, and native IME. Formatting, native Clippy, Wasm
plugin Clippy, shell syntax checks, and the optimized app build passed. The normal
verification script now also lints both plugins for `wasm32-wasip2`.

The rebuilt native app at PID **97862** loaded the existing completed issue, navigated
to October, and reloaded issues plus its calendar dependent without restarting.
The temporary draft survived replacement into fresh input entities and was then
cleared. Native typing and repaint increased draws from **1 to 36** while guest
calls stayed at **7**. Final telemetry shows two active generations and no blocked
generations. No persistent records were changed by this manual smoke test.

Current evidence lives in [readability-cleanup](verification/readability-cleanup/):
[checks](verification/readability-cleanup/automated-tests.log),
[build](verification/readability-cleanup/release-build.log),
[hashes](verification/readability-cleanup/hashes.json), and
[trace](verification/readability-cleanup/trace.json).
The detailed performance figures and original hashes below describe the earlier
validation build; they have not been relabeled as measurements of this refactor.

## Automated checks

`./scripts/verify.sh` completed successfully. It builds both Rust/Wasm components,
runs **12 tests**, checks formatting, and runs Clippy with warnings denied on the
four native crates and their test targets. See
[automated-tests.log](verification/automated-tests.log).

| Tests | Evidence |
|---|---|
| Five core lifecycle tests | Dependent lease before provider pool release; pending acquisition after retirement; retained committed view and rejected stale actions; visible failed cleanup; realm and ownership checks |
| Storage transaction test | Invalid batch rolls back; another plugin namespace cannot read its records |
| Native GPUI regression | Real GPUI draws preserve marked IME text through a stale snapshot; commit succeeds; reload creates a different input entity retaining the committed text; GPUI leak audit passes |
| Main real-component test | Actual Wasmtime issue CRUD, calendar broker query, persistence, stale-generation rejection, schema/capability denial, provider replacement during an in-flight broker call, activation trap recovery, fuel exhaustion and resource cleanup |
| New-service test | A `new.stats` contract is added to artifact metadata and called through the existing broker without a native interface change |
| Failed-recovery test | Both new activation and prior-artifact remount fail under injected storage failure; failure is reported and no worker or owned effect remains |
| ABI preflight test | An empty component compiles but is rejected for missing required exports before it can replace a running plugin |

The build reports an upstream future-compatibility notice for `block 0.1.6`.
Current builds/tests pass; this is not a claim about a future Rust release.

## Running native application

The actual macOS application was operated through accessibility controls and
keyboard input, and its rendered window was inspected.

| Scenario | Observed result |
|---|---|
| Create and complete an issue | Native input/button actions persisted the record and updated the calendar through its broker dependency |
| Process restart | The previously created issue and completion date reappeared from SQLite after launching a new host process |
| Plugin-only UI addition | Adding a new text node at the top of the calendar changed the native layout while optimized host PID **96092** stayed unchanged |
| Source build failure | With an intentional Rust compile error, the prior calendar remained visible and still navigated to October; fixing the source reloaded it |
| Delayed guest | Native typing remained available and its text survived the later guest response |
| Native input across reload | The draft text survived while accessibility exposed fresh input entities for the new generation |
| Accessibility and keyboard | Labels, editable values and full calendar dates were exposed to macOS; Tab moved focus from the input to Create issue, with the native focus ring visible |
| Guest infinite loop | Fuel exhaustion retired the affected closure; native typing and repaint still worked |
| Recovery after trap | Reload issues drained the calendar lease before the issues pool, then restored two active generations at PID **96092** |
| Cached scrolling | Viewport scrolling and accessibility inspection caused a native draw with no extra guest calls |

The temporary compile error and live-added UI node were removed. The final plugin
source and compiled artifacts are back to the validated normal versions. The app
retains one demonstration issue created during validation.

Platform accessibility was checked through macOS's accessibility API and native
control interaction. This was not a spoken VoiceOver audit of every possible
workflow. IME behavior was checked through the real native input API in the GPUI
regression, without changing the user's OS input-source settings.

## Performance evidence

Machine: **Apple M2**, native macOS, optimized Rust host. The plugins remain the
normal development Wasm artifacts. The workload was the six-week calendar and one
persisted issue, plus native typing, keyboard focus and repaint operations.

| Measurement | Result |
|---|---|
| Before native-only interaction | 3 draws, 7 guest calls |
| After five repaint clicks, typing and Tab | 80 draws, **still 7 guest calls** |
| Median GPUI draw duration in that 80-draw sample | **1.758 ms** |
| p95 GPUI draw duration | **8.339 ms** |
| Maximum GPUI draw duration | **8.528 ms** |
| Later sample including diagnostics/recovery | 246 draws; p95 **12.166 ms**, max **13.339 ms** |
| Cached-scroll check | Draw count 246 → 247; guest calls **31 → 31** |
| Live plugin build + component preparation | Calendar addition **1,192 ms**; restoration **682 ms**, with host PID unchanged |

These are GPUI completed-draw costs collected from its profiler, not an inference
from a timer's tick rate. They do not include all OS display/presentation latency,
prove sustained refresh-rate delivery, or establish performance for thousands of
rows. The debug build was substantially slower (p95 43.495 ms in its small sample),
which is why the launcher defaults to an optimized native host.

The measurements support the specific architectural claim: ordinary native
interaction, repaint and cached scrolling do not require a per-frame Wasm call.
They do not make guest-produced per-frame graphics free.

Raw snapshots and lifecycle traces:

- [Optimized baseline](verification/release-before.json) and
  [native-only result](verification/release-after.json).
- [Live UI addition trace](verification/live-addition-trace.json) and
  [telemetry](verification/live-addition-telemetry.json).
- [Guest trap trace](verification/guest-trap-trace.json) and
  [telemetry](verification/guest-trap-telemetry.json).
- [Recovered lifecycle trace](verification/recovered-trace.json) and
  [telemetry](verification/recovered-telemetry.json).
- [Before scrolling](verification/scroll-before.json) and
  [after scrolling](verification/scroll-after.json).
- [Optimized build log](verification/release-build.log).
- [Source/artifact hashes](verification/hashes.json).

## Interpretation

The PoC demonstrates actual Wasm isolation, dynamic native UI composition, a
schema-checked service broker, durable records, managed cleanup ordering,
cancellation, and restart-free plugin replacement. The [README](README.md)
identifies the remaining product scope: larger UI vocabulary, virtualization,
real external providers, future schema migrations, distribution and the full agent
harness. Host-managed pool/lease registrations are diagnostic resources; this does
not validate teardown of arbitrary external connection pools or guest-authored
mutable state.
