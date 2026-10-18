---
name: build-test-infrastructure
description: "Write or change build, packaging, verification and test tasks in build/, and the test drivers and fixtures they run. Use when touching build/, package.json scripts or test automation."
---

# Build and Test Infrastructure

## Shape

The build is TypeScript run directly by Node (type stripping): `node build/run.ts <task> [arguments]`. `package.json` scripts call it; nothing else is an entry point except `scripts/test-windows.ps1` for Windows CI.

| File | Owns |
|---|---|
| `build/run.ts` | The task table and one `AbortController` wired to SIGINT/SIGTERM |
| `build/lib/task.ts` | `define`, `series`, `parallel`, `execute`, adapted from VS Code `build/lib/gulp/task.ts` |
| `build/lib/bundles.ts` | esbuild entry points, staged resources, `buildApplication` |
| `build/lib/native.ts` | Addon identity, `compileNative`, `stageNative`, `assertNativeFresh` |
| `build/lib/release.ts`, `package.ts`, `installers.ts` | Release payload, unsigned app assembly, installers |
| `build/lib/package.{darwin,win32,linux}.ts` | Per-platform installer options and artifact extensions |
| `build/lib/verify.ts` | Inventories and checks for every artifact |
| `build/lib/test.ts` | UI suite, Rust tests, Linux container validation |
| `build/lib/processes.ts`, `watch.ts`, `development.ts`, `devtools.ts` | The only classes: owners of processes, watchers, the dev session and a socket |

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

- **Functions, not classes.** Steps are `async function (root, signal, ...)`. Name composite steps with `define('name', series(...))`; `parallel` waits for every task before reporting a failure. `vnilink/build-classes` rejects new classes outside the allowlist in `eslint.config.mjs`; add to it only for a new owner of OS resources.
- **Cancellation** is the `AbortSignal` from `run.ts`. Pass it down, give every `ProcessRunner` the signal, and check `signal.throwIfAborted()` before publishing output.
- **Tests replace collaborators through options** (`ReleaseOptions.buildApplication`, `InstallerOptions.runBuilder`), never by subclassing or monkey-patching.
- **Publication is atomic.** Build in a temporary sibling, verify, then rename over the output under a `.lock` directory; never delete another build's files.
- **Imports** name the `.ts` file (`./native.ts`) and use `import type` for types; `build/tsconfig.json` (`erasableSyntaxOnly`, `verbatimModuleSyntax`) rejects syntax Node cannot strip.

## Build and dev contracts

- One-shot, release and watch builds share `bundleOptions` and `resources`.
- Native artifacts are identified by sources, target triple, profile and macOS minimum (`nativeIdentity`); existence in `out/native` proves nothing. Rust changes need `npm run build` and a restart; Rust is never watched.
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
