# Rust Workspace

## Ownership and dependency direction

Rust remains a reusable library behind Electron. Rust uses structs, traits and modules idiomatically; TypeScript's class preference does not require imitating JavaScript inheritance in Rust.

| Crate | Owns |
|---|---|
| `vnilink` | Domain state, protocol, authorization, connections, service ownership, jobs and backend contracts. |
| `vnilink-app` | Application-facing facade, adapter composition and conversion of application values to core operations. |
| `vnilink-node` | napi entry points, boundary validation/conversion and bridge-owned channel/subscription handles. |
| `vnilink-macos`, `vnilink-windows`, `vnilink-linux` | Native OS backends implementing core/media contracts; availability based on real session capabilities. |
| `vnilink-media`, `vnilink-audio`, `vnilink-hevc` | Specialized processing, encoding/decoding and media-native integration. |
| `vnilink-rename` | Isolated filesystem-native operations used by callers needing their guarantees. |
| `vnilink-log` | The engine's log: the only `tracing` subscriber, its filter (`VNILINK_LOG`, the setting, at runtime), log files and the flight recorder. Other crates record with `tracing` under a `vnilink::trace` target and never install a subscriber. |
| `vnilink-testkit` | Engines on one computer and each service's functional (`tests/<service>.rs`) and performance (`benches/<service>.rs`) suites, with the fakes they share. Never a dependency of a shipped crate. |

The facade composes the core and platform backends; the node bridge calls the facade. Core contracts do not depend on Electron or platform adapter crates. Specialized media dependencies follow their Cargo manifests; avoid cycles or folding unrelated native concerns into them. New bridge commands do not reach into core internals.

A layer earns its place through ownership, conversion, lifecycle or composition. Evaluate suspected redundant wrappers against those responsibilities before consolidating them. Do not add a pass-through service family solely to mirror a directory in VS Code.

## File placement and invariants

- Shared domain logic belongs in topical core modules; native OS implementation belongs in its platform crate. `core/` owns orchestration of shared state and service lifecycles.
- Split modules at responsibility boundaries, not arbitrary line counts. Preserve the owning module's coherent test organization, including registered `src/tests` modules or inline tests.
- New unsafe sites require a local explanation of the safety invariant. Preserve existing unsafe confinement; safe public wrappers own native resource teardown.
- Vendor patches stay under `vendor/`. Read their `VNILINK.md` where present and preserve source/license/provenance requirements; consult manifests and native build inputs before changing toolchains.
- Cross-language values follow `ffi-boundary`; background work follows `async-cancellation`; errors/docs follow `error-doc-voice`.

## Verification

Use `unit-tests` for covering crate checks and `cross-platform-validation` for native changes. A service's behaviour across two engines belongs in its `vnilink-testkit` suite (`node build/run.ts test:service <service>`), its speed in its bench (`bench:service`); unit tests of one module stay beside it. Build/link results do not establish native runtime or hardware support. Per-core detail lives in `crates/vnilink/AGENTS.md`.
