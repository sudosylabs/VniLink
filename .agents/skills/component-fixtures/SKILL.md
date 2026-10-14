---
name: component-fixtures
description: "Design isolated, themeable VniLink UI fixtures with injected test services, explicit async states and reproducible screenshot baselines."
---

# Component Fixtures

Keep component ownership/class structure from `workbench-architecture`. Presentational widgets can take explicit values/narrow interfaces; service-driven views use injected fixture implementations through `ServiceCollection`. Test isolation is not a reason to replace the service architecture with global mocks or procedural page factories.

Fixtures define states worth testing: loading, empty, populated, unavailable, validation/error and recovery as applicable. Expose controlled async completion/failure, saved baseline and lifecycle events. Production views never import fixture data or simulate live capabilities.

Co-locate component fixture definitions with their area when integrated discovery supports it; shared mounting, theme/service setup, clocks and cleanup belong in reusable fixture infrastructure. Today browser tests exist, but a generic `.fixture` discovery system is not assumed to exist. Introduce it as scoped tooling work rather than scattering unregistered files.

Capture after explicit settlement, with controlled viewport/theme/platform inputs. Keep OS mocks separate from real native integration evidence. Review visual differences before accepting intentional baseline changes; update them with their owning change. Assertion and baseline scope should be stable user behavior, not incidental markup. Use `validate-ui-scenario` for real-window evidence and `unit-tests` for covering checks.
