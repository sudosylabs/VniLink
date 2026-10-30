# Build Infrastructure

Build, packaging, verification and test tasks are TypeScript functions run by `node build/run.ts <task>`; `build-test-infrastructure` describes the layout, how to write a task and the contracts each task keeps. Read it before changing anything here or in `package.json` scripts.

`prerequisites.json` declares what every task needs on each OS; `npm run doctor` reports it and each task checks its own entry before it runs.

Build definitions, resources and native artifact selection are shared by the one-shot, release, watch and test flows. Keep host tooling separate from runtime services and generated `out/`/`target/` artifacts. Formatting and import-layer tools stay deterministic.

`node build/run.ts check:license` (`build/lib/license.ts`) owns the per-file license notices, as `licensing` describes. `build/vscode-source.json` and `node build/run.ts check:upstream` own copied-source provenance; `workbench-architecture` defines permitted upstream adaptations. `build/lint/vscode` holds verbatim VS Code lint rules. Toolchain changes also require `cross-platform-validation`.

The measurement harnesses document themselves at the top of their tasks: the Linux netem harness (`npm run measure:impaired`) in `build/lib/impaired.ts` and `build/lib/netem.ts`, and the glass-to-glass measurement (`npm run measure:latency`) in `build/lib/latency.ts`, each with what its evidence does not cover.
