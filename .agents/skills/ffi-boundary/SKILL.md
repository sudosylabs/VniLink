---
name: ffi-boundary
description: "Define and review napi control/data contracts, lossless values, runtime validation, subscription ownership, binary buffers and preserved error semantics."
---

# FFI Boundary

The node bridge converts facade values to caller-owned control transfer shapes and owns native binary channels. Read actual exported APIs in `crates/vnilink-node/src/lib.rs` and `data.rs`; the boundary is not limited to two open functions and JSON execute.

## Values and validation

Use explicit transfer structs/enums for new crossing types and deliberate field-by-field conversions. Do not expose core/native resource objects directly or rely on a core layout staying a caller contract. Facade aliases already present are migration debt, not a precedent for new accidental coupling.

Keep established public key conventions stable; use serde renames for changed/new caller contracts deliberately and mirror them in TS guards. Not every existing reply uses camelCase, so avoid a global casing rewrite. Small bounded integers cross as numbers; 64-bit identities/counts/generations cross losslessly as decimal strings. Validate parsing, range, required fields, unknown extras and semantic limits at the owning boundary; do not coerce malformed data into defaults.

Command limits come from source constants and legitimate payload needs. Reject oversized bodies before parsing. Binary metadata/payload sizes also have bounded contracts. Native buffers have explicit ownership/copy/release semantics; a zero-copy claim requires tracing actual buffer lifetimes and thread boundaries.

## Handles and errors

Opaque subscription/channel IDs stay in bridge-owned state. Creation, read/write, close/release and failure cleanup are explicit. Address session/service work by its generation; release invalidates subsequent operations without resurrecting handles. Counters must handle exhaustion without reusing live identities. Native handle maps and queues remain bounded.

Core/facade errors retain their distinguishable names/reasons through mapping. Invalid boundary representation and unavailable transport are their own conditions, not fabricated domain outcomes. Match structured categories where available; do not depend on human message substrings. `error-doc-voice` owns user presentation and diagnostic separation.

## Review and proof

Trace values in both directions through facade, transfer conversion, runtime validators and consumers. Exercise malformed/oversized/boundary values, wrong/stale owners and closed handles. TS compilation alone cannot prove napi representation or resource cleanup. Follow `engine-bridge`, `unit-tests` and relevant cross-platform checks.
