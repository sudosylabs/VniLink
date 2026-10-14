# Build Infrastructure

`build-test-infrastructure` defines task composition, process ownership, artifact freshness and reproducible environments. Read it before modifying build/watch/dev tooling. Study the corresponding VS Code build subsystem rather than imposing the workbench's class shape on task combinators.

Build definitions, resources and native artifact selection must be shared by one-shot/watch/test flows. Keep host tooling separate from runtime services and generated `out/`/`target/` artifacts. Formatting and import-layer tools remain deterministic.

`build/vscode-source.json` and its checker own copied-source provenance; `workbench-architecture` defines permitted upstream adaptations. Toolchain changes also require `cross-platform-validation`.
