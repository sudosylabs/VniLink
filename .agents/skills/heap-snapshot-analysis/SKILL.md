---
name: heap-snapshot-analysis
description: "Investigate V8 retention with controlled repeated scenarios, comparable snapshots and retainer paths; distinguish JS evidence from native memory."
---

# Heap Snapshot Analysis

Use when JS objects survive lifecycle teardown or repeated work grows retained memory. Keep application state, GC procedure, cycle count and environment comparable. Capture a baseline and post-scenario/post-disposal snapshot; for a fix, compare the same sequence before and after the change. Different workloads are not a useful pair.

Compare retained size and growth over repeated cycles, then follow the retaining path to its root (store, listener, map, closure or service). Identify the owner that should release it. A count alone or a single reachable object is not proof of unintended retention; account for intentional bounded caches.

Narrow large captures or use suitable streaming analysis rather than exhausting the workstation. Record sizes, identities/paths and the reproducible scenario. Do not fix symptoms by periodically clearing caches when their ownership is wrong.

V8 snapshots do not measure all native buffers, Rust allocations, codecs or child processes. Combine evidence at those owners under `memory-leak-audit`. Verify the same retirement scenario after the fix and relevant behavior checks under `unit-tests`.
