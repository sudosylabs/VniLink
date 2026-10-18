---
name: code-review
description: "Review branch, commit or local changes against product contracts, correctness, architectural standards, lifecycle, security and cross-platform evidence."
---

# Code Review

## Fix the scope

Read user-supplied specification/reference paths. Establish comparison ref/merge-base, committed changes, working-tree changes and new files; reuse confirmed scope rather than asking again. Inspect status/log/diff before assuming an empty diff means no work. Respect read-only requests and isolate reproductions where required.

## Review concerns

- Product/spec behavior: real permissions/capabilities, defaults versus overrides, persistence and error/recovery semantics.
- Correctness/security: authorization, boundary validation, ownership, race/cancellation, filesystem guarantees and backward compatibility.
- Architecture: `workbench-architecture`, source placement, Rust ownership and reusable build/test infrastructure. Current code is not precedent for lowering the target.
- UI: visual continuity, accessibility, themes and platform-specific controls where affected.
- Portability/verification: `cross-platform-validation`, native binary freshness and reproducibility. Test abstraction use does not demonstrate native success.
- Scope: distinguish necessary supporting changes from unrelated migration; do not demand removal of required dependencies simply because they are additional files.

Apply relevant skills; formatting findings do not outrank functional/security defects just because they are easy to spot. Reproduce uncertain claims when feasible through `unit-tests`, reporting evidence and limitations. Follow the user's requested depth; revalidate previous findings and adjacent caller/recovery paths when reviewing fixes.

## Architecture checklist

Answer these for every changed TypeScript class or task; a "no" is a finding unless the change explains why.

| Question | Where the answer should be |
|---|---|
| Who owns each piece of state, and is there exactly one owner? | A service, view or main-process service; never module globals |
| Which service boundary does it cross, and is the contract validated there? | `common` guards, channel facades (`windowIdFromContext`), `isConfigurationUpdate` |
| Where is it registered? | `registerSingleton`, `registerWorkbenchContribution2`, `registerAction2`, the home pages or setting controls registries, `initChannels` |
| How is it disposed, including after an `await`? | `_register`, `DisposableStore`, `isDisposed()` checks, `CancellationToken` |
| Which widget renders each control? | `base/browser/ui` widgets or `settingRenderers.ts`; DOM built with `$`, `append`, `h` |
| Does a new setting go through the schema? | `configurationSchema.ts`, applied instantly, no page-level Save |
| Is build code a function? | `build/lib`, composed with `define`/`series`/`parallel`; classes only for OS-resource owners |
| Which upstream class or in-repo reference was it compared with? | VS Code at the pinned commit; `contrib/devices` for workbench views |

The lint rules (`vnilink/workbench-dom`, `vnilink/build-classes`, `vnilink/layers`, the ported `local/code-*` rules) catch some of these mechanically; passing lint does not answer the table.

## Findings

Each actionable finding states severity, precise location, concrete trigger, consequence and supporting evidence/contract. Separate correctness/spec failures, architectural noncompliance and optional notes; not every spacing issue has the same severity as data loss. Preserve review handoff/versioning when requested. A clean review states scope and material verification limits rather than claiming universal correctness.
