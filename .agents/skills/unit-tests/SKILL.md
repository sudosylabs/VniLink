---
name: unit-tests
description: "Select meaningful covering checks and completion evidence for Rust, TypeScript, native bridge, UI or instruction changes without unnecessary full-suite rituals."
---

# Validation Selection and Completion

This skill owns check selection for the repository. Read current `package.json`, Cargo manifests and `.github/workflows/ci.yml` to confirm executable entry points; a documented command is not evidence that it ran.

## Select by impact

| Change | Covering evidence |
|---|---|
| Pure guards/transforms or tooling | Focused Node tooling tests with meaningful valid/invalid/boundary cases. |
| Core/facade behavior | Owning crate tests with a selector that covers callers, failure and lifecycle paths affected. |
| Bridge shape or binary channel | TS contract checks, Rust/bridge tests, rebuilt staged addon and appropriate native live probes. |
| Workbench behavior/design | Typecheck, relevant browser scenarios and real visual/interaction evidence when changed. |
| Native backend/toolchain | Owning platform checks plus relevant environments from `cross-platform-validation`. |
| Build/test infrastructure | Runner/task failure, selection, freshness and teardown checks plus a representative scenario. |
| Instructions/skills only | Metadata, references, consistency and realistic decision walkthroughs; no application build ritual. |

Run the smallest checks that actually cover the change, plus format/lint/type checks for edited code. Broaden when shared contracts, generated artifacts, dependency changes, new failures or unresolved risks justify it. Before a requested commit/merge/release, satisfy its hook/CI/release gates. Do not equate ordinary task completion with rerunning every layer, and do not omit affected platforms because the host is macOS.

## Meaningful tests

Assert complete behavior or non-obvious invariants. Prefer observable success, failure/recovery, isolation and teardown over snapshots mirroring the implementation. Test changed limits/authorization and stale/disposed work where relevant. Avoid tests added solely to raise counts; reversible low-impact edits may be covered by existing checks.

Rust tests follow the owning area's organization, including inline tests and registered `src/tests` modules. Node validators test exact fields, bounds and rejected extras. Browser scenarios are independent and selectable through maintained infrastructure (`build-test-infrastructure`), not selected by editing a runner or invoking unrelated areas under a tabs test.

## Binary freshness and outcomes

Native tests load the staged addon from `out/native/vnilink.node`. Rebuild/restage through the current build pipeline after Rust/API changes; compiling to another target directory does not refresh the running app. Probe real reads, permitted non-disruptive writes, invalid identifiers/commands and release behavior as applicable.

Separate passed, failed, not run, ignored and manual checks. Record commands/scenarios and environment sufficient to reproduce the result. Identify material pre-existing failures from evidence, not assumptions. Neither abstraction boundaries nor a configured CI matrix proves a platform pass. `cross-platform-validation` owns that distinction.
