# Core Crate

The core owns remote-access domain state and orchestration. Modules are topical; native adapters implement its contracts without introducing Electron dependencies.

Enums name lifecycle states and close causes explicitly. Update every affected match when adding a variant. Errors follow `error-doc-voice`; concurrency and owner generations follow `async-cancellation`.

Tests may be inline or in registered modules under `src/tests`, matching the owning area. Add behavioral regressions for changed contracts; use `unit-tests` for selection and `cross-platform-validation` where behavior depends on OS backends or peers.
