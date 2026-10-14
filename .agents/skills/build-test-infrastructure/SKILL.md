---
name: build-test-infrastructure
description: "Design or change reusable build tasks, watch/dev runners, test drivers and fixtures with reproducible setup, teardown and selectable scenarios."
---

# Build and Test Infrastructure

## Architectural reference

Inspect upstream `build/lib/gulp/task.ts`, `test/automation/src/application.ts`, `code.ts`, `processes.ts` and the relevant unit/smoke entry point. Resolve the upstream checkout as described in `workbench-architecture`. Adopt responsibilities and composition, not an unrelated dependency stack merely because VS Code uses it.

Tasks have names, inputs, outputs and explicit dependencies. Series/parallel composition may use functions; persistent process, watcher and automation state has class owners. Share task definitions, launch options, readiness checks, fixture setup and cleanup across local and CI workflows.

## Build and dev contracts

- One-shot builds and watch mode share bundle/resource definitions. Separate source compilation, native build, artifact staging and application launch; each stage reports its own failure.
- Identify native artifacts by target triple, profile and owning output directory. A module merely existing in `out/native` does not prove freshness. Rebuild/restage after bridge changes through the maintained build entry point; investigate stale binaries before diagnosing contract errors.
- Rust changes require rebuild/restart. Renderer reload preserves the engine; main/preload/utility changes follow their explicit restart lifecycle. Do not add Rust hot reload implicitly.
- Process owners handle spawn errors, exit/signal, restart and shutdown with bounded waits and teardown on failed startup. Descendant cleanup must be verified on each affected OS rather than assuming Unix signals work identically on Windows.
- Build inputs, dependency versions and toolchains are reproducible; native source/licenses and relinking resources remain part of artifact staging where required.

## Test contracts

Reusable Application/driver owners launch isolated profiles, wait for explicit readiness, interact through stable APIs and collect failures. Area scenarios assert behavior and use shared setup/teardown rather than embedding launchers, repeated JS bundles or arbitrary sleeps. Polling has a condition, bound and useful timeout diagnostics; retries do not hide deterministic failures or repeat destructive actions.

Provide a maintained selector by area/name and environment. Selecting a test must not require editing the runner. Independent scenarios must not be hidden under a tabs/icon scenario. Configuration and lifecycle fixtures expose controlled errors, disposal and async completion. Use classes for stateful drivers; ordinary assertion helpers may remain functions.

Store logs/screenshots/traces in a discoverable run output with environment and scenario identity. Restore app/window state and remove temporary profiles on success, failure and interruption. A temporary diagnostic driver is not completion evidence unless the relevant behavior can be rerun through maintained infrastructure.

## Existing debt and completion

Current `build/tasks.mjs` shares bundle/resource inputs, while `test/smoke/main.ts` orchestrates browser bundles and `verifyTabs` calls unrelated areas. These are migration inputs, not proof that the target runner exists. A project-owned Linux test image and shared automation layer must be implemented as scoped infrastructure work; this skill does not pretend they are present.

Validate infrastructure itself: selection/isolation, failure exit, artifact freshness and cleanup, then one representative application scenario. `unit-tests` owns check selection; `cross-platform-validation` owns platform prerequisites.
