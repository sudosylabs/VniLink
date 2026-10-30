# Build Infrastructure

Build, packaging, verification and test tasks are TypeScript functions run by `node build/run.ts <task>`; `build-test-infrastructure` describes the layout, how to write a task and the contracts each task keeps. Read it before changing anything here or in `package.json` scripts.

`prerequisites.json` declares what every task needs on each OS; `npm run doctor` reports it and each task checks its own entry before it runs.

Build definitions, resources and native artifact selection are shared by the one-shot, release, watch and test flows. Keep host tooling separate from runtime services and generated `out/`/`target/` artifacts. Formatting and import-layer tools stay deterministic.

`node build/run.ts check:license` (`build/lib/license.ts`) owns the per-file license notices, as `licensing` describes. `build/vscode-source.json` and `node build/run.ts check:upstream` own copied-source provenance; `workbench-architecture` defines permitted upstream adaptations. `build/lint/vscode` holds verbatim VS Code lint rules. Toolchain changes also require `cross-platform-validation`.

`IMPAIRED_NETWORK.md` documents the Linux netem measurement harness (`npm run measure:impaired`), its profiles and what its evidence does not cover. `LATENCY.md` documents the glass-to-glass measurement (`npm run measure:latency`) and the limits of its evidence. `PERCEIVED.md` documents perceived quality read from a Mac's screen (`npm run measure:perceived`): that Mac's own floor by default, and remote desktop applications compared as black boxes with an optional second computer.
