# VniLink Guidance Rewrite Coverage

This map records the accepted October 2026 instruction scope. It is not a runtime verification report or a specification authorizing the migrations listed below. Paths are repository-relative; upstream paths are relative to the VS Code checkout resolved by `workbench-architecture`.

| Concern and source evidence | Authoritative owner | Intended result / remaining migration |
|---|---|---|
| Workbench composition versus upstream `src/vs/workbench/browser/workbench.ts`; main bootstrap versus CodeApplication/DesktopMain | `workbench-architecture`; `src/AGENTS.md` placement | Done: `VniLinkApplication`, `DesktopMain`/`Workbench`, registries, contributions and schema-driven settings. |
| Build, packaging and test tasks in `build/lib`, run by `build/run.ts` | `build-test-infrastructure`; `build/AGENTS.md` | Done: TypeScript functions composed with `define`/`series`/`parallel`; classes only for OS-resource owners, enforced by `vnilink/build-classes`. |
| Cross-area calls in `test/browser/tabs.ts`, smoke's monolithic orchestration | `build-test-infrastructure`; `test/AGENTS.md` | Selectable independent scenarios, shared automation and fixtures. Selector/driver implementation remains required. |
| Windows-control and removed-sidebar assumptions in `test/smoke/main.ts` | `smoke-tests` | Platform-aware behavioral expectations; stale tests need separate implementation fixes. |
| CI matrix/dependencies and missing project Linux image/runner | `cross-platform-validation` | Explicit Docker/native Windows/hardware prerequisites and evidence; provision a maintained runner in scoped work. |
| CI has no explicit core/facade suite entry | `unit-tests`; `cross-platform-validation` | Select all affected test owners and satisfy integration gates; workflow remediation is not performed by this rewrite. |
| Rust/Electron facade, native adapter and media dependency boundaries | `crates/AGENTS.md`; `engine-bridge`; `ffi-boundary` | Ownership-based placement and complete caller contracts, no accidental wrapper proliferation. |
| Binary APIs exist in node `data.rs`, despite JSON-only/deferred wording | `engine-bridge`; `ffi-boundary`; `cpu-profile-analysis` | Accurate control/data-path guidance, bounded ownership/backpressure and measurement. |
| User corrections about active-tab continuity, previews, spacing, icons, native materials, defaults and truthful UI | `design-philosophy`, `ux-css-layout`, `ux-theming` | Concrete product direction and stateful behavior, not templates derived from current UI. |
| Missing full theme/accessibility foundations | `ux-theming`; `accessibility` | Target standards plus explicit prerequisite implementation; no claim that absent systems already work. |
| Async stale/absent distinction and effects replay risk | `async-cancellation`; `memory-leak-audit` | Correct generation fencing, retry safety and complete retirement; tests prove races/resources. |
| Raw error strings versus useful localized recovery | `error-doc-voice` | Preserved categories with user-appropriate presentation and separate diagnostic evidence. |
| Contradictory full-suite/inline-test/temporary-runner requirements | `unit-tests` | One proportional completion policy and existing coherent test placement; no platform success inferred from abstractions. |
| Reviews/profiling/heap/CI claiming evidence too broadly | Their respective skills, routed to validation owners | Reproducible scenarios, actual outcomes and bounded claims. |
| Earlier replies did not read referenced files and overfit examples | Root AGENTS; `feedback-learning`; `update-instructions` | Evidence-first analysis covering relevant concerns before proposing/implementing. |

## Decision walkthroughs for validation

- **New settings page:** inspect upstream ownership, use class/services and real state, choose global/default scope, apply UI/accessibility rules, verify persistence/failure/focus/visuals. Do not import missing editor infrastructure blindly or run unrelated suites as a ritual.
- **New codec/backend:** trace facade and control/binary contracts, separate preference/support, rebuild the native module, select affected Rust/bridge tests and Linux/native Windows/hardware evidence. A successful Mac probe cannot complete a cross-platform claim.
- **Titlebar adaptation:** preserve native controls/geometry and tab/content continuity, consult provenance before copying/editing, test right-side-control platforms and zoom. Architectural fidelity and explicit product adaptations are compatible.
- **Build/test change:** shared named stages and stateful owners, selectable independent scenarios, failure cleanup and target-isolated artifacts. Temporary runner edits do not establish reproducible infrastructure.
- **Permission/remote action:** observe real OS state and actual support, fence stale operations, test denial/revocation/recovery and use isolated authorized targets for disruptive actions. A fixture switch is not OS integration proof.
- **Instruction-only change:** validate metadata, references, target/current distinctions and scope. Do not build or claim to fix application debt.
