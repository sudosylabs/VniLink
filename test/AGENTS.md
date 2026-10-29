# Test Infrastructure

Use `unit-tests` for coverage selection and completion, `build-test-infrastructure` for runner/fixture design, and `cross-platform-validation` for environments. A meaningful test asserts externally observable behavior or a contractual invariant, including failure/recovery when relevant.

Current layers are `test/tooling` (Node contracts/tooling), `test/native` (built napi module), `test/browser` (renderer scenarios) and `test/smoke` (Electron orchestration). Rust services are tested two engines at a time on one computer by `crates/vnilink-testkit` (`node build/run.ts test:service <service>`, `bench:service <service>`); with `--peer` the host half runs in `node build/run.ts testkit:agent` on a second computer. Target reusable drivers and lifecycle fixtures belong in `test/automation`; create that infrastructure only as authorized implementation work. Scenarios use it rather than becoming their own application launchers.

Select areas through a maintained runner interface. Existing cross-area calls from `verifyTabs` and temporary runner edits are debt, not the template for new tests. Keep test profiles, permissions, temporary files and secret-storage fixtures isolated from the user's development app.
