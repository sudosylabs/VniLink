---
name: validate-ui-scenario
description: "Validate a VniLink UI change in a real window with repeatable interactions, relevant screenshots, accessibility and platform-aware expectations."
---

# Validate a UI Scenario

1. Define the behavior and relevant visual states from the request/reference: initial/loading, edited/saved, error/recovery, empty/unavailable as applicable. Name expected outcomes before driving the window.
2. Use maintained automation/fixtures and scenario selection (`build-test-infrastructure`, `smoke-tests`). Exercise real interaction and state, not only DOM presence. Wait for readiness and state changes; repeatable runs should not depend on arbitrary delays or the user's existing profile.
3. Inspect the visual changes at relevant narrow/wide widths, themes, zoom and native-control/material presentations. Capture meaningful states and failures with logs/environment; a screenshot alone does not prove persistence, engine effects or keyboard behavior.
4. Check keyboard/focus/semantics under `accessibility`. Distinguish automated DOM assertions, visible inspection and native assistive-technology checks.
5. Restore test app/window/fixture state and release resources. Attach or link useful evidence and report manual or unavailable platform scenarios explicitly.

Code checks establish code properties; screenshots establish inspected appearance; driven behavior establishes interactions exercised. Do not claim one from another. A temporary one-off driver is diagnostic evidence until the behavior is reproducible through maintained infrastructure. Apply `unit-tests` and `cross-platform-validation` for proportional coverage.
