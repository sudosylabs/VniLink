---
name: fix-ci-failures
description: "Diagnose actual CI failures by platform and stage, reproduce evidenced causes and verify reruns without guessing from macOS or repeatedly running unrelated matrix legs."
---

# Fix CI Failures

Read the current workflow and actual run logs, using available CI tooling (normally `gh`). Identify revision, platform/architecture, failing job/step and exact failure evidence. A workflow file is configuration, not proof that its checks passed.

Classify failure at dependency/environment, build/link, contract/native, window scenario or cleanup. Establish the cause; Linux failures are not presumed environmental and Windows failures are not guessed from macOS. Match reproducible prerequisites under `cross-platform-validation` and use the smallest covering command/scenario from `unit-tests`.

Fix source-of-truth inputs for generated/copied artifacts; formatting/lint violations use their maintained tools. Upstream hash failures require understanding provenance (`workbench-architecture`), not an unconditional hash refresh or restoration that discards intentional product adaptations.

Verify the failing stage locally where feasible, then rerun the relevant native CI leg when needed and authorized. Push only when requested/authorized; this skill does not grant repository mutation permission. Avoid repeated unrelated full-matrix runs while diagnosing one step.

Follow required runs to terminal outcomes when the user requests CI completion; honor an explicit request not to watch. Report actual passed/failed/pending jobs and any hardware/manual checks outside CI. A known recurring failure remains an evidenced gap, not a skipped success.
