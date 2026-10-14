---
name: async-cancellation
description: "Implement or review async ownership, session generation fencing, disposal, cancellation and safe retry/resume semantics in Rust and TypeScript."
---

# Async Ownership and Cancellation

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
