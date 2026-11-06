---
name: fix-ci-failures
description: "Diagnose and fix a failing GitHub Actions run: read the failing job's log, place it by platform and stage, reproduce it locally or in the container, and fix the cause. Use when CI is red, a check fails on a pull request, or someone asks why a run failed."
---

# Fix CI Failures

## Read the failure

`.github/workflows/ci.yml` calls one workflow per OS: `test-macos.yml`, `test-linux.yml` (Ubuntu and Fedora, both running `.github/actions/linux-suite`), `test-windows.yml` and `test-linux-container.yml`. `supply-chain.yml` audits dependencies and `release.yml` builds installers; each fails for its own reasons.

```sh
gh run list --workflow ci.yml --branch <branch> --limit 5
gh run view <run-id> --json jobs --jq '.jobs[] | "\(.conclusion) \(.name)"'
gh run view <run-id> --log-failed | grep -E "FAILED|panicked|not ok|failed with exit"
```

Name the commit, the job, the step and the first failing test or command. A green workflow file proves nothing; only the run's log does.

## Place and reproduce it

| Failed in | Reproduce with |
|---|---|
| `npm run check` (lint, types, format, hygiene, license, tooling tests) | The same command on any OS |
| `check:rust` (rustfmt, Clippy) | `npm run check:rust`; Linux-only code needs the container |
| `test:rust` on Linux | `npm run test:linux`, the Debian image the container job runs |
| `test:rust` on Windows | `npm run test:windows` on a Windows computer (`scripts/test-windows.ps1`) |
| UI scenarios | `npm run test:ui -- --scenario <area>` on that OS (`smoke-tests`) |
| Packages or the doctor | `npm run doctor -- --require <tasks>`; packages come from `build/prerequisites.json` |

A test that times real seconds can fail under a full suite's load and pass alone. Run it alone several times (`node build/run.ts test:stress <service> --times 10 --load` for a test-kit suite) before calling it flaky, and name it as flaky in the report rather than skipping it. Windows failures are diagnosed from Windows logs and runs, Linux ones from Linux: a macOS pass says nothing about either.

## Fix the cause

Fix the source a generated file comes from, and formatting with its tool. An upstream-hash failure (`check:upstream`) means a VS Code copy changed: record a reviewed adaptation (`workbench-architecture`) rather than refreshing the hash. Verify the failing step locally, then let CI run again; pushing or rerunning CI is done only when the user asks. Report each job's actual outcome, and what CI cannot cover (real hardware, two computers).
