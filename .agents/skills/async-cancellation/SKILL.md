---
name: async-cancellation
description: "Keep background work owned and cancellable: session and service generations, StaleSession versus NoSession, disposal during an await, AbortSignal and CancellationToken, and safe retry or resume. Use when writing or reviewing work that outlives a call (a task, worker, timer, stream or job), reconnect logic, or anything that acts after an await."
---

# Async Ownership and Cancellation

## Where it lives

- **Rust core:** a session or service is addressed by id and generation, and every action looks it up again before it acts (`runtime(state, id, generation, hosted)` in `crates/vnilink/src/core/desktop_stream.rs` is the pattern). A replaced owner answers `Error::StaleSession`; one that is gone answers `Error::NoSession`.
- **Workbench:** a view registers what it owns, and after an `await` checks `isDisposed()` and its revision before touching state (`DeviceDetails.run` in `src/workbench/contrib/devices/deviceDetails.ts`). Requests that can be abandoned take a `CancellationToken` (`src/base/common/cancellation.ts`).
- **Build tasks:** one `AbortSignal` from `build/run.ts`, passed down to every `ProcessRunner` (`build-test-infrastructure`).

## Owner and generation

Every background operation has an owner and a termination path. Session/service work carries the exact identity and generation required by its core contract; role/direction also travels where IDs can overlap. Connection/service generations fence work, while UI requests may use local revisions or AbortSignals. Do not invent session generations for unrelated configuration/build tasks.

Adopt a newer epoch before acting on it. Old workers stop and their late results cannot mutate current state, enable controls or publish data. Recheck owner/revision after awaits and before observable effects. Startup cancellation, disposal during an operation and repeated calls are first-class paths.

## Cancel, suspend, detach, close

Read current lifecycle enums and owning implementation rather than copying a stale enum example. Suspended records may remain while their old transport workers are cancelled; reconnection creates current-generation work. Close ends ownership with the correct cause. Deliberately detached terminals/jobs may survive under their documented policy/timeouts; that is distinct from accidentally orphaning a worker.

Queues/backpressure waits and native I/O must observe cancellation. Closing an owner releases its resources even when a reader/writer is blocked. Releasing a subscription or channel permanently invalidates that handle; late messages cannot recreate it. See `memory-leak-audit`.

## Retry safety

`StaleSession` identifies an outdated request; `NoSession` identifies absence of the addressed live session. Preserve the distinction. Refresh state before deciding whether to retry; never automatically replay input, filesystem mutations, power commands or other effects into a new generation. Read retries and explicit resumable operations follow their actual contract.

Retries have bounds, a terminal condition and a reason. UI save/apply operations preserve drafts and reconcile observed state on ambiguous completion. Capturing a new revision is not enough if old work can still overwrite the result or release the new owner's handle.

## Proof

Test races through controlled completion/cancellation where feasible: replacement during await, disposal before result, blocked close, duplicate release and reconnect. Assert resulting state and resource retirement, not only that a promise rejected. Use `unit-tests` for scope and native/platform evidence for cancellation relying on OS I/O.
