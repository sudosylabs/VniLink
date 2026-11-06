---
name: handwriting
description: "How Rust and TypeScript files read: imports, declarations, method shape, comments and formatting, after VS Code's style. Use when writing or tidying a source file, or when a review asks for readability."
---

# Handwriting

Architecture belongs to `workbench-architecture` and `crates/AGENTS.md`; formatting cannot compensate for unclear ownership. Application TypeScript should read like the corresponding VS Code subsystem: focused methods, explicit fields/dependencies, meaningful names and one statement per line. Avoid compressed constructor/event-handler chains that hide steps and make diffs hard to review.

## Imports and structure

TypeScript imports form a coherent top block; group external packages before project imports, order within groups consistently, and consolidate duplicate value imports. Types may be separate when the lint rules permit it. In `src/` and `test/`, relative module specifiers end in `.js` (esbuild resolves them); in `build/`, Node runs the TypeScript directly, so they name the `.ts` file and types use `import type`. Stylesheet specifiers retain `.css`. Side-effect registration imports are intentional and their order must preserve initialization dependencies. Keep license/provenance headers first.

Separate top-level declarations and methods with one blank line. Use tabs and the repository's TypeScript formatter (`tsfmt.json`/`build/lib/format.ts`, run with `npm run format`). Do not manually reformat pinned upstream copies or reorder their imports to satisfy application conventions; see `workbench-architecture`.

Rust module intent docs precede declarations. Group imports by standard library, external crates and workspace/crate modules consistently, letting rustfmt order names. Locally scoped imports remain appropriate for isolated tests or features where the owning area uses them. Use idiomatic structs/traits; class preference does not change Rust idioms.

Split files when responsibilities diverge, preserving a readable public contract and lifecycle owner. Avoid arbitrary line-count splits or one file per trivial method. Public docs and non-obvious invariants follow `error-doc-voice`.

## Verification

Run format/lint checks covering edited files and resolve their violations. Mechanical formatting is not architectural review. `unit-tests` defines when broader checks are required; generated/upstream artifacts follow their own source-of-truth verification.
