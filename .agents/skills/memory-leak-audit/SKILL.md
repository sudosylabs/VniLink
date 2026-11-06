---
name: memory-leak-audit
description: "Find and prove leaks: listeners, disposables, timers, subscriptions, native channels, workers or queues that outlive their owner, and JavaScript memory that grows with repeated work. Use when memory or handle counts grow, when something keeps running after its view, session or service closed, or when reviewing teardown."
---

# Resource and Leak Audit

## TypeScript owners

Register owned stores/listeners/subscriptions immediately and make teardown visible through the class lifecycle (`workbench-architecture`). DOM listeners use disposable helpers; intentional lifetime-long listeners require a documented reason. Objects created repeatedly must have a replacement/release policy, not accumulate until process exit.

Use appropriate DisposableStore/MutableDisposable/DisposableMap ownership. One-time events still need cleanup when their expected event never occurs. Handle rejected startup and disposal during awaits; timers, ports, watchers and child processes count as resources, not only DOM nodes.

## Rust/native owners

Trace every handle/subscription map from creation through explicit release, owner loss, revocation, replacement and shutdown. Binary channels and native encoders/processes need failure cleanup as well as successful close. Workers observe cancellation during blocked I/O/backpressure; a stopped parent cannot leave a worker alive by accident.

Avoid Arc cycles; bound queues, histories and caches by their documented policy. Intentional detached work follows its lifecycle contract (`async-cancellation`) rather than being called a leak merely because a transport ended.

## Proof

Identify the retaining/owning path and reproduce growth or non-retirement. Use repeated create/use/close cycles with listener/handle/process counts or comparable heap/native measurements. A heap snapshot cannot prove a native allocation is gone; choose evidence at the owning layer.

Fix the owner rather than clearing symptoms on timers. Re-run the same scenario after the fix plus covering behavior checks (`unit-tests`). Report unmeasured suspicions separately from demonstrated leaks.

## Heap snapshots

For JavaScript objects that survive teardown, or memory that grows with repeated work: keep the application state, garbage-collection procedure, cycle count and environment the same, and take a snapshot before and after the scenario. A fix compares the same sequence before and after the change. Compare retained size and its growth over repeated cycles, then follow the retaining path to its root (a store, a listener, a map, a closure, a service): that owner is what should release it. A count alone, or one reachable object, is not a leak; account for caches bounded on purpose. Narrow large captures rather than exhausting the computer. V8 snapshots do not see native buffers, Rust allocations, codecs or child processes; measure those at their owner.
