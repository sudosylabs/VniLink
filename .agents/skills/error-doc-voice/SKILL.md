---
name: error-doc-voice
description: "Preserve Rust/bridge error categories and write clear localized user feedback, diagnostic details, ownership/invariant documentation, and project-neutral comments, docs and fixtures."
---

# Errors and Documentation

## Error ownership

Domain failures live in the owning shared Rust taxonomy and map through facade/bridge without losing distinguishable categories. Extend that taxonomy for new domain states rather than inventing formatted errors in callers. Boundary parse errors, Electron transport failures and local persistence failures also exist; they must not be mislabeled as core domain failures.

Messages name the actual state in plain language. Rust diagnostic reasons use the existing lowercase style; bound dynamic detail at its constructor/owner. Preserve native/transport diagnostic context for logs without exposing secrets, unrestricted paths or large payloads in user-facing messages. Verify actual source bounds instead of caching numeric limits in this skill.

Match error categories, not mutable message text. Do not collapse stale/absent, denied/unavailable or saved-but-unconfirmed states. Preserve structured information through layers; logging every stage must not duplicate user notifications.

## User presentation

The bridge preserves core semantics; the UI may provide localized, contextual explanations and actionable recovery without inventing success or a new domain meaning. Raw technical exception strings are diagnostic evidence, not automatically good interface copy. Say what failed and what the user can do; do not prescribe restarting for every failure or expose implementation details that do not affect a decision.

Separate configuration validation, save failure and runtime reconciliation. Permission/capability explanations come from actual OS/backend state. Avoid routine reassurance and verbosity that `design-philosophy` identifies as unnecessary.

## Documentation

How an implementation works is documented in the module that owns it: the file-header (`//!`) comment and the doc comments on its contracts. A contract that spans modules is summarised in `ARCHITECTURE.md`, which names the code that implements it and never restates a module's detail. A comment names code (symbols, and a file only when that file is what the comment describes); it never cites a document, a skill, the README, a plan, or a step or phase, which can be left behind when the code changes.

Module docs state ownership, neighboring boundaries and important effects. Public contracts document meanings that names/types do not make obvious, particularly lifecycle, permission, persistence, thread/resource and generation semantics. Preserve upstream license/provenance headers.

Comments explain reasons, invariants and traps rather than narrating obvious code. Keep them close to the relevant contract; do not repeat the same architectural rule throughout callers. Avoid blanket boilerplate stating negative scope in every trivial file. Documentation describes current facts or explicitly labeled target/migration work, not proposals presented as implemented behavior.

## Writing about the project

Comments, docs, skills and fixtures describe the project, so they read true in any contributor's checkout. State behaviour and constraints by platform or hardware class: "on a hybrid Intel+NVIDIA laptop", "on Apple silicon". A measured number keeps its hardware class, the date it was measured and where the evidence lives (a test, the module doc of the measuring task, or the commit that measured it).

Name what a contributor supplies by its role: "a VS Code checkout at the ref in `build/vscode-source.json`", "the remote computer's SSH destination". Fixtures use neutral placeholders: `/Users/someone`, `C:\Users\someone`, `/home/someone`, and a device such as `Studio PC`. Product identity (app id, publisher, repository, support address) is read from `product.json`; only the contact documents `SECURITY.md`, `CODE_OF_CONDUCT.md` and `CONTRIBUTING.md` spell it out.

`npm run check:hygiene` rejects home directories other than the placeholders, git authors' handles and product identity outside those files, and comments in the project's own Rust and TypeScript that cite a markdown document, the README, a skill or `docs/`, or a step or phase.
