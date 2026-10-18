# Build Infrastructure

Build, packaging, verification and test tasks are TypeScript functions run by `node build/run.ts <task>`; `build-test-infrastructure` describes the layout, how to write a task and the contracts each task keeps. Read it before changing anything here or in `package.json` scripts.

Build definitions, resources and native artifact selection are shared by the one-shot, release, watch and test flows. Keep host tooling separate from runtime services and generated `out/`/`target/` artifacts. Formatting and import-layer tools stay deterministic.

`build/vscode-source.json` and `node build/run.ts check:upstream` own copied-source provenance; `workbench-architecture` defines permitted upstream adaptations. `build/lint/vscode` holds verbatim VS Code lint rules. Toolchain changes also require `cross-platform-validation`.
