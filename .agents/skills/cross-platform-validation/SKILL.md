---
name: cross-platform-validation
description: "Validate cross-platform changes from a macOS workstation using Linux containers, native Windows/macOS runners and real desktop or peer tests; distinguish evidence and missing prerequisites."
---

# Cross-Platform Validation

## Choose environments by affected behavior

VniLink targets macOS, Windows and Linux. Inspect the changed dependency/OS branches and `.github/workflows/ci.yml` before selecting checks. Shared bridge, protocol, native build and session changes require evidence beyond the development OS. Platform-only work exercises that backend and relevant shared contracts, without unrelated matrix runs as a ritual.

| Environment | Establishes | Does not establish |
|---|---|---|
| macOS native run | macOS application/native behavior exercised | Windows/Linux behavior |
| Linux container | Linux compilation, portable/native contract tests and headless Electron behavior exercised | Real Wayland portals, GPU availability, physical input/capture/privacy |
| Cross-compile/link | Target compilation, ABI/link compatibility | Target runtime behavior |
| Native Windows CI/VM/laptop | Windows runtime scenarios exercised | Physical hardware/interactive desktop behavior absent from the environment |
| Real desktop and two-device tests | OS permissions, hardware and directed interoperability exercised | Untested OS versions, architectures or backends |

## Preflight and reproducibility

Check availability before promising a result. Record OS/version, architecture, target triple, toolchain/profile, desktop/session type and relevant permissions/devices. Read current Cargo manifests, toolchain files, build scripts and CI rather than copying stale package lists.

- **Linux on macOS:** verify Docker daemon and intended image architecture. Use a maintained project image/runner when available; otherwise identify the missing infrastructure and implement it only within authorized scope. Image definitions should record/pin reproducible base/toolchain inputs. Match CI dependencies, including display libraries, native compilers, D-Bus, secret service and Xvfb for relevant Electron tests. Mount source read-only where feasible and isolate Linux output, dependency/cache and profile directories; do not reuse macOS `node_modules` or native artifacts.
- **Windows:** use the actual target/toolchain from native build inputs (currently GNU/MSYS2). A native runner, VM or reachable laptop is required for Windows execution; record how it will be reached and which tests can run in its desktop session. Availability of CI configuration is not a successful run. Access/setup requiring user action is a named prerequisite, not simulated by changing `process.platform`.
- **macOS:** distinguish synthetic probes, headless checks and interactive permission/hardware behavior. Use temporary test profiles and avoid changing the user's real grants or configuration merely to prepare a test.

Missing environments do not prevent independent implementation or available checks. Finish unaffected work and report exactly what remains unverified. Installing environments, messaging devices or pushing to trigger CI remains subject to the user's scope/authorization.

## Capability and interoperability checks

For affected capabilities, exercise supported and unavailable states, real permission denial/revocation, teardown/reconnect and fallback where applicable. X11 and Wayland are different backends; Windows/macOS versions can change native support. Codec linkage or one probe does not prove every hardware encode/decode path.

For changes affecting peers, select directed host/viewer pairs that cover changed sender and receiver behavior, negotiation and compatibility. Full integration/release validation covers all six cross-OS directions: Mac→Windows, Windows→Mac, Mac→Linux, Linux→Mac, Windows→Linux, Linux→Windows. Record direction and roles explicitly. Container loopback is not a substitute for real device interoperability. Privacy, restart/shutdown and disruptive permission tests need an isolated or authorized target; describe unperformed checks accurately.

## Completion evidence

Follow `unit-tests` for test selection. Report command/scenario, actual outcome and environment, separating compile/link, runtime, hardware and two-device evidence. List ignored/manual tests affecting the change. Respect an explicit user request not to watch CI; report the run link and pending evidence instead of polling to completion.
