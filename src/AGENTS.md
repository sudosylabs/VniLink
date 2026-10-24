# TypeScript Sources

## Ownership and file placement

Preserve VS Code's separation by responsibility and execution environment. `workbench-architecture` defines the class/service standard; the directory names are not a substitute for it.

| Home | Responsibility |
|---|---|
| `src/base/common`, `src/base/browser` | Reusable primitives and widgets, independent of product services. |
| `src/platform/<service>/common` | Service interfaces, identifiers and contracts usable across processes. |
| `src/platform/<service>/{browser,node,electron-main,electron-utility,electron-browser}` | Implementations in their actual execution environment. These folders map to real processes and stay. |
| `src/code/electron-main` | Application startup, main-process service composition and IPC channel registration. |
| `src/workbench` | Renderer only, so no environment folders: `workbench.ts`/`layout.ts`/`part.ts` (shell), `desktop.main.ts` (core services), `workbench.desktop.main.ts` (imports every service and contribution). |
| `src/workbench/parts/<part>` | Workbench regions such as the titlebar and views part, built on `Part`. |
| `src/workbench/services/<area>` | `<area>Service.ts` holds the contract and decorator; `<area>ServiceImpl.ts` the implementation, which ends with `registerSingleton`. |
| `src/workbench/contrib/<feature>` | A feature's views and its `<feature>.contribution.ts`, which registers pages, actions and workbench contributions. |
| `src/workbench/actions` | Workbench-wide `Action2` commands. |
| `src/main.ts`, `src/renderer.ts`, `src/preload.ts` | Thin entry points and the isolated renderer bridge. |
| `src/typings` | Ambient declarations; runtime contracts belong beside their service. |

A feature is wired by registration, never by editing another feature: Home renders pages from `HomePagesRegistry`, toolbar buttons come from `ViewActionsMenu` items with `when` clauses on context keys (`src/workbench/contextkeys.ts`), and startup work is a `registerWorkbenchContribution2` at the right `WorkbenchPhase`. A new service or contribution file is imported from `workbench.desktop.main.ts`.

Dependencies flow from application/workbench toward platform/base. `common` contracts do not depend on browser or privileged implementations. Browser code consumes the preload/service boundary rather than importing Node or Electron. Consult `build/lint/layers.ts`; exceptions need an architectural reason, not a lint suppression.

## Coding and interaction

Stateful subsystems have class owners, service contracts and explicit lifecycles. Stateless guards and transformations may be functions. See `workbench-architecture`; formatting belongs to `handwriting`.

Use `src/base/common/platform.ts` and `process.ts` for platform checks inside application sources. Build/test host tooling has its own environment detection. Relative TypeScript module imports end in `.js`; CSS imports retain `.css`. Workbench strings use `nls.localize`; structured error semantics follow `error-doc-voice`.

UI work loads `design-philosophy` and `accessibility`, with layout/theming skills as applicable. Real engine and OS state governs availability and permission presentation. Fixture data is isolated to tests, never substituted into production flows.

## Verification

Follow `unit-tests` and, when affected, `cross-platform-validation`. The upstream reference is a VS Code checkout at the ref in `build/vscode-source.json`, wherever the contributor keeps it; the architecture skill says how to verify its revision.
