# VniLink Agent Instructions

VniLink is a cross-platform remote-access application. Rust owns identity, pairing, authorization, connections, remote services, media and transfers. Electron TypeScript owns application/window lifecycle, workbench composition, presentation and local integration. The boundary is defined in `crates/AGENTS.md` and `src/AGENTS.md`.

## Standard and analysis

The current repository is not the quality standard. Follow the documented target architecture; identify existing debt without repeating it or silently expanding a task into its remediation. Working behavior and acceptable visuals do not replace architectural quality.

Read paths and reference files supplied by the user before drawing conclusions. Treat examples as evidence of a broader concern, not as its entire scope. For substantial changes or proposals, assess architecture, lifecycle, contracts/security, UX/accessibility, portability and build/test reproducibility; pursue the relevant concerns and explain material exclusions. Separate source evidence, inference and proposals.

VS Code is the TypeScript architectural reference. Read the corresponding upstream subsystem before designing ours. Preserve copied infrastructure at full fidelity; explicit VniLink product adaptations remain valid. Import directories alone do not constitute architectural fidelity. See `workbench-architecture` for reference selection and provenance.

## Repository structure

| Area | Placement rule |
|---|---|
| `src/` | Electron/TypeScript architecture; `src/AGENTS.md` owns its layers. |
| `crates/` | Rust core, facade, bridge, media and native backends; `crates/AGENTS.md` owns the dependency map. |
| `build/`, `scripts/` | Build, packaging and test tasks as TypeScript functions run by `node build/run.ts <task>`; `scripts/` holds only the Windows validation entry point. |
| `test/` | TypeScript contract, native bridge and UI automation; Rust behavior tests remain with their crates. |
| `assets/` | Product artwork and shipped notices/resources, accessed through owning helpers. |
| `vendor/` | Intentional dependency sources/patches with provenance and license obligations. |
| `.cargo/`, `.github/workflows/` | Native toolchain and CI inputs shared with reproducible validation. |
| `out/`, `target/`, `dist/` | Generated artifacts; never a substitute for editing the owning source. |
| `docs/` | Local ignored handoffs/notes under the current repository policy; do not force-track them. |

## Placement and skills

- `src/AGENTS.md` — TypeScript layers and file placement; `workbench-architecture` — how views, services, contributions and IPC are written, with `src/workbench/contrib/devices` as the reference; `recipes` — step lists for common additions.
- `crates/AGENTS.md` — Rust ownership and dependency direction; `engine-bridge` and `ffi-boundary` — control and binary contracts.
- `build/AGENTS.md`, `test/AGENTS.md` — maintained build/test infrastructure; `build-test-infrastructure` — reusable tasks, runners and fixtures.
- `design-philosophy`, `ux-css-layout`, `ux-theming`, `accessibility` — product design and interaction requirements.
- `async-cancellation`, `memory-leak-audit`, `error-doc-voice` — lifecycle, resources and error semantics.
- `unit-tests` — authoritative validation selection and completion policy; `cross-platform-validation` — environments, prerequisites and limits of evidence.
- `commit-organization` — coherent commit boundaries, selective staging and verification of each commit; use when committing work or planning a commit series.

Skill names refer to `.agents/skills/<name>/SKILL.md`. Read the applicable guidance, not every skill as a ritual. New instructions belong to one owner; other documents link to that owner.

## Completion

Use `unit-tests` to select meaningful checks and `cross-platform-validation` for platform-sensitive work. Report actual outcomes, environment and unresolved verification. A local macOS pass, successful cross-compile or CI configuration is not proof of other platforms working. Instruction-only changes validate instructions and references; they do not require application builds.
