---
name: error-doc-voice
description: "Errors, messages, comments and docs: Rust error categories kept through the bridge, plain localized messages, code documented in its module, and text that reads true in any contributor's checkout. Use when adding or changing an error, a message people read, a comment, a doc or a fixture."
---

# Errors and Documentation

## Error ownership

Domain failures live in the owning shared Rust taxonomy and map through facade/bridge without losing distinguishable categories. Extend that taxonomy for new domain states rather than inventing formatted errors in callers. Boundary parse errors, Electron transport failures and local persistence failures also exist; they must not be mislabeled as core domain failures.

Messages name the actual state in plain language. Rust diagnostic reasons use the existing lowercase style; bound dynamic detail at its constructor/owner. Preserve native/transport diagnostic context for logs without exposing secrets, unrestricted paths or large payloads in user-facing messages. Verify actual source bounds instead of caching numeric limits in this skill.

Match error categories, not mutable message text. Keep stale and absent, denied and unavailable, and saved-but-unconfirmed as distinct states, with their structure intact through every layer. Each failure reaches the user once, however many stages log it.

## User presentation

The bridge preserves core semantics; the UI may provide localized, contextual explanations and actionable recovery without inventing success or a new domain meaning. Raw technical exception strings are diagnostic evidence, not automatically good interface copy. Say what failed and what the user can do about it; suggest restarting only when a restart is what fixes it, and mention an implementation detail only when it changes the user's decision.

Separate configuration validation, save failure and runtime reconciliation. Permission/capability explanations come from actual OS/backend state. Say only what helps a decision; `design-philosophy` owns the product's tone.

## Documentation

How an implementation works is documented in the module that owns it: the file-header (`//!`) comment and the doc comments on its contracts. A contract that spans modules is summarised in `ARCHITECTURE.md`, which names the code that implements it and never restates a module's detail. A comment names code (symbols, and a file only when that file is what the comment describes); it never cites a document, a skill, the README, a plan, or a step or phase, which can be left behind when the code changes.

Module docs state ownership, neighboring boundaries and important effects. Public contracts document meanings that names/types do not make obvious, particularly lifecycle, permission, persistence, thread/resource and generation semantics. Preserve upstream license/provenance headers.

Comments explain reasons, invariants and traps rather than narrating obvious code. Keep each one beside the contract it explains, once, rather than restated in every caller; a file states what it does, not what it leaves to others. Documentation describes current facts or explicitly labeled target/migration work, not proposals presented as implemented behavior.

## Writing about the project

Comments, docs, skills and fixtures describe the project, so they read true in any contributor's checkout. State behaviour and constraints by platform or hardware class: "on a hybrid Intel+NVIDIA laptop", "on Apple silicon". A measured number keeps its hardware class, the date it was measured and where the evidence lives (a test, the module doc of the measuring task, or the commit that measured it).

Name what a contributor supplies by its role: "a VS Code checkout at the ref in `build/vscode-source.json`", "the remote computer's SSH destination". Fixtures use neutral placeholders: `/Users/someone`, `C:\Users\someone`, `/home/someone`, and a device such as `Studio PC`. Product identity (app id, publisher, repository, support address) is read from `product.json`; only the contact documents (`identityFiles` in `build/lib/hygiene.ts`) spell it out.

`npm run check:hygiene` rejects home directories other than the placeholders, git authors' handles and product identity outside those files, and comments in the project's own Rust and TypeScript that cite a markdown document, the README, a skill or `docs/`, or a step or phase.
