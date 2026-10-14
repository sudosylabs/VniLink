---
name: update-instructions
description: "Maintain AGENTS.md and skills against verified sources, target standards and user decisions, with clear ownership, reference validation and no unrelated implementation changes."
---

# Maintain Instructions

Use `skill-creator`/`writing-for-agents` guidance when available. Preserve current user intent and scope; explicit permission to update instructions does not authorize application refactors or environment setup.

## Audit before editing

Read supplied paths and applicable instructions, then compare relevant upstream/current implementations. Cover the concern broadly enough to include architecture, runtime/contracts, UX, portability and reproducibility where relevant. Examples are evidence, not necessarily the entire requested scope. For a substantial rewrite, map observed problems and accepted decisions to their instruction owners; see [coverage.md](references/coverage.md) for this rewrite's coverage and migration boundaries.

Distinguish target standard, existing facility and missing prerequisite/migration. The current repository is not the quality ceiling; missing infrastructure cannot silently waive the standard or be described as implemented.

## Rewrite and route

One rule has one owning document; other guidance points to it. Root AGENTS routes, layer AGENTS defines placement/ownership, skills explain workflows/contracts. Preserve useful non-obvious guarantees while deleting contradictions, stale examples and generic boilerplate. Descriptions must trigger on the actual work; avoid requiring every skill on every task.

Verify source facts and referenced paths/commands before publishing them. Do not rewrite user goals merely because current tooling is incomplete. Keep implementation plans distinct from instruction changes.

## Validate

Run `MUSE_NO_AUTO_UPDATE=1 muse skills validate <skill-directory> --json` for changed skills if available; inspect validity, diagnostics and warnings. `skill-creator` quick validation is a fallback where available. Verify links/reference paths, duplicated/conflicting completion rules and realistic task decisions; metadata validation alone does not prove useful guidance.

Check that changes stayed in the authorized instruction scope. Instruction-only changes do not need application builds or platform test runs. Report the instructions changed, validation outcomes and migrations still required.
