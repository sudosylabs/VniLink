---
name: workbench-architecture
description: "Design or change TypeScript application/workbench architecture using VS Code class ownership, service composition, lifecycle and contribution patterns."
---

# Workbench Architecture

## Reference before design

Read the user's supplied files. Locate the analogous subsystem in the local VS Code checkout (normally `/Users/hammed/Projects/vscode`) and inspect its owning classes, interfaces, registration and lifecycle, including relevant dependencies. Verify the reference revision against its Git state and `build/vscode-source.json`; a newer local checkout is architectural evidence, not automatically the pinned copy source. If that checkout is unavailable, use a verified upstream revision or available imported sources and state the limitation.

Useful entry points relative to the upstream checkout:

- `src/vs/workbench/browser/workbench.ts` — Workbench startup, services, listeners, rendering and restoration.
- `src/vs/workbench/browser/layout.ts` — shared layout ownership and parts.
- `src/vs/workbench/electron-browser/desktop.main.ts` — DesktopMain composition.
- `src/vs/code/electron-main/app.ts` — CodeApplication main-process lifecycle.

Explain which responsibilities transfer to VniLink and which are editor-specific. Keep scope aligned with the requested product behavior; reading dependencies does not authorize importing an entire editor.

## Class ownership

VniLink TypeScript is strongly object-oriented. Workbench composition, application bootstrap, feature controllers and stateful services belong to cohesive classes with explicit dependencies, lifecycle methods and disposable ownership. A long initializer moved unchanged into `start()` is still tangled composition.

Separate startup/composition, service registration, listener wiring, layout/rendering, restoration and shutdown where they have distinct responsibilities. Views consume injected service contracts rather than creating backend services. Shared services register through the existing instantiation infrastructure. Small widgets may receive explicit values or narrow interfaces; isolation does not require abandoning DI.

Functions remain suitable for pure transformations, type guards, small callbacks and task combinators, including where upstream uses them. Avoid turning this exception into procedural stateful orchestration. Split responsibilities rather than creating shallow manager classes or a service for every helper.

Use `Disposable`, stores and registered listeners to make ownership visible. Account for failed startup, disposal during an await, window reload and repeated initialization. See `async-cancellation` and `memory-leak-audit`.

## Provenance and placement

`src/AGENTS.md` owns placement. Preserve copied upstream infrastructure at full fidelity, retaining license headers. `build/vscode-source.json` identifies pinned copies and explicitly adapted modules. Check its verification tool before edits; adaptations must be intentional, declared and reviewed. Do not refresh hashes simply to bless an unexplained change. Explicit product adaptations may remove unrelated editor behavior while retaining architectural quality.

## Review bar

Before completing, identify the owner of state, the service boundary, registration/composition location and teardown path. Compare the resulting organization to the upstream reference, not just whether it compiles. Existing procedural code is migration debt; remediate only the authorized area and report neighboring debt without spreading it.
