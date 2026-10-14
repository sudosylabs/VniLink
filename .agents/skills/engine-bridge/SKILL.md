---
name: engine-bridge
description: "Expose or diagnose Rust capabilities through Electron control commands and binary data channels with complete facade, validation, service and live-runtime wiring."
---

# Engine Bridge

`crates/vnilink-node` owns the napi boundary. Control commands/replies cross the JSON path; frames, terminal bytes and transfer/media payloads use the owned binary channel path where available. Inspect both before extending or diagnosing a capability. A missing UI operation does not prove the core lacks it.

## Wire a control capability

1. Identify its core owner and application facade method/types in `vnilink-app`. Business policy stays in Rust; facade composition and boundary conversion do not repeat it in TypeScript.
2. Add the appropriate command/reply and caller-owned transfer conversion. Follow `ffi-boundary` for keys, integer representation, handles and errors; review conversion field by field.
3. Dispatch through the owning command family. Maintain exhaustive routing and exact `CommandShape` entries, including unit commands that serde otherwise accepts with extra fields. No catch-all arm conceals unwired variants.
4. Mirror command/reply types and runtime guards in `src/platform/engine/common/engine.ts`. Require complete fields, rejected extras and both representation/semantic bounds. IDs alone do not address session-scoped work (`async-cancellation`).
5. Integrate the renderer-facing service and registration following `workbench-architecture`. UI controls read real saved/runtime/capability state and handle errors, unavailable engines and disposal. A bridge enum addition is not completed end-to-end wiring.
6. Extend the owning contract tests (currently `test/tooling/engine.test.mjs` and area tests) and appropriate Rust/native behavior tests. Exercise failure/bounds as well as a valid shape.

For writes, define readback or observable effect, persistence ownership, when defaults take effect, and rollback/reconciliation on failure. Explicit codecs are preferences requiring negotiated support, not a reason for the UI to invent codec availability. Default settings and live session overrides remain separate contracts.

## Binary capabilities

Inspect `crates/vnilink-node/src/data.rs`, utility-process channel ownership and `src/platform/engine/common/dataChannel.ts`. Reuse owned channels for high-volume bytes; avoid routing frames/blobs through base64 JSON for convenience. Validate metadata/commands, preserve generations and bound buffers/backpressure. Owner/window/session close tears down native handles and MessagePorts; late events cannot revive a released owner.

## Verify

Follow `unit-tests`: build/restage the actual addon, typecheck/contract tests and live probes of reads, safe writes, invalid/unknown inputs and release behavior. Inspect the running module, not just the compiled source. `cross-platform-validation` applies to native/backend/transport changes. A mock reply tests caller handling; it does not demonstrate integration.
