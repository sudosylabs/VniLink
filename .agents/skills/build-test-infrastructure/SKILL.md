---
name: build-test-infrastructure
description: "Write or change build, packaging, verification and test tasks in build/, and the test drivers and fixtures they run. Use when touching build/, package.json scripts or test automation."
---

# Build and Test Infrastructure

## Shape

The build is TypeScript run directly by Node (type stripping): `node build/run.ts <task> [arguments]`; `--help` lists the tasks. Every `package.json` script is exactly `node build/run.ts <its name>` (a tooling test enforces it); only npm's `preinstall`, `postinstall` and `prepare` differ. `dev` and `test:ui` prepare a checkout first (`build/lib/preLaunch.ts`); the launchers `scripts/code.{sh,bat}` and `scripts/test.{sh,bat}` (after VS Code's) only run those tasks, and `scripts/test-windows.ps1` is the Windows suite `test:windows` runs.

| File | Owns |
|---|---|
| `build/run.ts` | Every task, composites included; lazy module loading, failure reporting, and one `AbortController` wired to SIGINT/SIGTERM |
| `build/lib/tools.ts` | Tasks that are another program's run: tsc, eslint, stylelint, `node --test`, cargo fmt and clippy |
| `build/lib/vscode/` | The provenance manifest (`manifest.ts`), import, prune and reset (`import.ts`, `prune.ts`) and `check:upstream` (`dependencies.ts`) |
| `build/lib/unused.ts` | `check:unused` |
| `build/lib/lock.ts` | Publication locks that a stopped build does not leave stuck |
| `build/prerequisites.json`, `build/lib/prerequisites.ts` | What each task needs per OS, the checks, and the package lists the container, CI and CONTRIBUTING.md read; `doctor.ts` reports them |
| `build/lib/task.ts` | `define`, `series`, `parallel`, `execute`, adapted from VS Code `build/lib/gulp/task.ts` |
| `build/lib/bundles.ts` | esbuild entry points, staged resources, `buildApplication` |
| `build/lib/native.ts` | Addon identity, `compileNative`, `stageNative`, `assertNativeFresh`, and `nativeCargo`/`nativeArtifact` for every Cargo run with the addon's toolchain |
| `build/lib/release.ts`, `package.ts`, `installers.ts` | Release payload, unsigned app assembly, installers |
| `build/lib/package.{darwin,win32,linux}.ts` | Per-platform installer options and artifact extensions |
| `build/lib/verify.ts` | Inventories and checks for every artifact |
| `build/lib/test.ts` | UI suite, Rust tests, Linux container validation |
| `build/lib/processes.ts`, `watch.ts`, `development.ts` | The only classes: owners of processes, watchers and the dev session |

Read VS Code's `build/lib/gulp/task.ts` and its gulpfiles before changing composition; read `test/automation/src` before changing drivers.

## Writing a task

```ts
export async function buildRelease(root: string, signal: AbortSignal, options: ReleaseOptions = {}): Promise<string> {
	const processes = new ProcessRunner(signal);   // stops its processes when `signal` aborts
	try {
		await (options.buildApplication ?? buildApplication)(root, signal, { production: true, ... });
		signal.throwIfAborted();                      // before anything is published
		...
	} finally {
		await processes.stop();
	}
}
```

- **Functions, not classes.** Steps are `async function (root, signal, ...)`. A task made of other tasks runs them with `step('name')` in `run.ts`, which checks each step's prerequisites; inside a task, name composite steps with `define('name', series(...))`; `parallel` waits for every task before reporting a failure. `vnilink/build-classes` rejects new classes outside the allowlist in `eslint.config.mjs`; add to it only for a new owner of OS resources.
- **Cancellation** is the `AbortSignal` from `run.ts`. Pass it down, give every `ProcessRunner` the signal, and check `signal.throwIfAborted()` before publishing output.
- **Tests replace collaborators through options** (`ReleaseOptions.buildApplication`, `InstallerOptions.runBuilder`), never by subclassing or monkey-patching.
- **Publication is atomic.** Build in a temporary sibling, verify, then rename over the output under `acquireLock` (`lock.ts`); never delete another build's files.
- **Failures throw.** A task that cannot finish throws with a message naming the cause; it never sets `process.exitCode` and returns, which a composite would read as success.
- **Imports** name the `.ts` file (`./native.ts`) and use `import type` for types; `build/tsconfig.json` (`erasableSyntaxOnly`, `verbatimModuleSyntax`) rejects syntax Node cannot strip.

## Prerequisites

- A task's needs are declared in `build/prerequisites.json` (every `run.ts` task has an entry; a tooling test enforces it) and checked by `assertTaskReady` before the task runs. A new tool, library or service a task calls goes there, with its Debian, Fedora, macOS or Windows source, not into an ad-hoc check inside the task.
- `prerequisites.ts`, `native.ts`, `preinstall.ts` and `preLaunch.ts` (until it installs) import only Node's own modules: they run before or without `node_modules`, and the Linux image copies `prerequisites.ts` alone. Other build code checks the OS with `build/lib/platform.ts`.
- `postinstall` prepares only idempotent, local state (Rust toolchain and target, branded Electron), skips it via the install-state hash and never fails the install; the task that needs it checks again.
- The CONTRIBUTING.md setup section is generated (`node build/run.ts docs:prerequisites`); edit the manifest instead.

## Build and dev contracts

- One-shot, release and watch builds share `bundleOptions` and `resources`.
- Native artifacts are identified by sources, target triple, profile and macOS minimum (`nativeIdentity`); existence in `out/native` proves nothing. Rust changes need `npm run build` and a restart; Rust is never watched. `build` bundles without typechecking; `check` and `build:release` typecheck.
- Settings for the shipped addon reach only its target triple (`nativeEnvironment`): never export `MACOSX_DEPLOYMENT_TARGET` or global `RUSTFLAGS` to Cargo, because they also reach host build scripts and proc macros, which macOS 27 then refuses to load.
- Renderer reload keeps the engine; main, preload and utility changes need a restart (`development.ts` relaunches on exit code 42).
- Process owners handle spawn errors, exit and signals, and tear down descendants with bounded waits on every OS.
- Release staging keeps native sources and licences for relinking (`releaseSourceFilter`).

## Test contracts

- Browser scenarios live in `test/browser`, are listed in `test/automation/scenarios.ts`, and run with `npm run test:ui -- --scenario <name>`. A scenario builds its subject through an `InstantiationService` with fake services (`test/browser/settingsFixture.ts`).
- Smoke scenarios (`test/smoke/main.ts`) drive the real app through `window.driver`, wait for explicit readiness, and keep logs and screenshots in `.build/test-runs`.
- Tooling tests (`test/tooling/*.test.mjs`) import build and source modules directly; polling has a condition, a bound and a useful timeout message.
- Temporary profiles and work directories are removed on success, failure and interruption.

## Fixtures

- A fixture builds the real view through `ServiceCollection` with fake services, never a global mock or a procedural page factory; production code never imports fixture data.
- It exposes the states worth testing (loading, empty, populated, unavailable, error, recovery) and controls async completion and failure explicitly.
- Shared mounting, services and cleanup live in one fixture per area (`test/browser/settingsFixture.ts`, `DevicePageFixture`); screenshots are taken after explicit settlement with a controlled viewport and theme.

Validate infrastructure changes with the tooling tests, one representative build (`npm run build`) and one UI scenario. `unit-tests` owns check selection; `cross-platform-validation` owns platform prerequisites.
