# TypeScript Sources

## Ownership and file placement

Preserve VS Code's separation by responsibility and execution environment. `workbench-architecture` defines the class/service standard; the directory names are not a substitute for it.

| Home | Responsibility |
|---|---|
| `src/base/common`, `src/base/browser` | Reusable primitives and widgets, independent of product services. |
| `src/platform/<service>/common` | Service interfaces, identifiers and contracts usable across implementations. |
| `src/platform/<service>/{browser,node,electron-main,electron-utility}` | Implementations in their actual execution environment. |
| `src/workbench/browser/parts` | Shared window composition: titlebar, sidebar, view hosting and layout. |
| `src/workbench/services/<service>` | Workbench service contracts and implementations; remote domain policy stays in Rust. |
| `src/workbench/contrib/<feature>/{common,browser,electron-browser}` | Target home for product feature contributions, views and registration. |
| `src/app/electron-main` | Application startup, main-process composition and shutdown. |
| `src/main.ts`, `src/renderer.ts`, `src/preload.ts` | Thin entry points and the isolated renderer bridge. |
| `src/typings` | Ambient declarations; runtime contracts belong beside their service. |

Existing features currently live under `src/workbench/browser/features`. Moving them to `contrib` is a scoped migration, not something every feature edit performs. New areas follow the target layout; changes within existing areas preserve coherent imports until their migration. Do not create duplicate implementations in both homes.

Dependencies flow from application/workbench toward platform/base. `common` contracts do not depend on browser or privileged implementations. Browser code consumes the preload/service boundary rather than importing Node or Electron. Consult `build/lint/layers.mjs`; exceptions need an architectural reason, not a lint suppression.

## Coding and interaction

Stateful subsystems have class owners, service contracts and explicit lifecycles. Stateless guards and transformations may be functions. See `workbench-architecture`; formatting belongs to `handwriting`.

Use `src/base/common/platform.ts` and `process.ts` for platform checks inside application sources. Build/test host tooling has its own environment detection. Relative TypeScript module imports end in `.js`; CSS imports retain `.css`. Workbench strings use `nls.localize`; structured error semantics follow `error-doc-voice`.

UI work loads `design-philosophy` and `accessibility`, with layout/theming skills as applicable. Real engine and OS state governs availability and permission presentation. Fixture data is isolated to tests, never substituted into production flows.

## Verification

Follow `unit-tests` and, when affected, `cross-platform-validation`. The local upstream reference is normally `/Users/hammed/Projects/vscode`; see the architecture skill for how to verify its revision and locate a reference elsewhere.
