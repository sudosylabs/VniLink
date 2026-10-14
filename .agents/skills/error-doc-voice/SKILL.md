---
name: error-doc-voice
description: "Preserve Rust/bridge error categories and write clear localized user feedback, diagnostic details and ownership/invariant documentation."
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

Module docs state ownership, neighboring boundaries and important effects. Public contracts document meanings that names/types do not make obvious, particularly lifecycle, permission, persistence, thread/resource and generation semantics. Preserve upstream license/provenance headers.

Comments explain reasons, invariants and traps rather than narrating obvious code. Keep them close to the relevant contract; do not repeat the same architectural rule throughout callers. Avoid blanket boilerplate stating negative scope in every trivial file. Documentation describes current facts or explicitly labeled target/migration work, not proposals presented as implemented behavior.
