---
name: update-instructions
description: "Change AGENTS.md files or skills, or turn a user's correction into durable guidance. Use when asked to add, rewrite or audit instructions or skills, when the user corrects how work should be done and the rule should outlast the conversation, or when guidance turns out stale or contradictory."
---

# Maintain Instructions

Use `writing-for-agents` (or `skill-creator`) guidance when available. Permission to change instructions covers instructions only: application refactors and environment setup stay separate work.

## From a correction to a rule

Read the user's examples and the files they name before interpreting a correction. Decide whether the example illustrates a broader quality concern or is the whole request; use the surrounding conversation and newer decisions to set the scope, and keep an isolated incident from becoming a universal ban.

Name what went wrong: missing guidance, weak discovery (the rule exists but nothing points to it), conflicting rules, a stale fact, or guidance that was there and not followed. Each has a different fix; only the first needs new text. For an architectural correction, state ownership and lifecycle against the upstream reference; for a platform correction, the prerequisites and evidence; for a UI correction, the product intent rather than one example's pixel values.

## Where it goes

One rule has one owning document; others point to it. Root `AGENTS.md` routes, layer `AGENTS.md` files own placement, skills own workflows and contracts. A fact the repository already states (a list in code, a script, a config) is pointed to, not copied: a copy goes stale when the source changes. A new rule supersedes older conflicting guidance; keep the non-obvious guarantees and delete the contradiction.

A skill is reached through `.claude/skills`, a link to `.agents/skills`, by its `description`, and through the root `AGENTS.md` line that names it. The description says what the skill is for and when to use it ("Use when …"), in the words a task arrives in. Every skill has a line in the root `AGENTS.md`.

## Validate

Run `MUSE_NO_AUTO_UPDATE=1 muse skills validate <skill-directory> --json` for each changed skill when `muse` is installed, and read its diagnostics. Check every path, command and skill name the change cites against the repository, and walk one realistic task through the changed guidance. Instruction-only changes need no application build. Report what changed, the validation results, and any remediation the new guidance implies but this change did not do.
