---
name: memory-leak-audit
description: "Audit and prove resource/lifecycle leaks in TypeScript owners and Rust tasks, native channels, subscriptions and bounded queues."
---

# Resource and Leak Audit

## TypeScript owners

Register owned stores/listeners/subscriptions immediately and make teardown visible through the class lifecycle (`workbench-architecture`). DOM listeners use disposable helpers; intentional lifetime-long listeners require a documented reason. Objects created repeatedly must have a replacement/release policy, not accumulate until process exit.

Use appropriate DisposableStore/MutableDisposable/DisposableMap ownership. One-time events still need cleanup when their expected event never occurs. Handle rejected startup and disposal during awaits; timers, ports, watchers and child processes count as resources, not only DOM nodes.

## Rust/native owners

Trace every handle/subscription map from creation through explicit release, owner loss, revocation, replacement and shutdown. Binary channels and native encoders/processes need failure cleanup as well as successful close. Workers observe cancellation during blocked I/O/backpressure; a stopped parent cannot leave a worker alive by accident.

Avoid Arc cycles; bound queues, histories and caches by their documented policy. Intentional detached work follows its lifecycle contract (`async-cancellation`) rather than being called a leak merely because a transport ended.

## Proof

Identify the retaining/owning path and reproduce growth or non-retirement. Use repeated create/use/close cycles with listener/handle/process counts or comparable heap/native measurements. A heap snapshot cannot prove a native allocation is gone; choose evidence at the owning layer. `heap-snapshot-analysis` covers V8 mechanics.

Fix the owner rather than clearing symptoms on timers. Re-run the same scenario after the fix plus covering behavior checks (`unit-tests`). Report unmeasured suspicions separately from demonstrated leaks.
