---
name: component
description: "Specify a substantial new service, feature, bridge capability or crate before building it: purpose, owner, contracts, lifecycle, presentation and verification. Use when asked to design or spec a component, or when a request's boundaries or owner are unclear before implementing it."
---

# Component Design

For a substantial new component or an unclear boundary, write the design before the code. Read the request and the analogous upstream implementation first; placement follows the layer `AGENTS.md` files, and how TypeScript is written follows `workbench-architecture` (for a common addition, `recipes` already has the steps).

## What the design answers

- **Purpose and scope:** the capability people get, the state and operations it owns, its neighbours, and whether it is a global default, a per-device override or a runtime action.
- **Owner and shape:** the layer, the owning classes or Rust types, what is injected, where it is registered, and the dependency direction. Crossing to the engine names its command or channel (`engine-bridge`, `ffi-boundary`).
- **Contracts:** inputs and outputs, effects, permissions, persistence and how a write is read back, error categories (`error-doc-voice`).
- **Lifecycle:** start, replacement, cancellation, failure and disposal (`async-cancellation`, `memory-leak-audit`).
- **Presentation:** states (loading, empty, error, unavailable), keyboard and accessibility, themes and layout (`design-philosophy`).
- **Verification:** the tests that prove it and the environments it needs (`unit-tests`, `cross-platform-validation`).

State the assumptions and open decisions that change the implementation, and settle them with the user rather than guessing in code. A missing foundation (a service, a widget, an import) is a named part of the scope.

## Deliver what was asked

A request for a design ends with the design. A request to build it goes on to the implementation once the open decisions are settled.
