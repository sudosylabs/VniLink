---
name: smoke-tests
description: "Run the Electron UI scenarios and validate a UI change in a real window: scenario selection, isolated profiles, screenshots, readiness waits and diagnosis. Use when running or writing `npm run test:ui` scenarios, when one fails, or to confirm a visual or interaction change in the real app."
---

# Electron Smoke Tests

Confirm current scripts in `package.json`. `npm test` builds, runs native tests and launches `out/smoke.cjs` through `node build/run.ts test:ui`; that is a source-built Electron run, not packaged-distribution verification. Packaging must be tested separately when requested or affected.

## Execution

Use `unit-tests` to choose coverage and `cross-platform-validation` to prepare the environment. Ensure the native addon and renderer resources came from the intended revision/target. Isolate the profile and engine fixture from the user's active app, keychain and grants. Lifecycle/readiness/teardown belong to shared automation (`build-test-infrastructure`).

Run area/name selectors supported by the maintained runner. Do not prescribe editing `main.ts` or hiding unrelated checks inside another scenario.

The suite is written as VS Code's is. `test/smoke/src/main.ts` runs mocha in the application's main process; each area is a `describe` named after its scenario in `test/smoke/src/areas/<area>/<area>.test.ts`, one `it` per behaviour, and `--scenario` selects areas as mocha's `grep`. Areas reach the window through page objects in `test/automation/src` (`Automation` holds one per area), which alone know selectors; they call the workbench's typed driver (`src/workbench/services/driver`) through `Code`, which retries until an answer is accepted. A page needs no script of its own: a check that needs something the driver cannot give adds a driver method. Wait for the state that ends a change (`waitForTextContent`, `waitForLayout`, `whenAnimationsFinished`), never for a guessed duration. If the required selector does not exist, name that infrastructure gap; implement it when in scope or report the limits of the broader available run. Temporary diagnostic drivers must not masquerade as maintained end-to-end coverage.

Collect scenario-specific logs and useful failure screenshots/traces. Wait on observable state with bounded deadlines; timeouts do not prove a leak or environment issue without diagnosis. Restore window state and release engines/channels/profiles on every exit path.

## Platform behavior and diagnosis

Expected controls, native materials, fullscreen transitions, permissions and secret storage depend on the real OS/session. Platform-aware assertions should verify each intended presentation; skipping a branch or changing platform detection is not validation. Use deterministic fixture state for ordinary scenarios and separate real OS integration tests.

Identify the failing build, native or window stage. Reproduce its smallest covering scenario and inspect exact failure evidence. A Linux failure may be application logic or environment; investigate both. A keyring fallback counts as expected only in a scenario explicitly designed to test that fallback; it must not satisfy a real secure-storage integration test.

Re-run relevant coverage after a fix, broadening only under `unit-tests`. Report physical/native manual checks independently from headless success.

## Validating a UI change

1. Name the behaviour and the states that matter before driving the window: initial or loading, edited, error and recovery, empty or unavailable.
2. Use maintained scenarios (`npm run test:ui -- --scenario <name>`). Exercise real interaction and state, not only DOM presence, and wait for readiness rather than sleeping.
3. Inspect changed states at narrow and wide widths, both themes and the native materials. Compare against the previous run's screenshots in `.build/test-runs`; a screenshot alone proves neither persistence, engine effects nor keyboard behaviour.
4. Check keyboard, focus and semantics under `accessibility`, separating automated DOM assertions, visual inspection and assistive-technology checks.
5. Restore app, window and fixture state, and report manual or unavailable platform scenarios explicitly.

Code checks establish code properties, screenshots establish appearance, driven behaviour establishes the interactions exercised; do not claim one from another.
