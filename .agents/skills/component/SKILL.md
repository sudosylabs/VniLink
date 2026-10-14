---
name: component
description: "Specify a new service, feature, bridge family or crate through purpose, ownership, class/contracts, lifecycle, platform prerequisites and verification before implementing uncertain seams."
---

# Component Design Contract

Use for a substantial new component or an uncertain architectural boundary. Read the originating request/specification and analogous upstream implementation before choosing shape. Existing code is not the target standard; placement follows layer AGENTS and architecture follows `workbench-architecture`/Rust ownership.

## Define the component

- **Purpose and scope:** user-visible capability, state/operations owned, neighboring owners and necessary supporting work. Distinguish global settings, per-device overrides and runtime actions.
- **Architecture:** layer/home, owning classes or Rust structs/traits, injected contracts, registration/composition and dependency direction. Include native/facade/bridge responsibilities where relevant. A class wrapper around a procedural initializer is not a complete design.
- **Contracts:** input/output shapes, observable effects, availability/permissions, persistence/readback, error categories and compatibility. `engine-bridge` and `ffi-boundary` cover crossing contracts.
- **Lifecycle:** creation/startup, async fences, replacement, cancellation, resource ownership and disposal, including failure paths. See `async-cancellation` and `memory-leak-audit`.
- **Presentation:** relevant design references, interaction states, keyboard/accessibility, theme/material behavior and responsive placement.
- **Verification/prerequisites:** meaningful covering scenarios, required native/container/hardware/peer environments, reproducible setup and the limits of unavailable evidence. Use `unit-tests`, `cross-platform-validation` and `build-test-infrastructure`.

State assumptions and unresolved decisions that affect implementation; investigate them or obtain required input instead of burying guesses in code. Keep the specification proportionate to the component; a fixed paragraph count does not determine its boundaries.

## Deliver according to the request

A specification-only request ends with a concrete reviewable design and no implementation. An authorized implementation request continues after clarifying necessary seams; this skill does not add an approval stop. Missing foundation work is an explicit dependency/scope item, not a reason to silently import an editor or pretend a service exists.
