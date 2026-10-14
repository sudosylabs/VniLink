---
name: smoke-tests
description: "Run and diagnose Electron end-to-end scenarios using isolated profiles, maintained scenario selection, native artifact freshness and platform prerequisites."
---

# Electron Smoke Tests

Confirm current scripts in `package.json`. Today `npm test` builds, runs native tests and launches `out/smoke.cjs`; that is a source-built Electron run, not packaged-distribution verification. Packaging must be tested separately when requested or affected.

## Execution

Use `unit-tests` to choose coverage and `cross-platform-validation` to prepare the environment. Ensure the native addon and renderer resources came from the intended revision/target. Isolate the profile and engine fixture from the user's active app, keychain and grants. Lifecycle/readiness/teardown belong to shared automation (`build-test-infrastructure`).

Run area/name selectors supported by the maintained runner. Do not prescribe editing `main.ts` or hiding unrelated scenarios under `verifyTabs`. If the required selector does not exist, name that infrastructure gap; implement it when in scope or report the limits of the broader available run. Temporary diagnostic drivers must not masquerade as maintained end-to-end coverage.

Collect scenario-specific logs and useful failure screenshots/traces. Wait on observable state with bounded deadlines; timeouts do not prove a leak or environment issue without diagnosis. Restore window state and release engines/channels/profiles on every exit path.

## Platform behavior and diagnosis

Expected controls, native materials, fullscreen transitions, permissions and secret storage depend on the real OS/session. Platform-aware assertions should verify each intended presentation; skipping a branch or changing platform detection is not validation. Use deterministic fixture state for ordinary scenarios and separate real OS integration tests.

Identify the failing build, native or window stage. Reproduce its smallest covering scenario and inspect exact failure evidence. A Linux failure may be application logic or environment; investigate both. A keyring fallback counts as expected only in a scenario explicitly designed to test that fallback; it must not satisfy a real secure-storage integration test.

Re-run relevant coverage after a fix, broadening only under `unit-tests`. Report physical/native manual checks independently from headless success.
