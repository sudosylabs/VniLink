---
name: commit-organization
description: Organize and commit repository changes as several coherent, independently reviewable commits. Use when asked to commit work, split a large change, or plan a commit series; also supports author attribution when requested.
---

# Commit Organization

Default to several coherent commits when the work contains distinct changes. Each commit should express one purpose and include the code, tests, contracts and supporting resources needed for that purpose. A single cohesive fix can remain one commit; choose boundaries by meaning and dependencies rather than a target count or file size. Follow an explicit user instruction to squash or use a different grouping.

## Establish scope

Inspect the branch, staged and unstaged diffs, untracked files, renames, ignored-file policy and any user-selected scope. Account for existing staged changes before staging anything. A request to commit all changes includes eligible new files, while repository exclusions still apply; VniLink's ignored `docs/` stays untracked unless the user explicitly changes that policy.

Use the session's existing authorization and comparison point. Committing the current working tree does not require choosing an upstream base. When the user requests an authorship audit, or ownership affects the authorized scope, read [authorship.md](references/authorship.md). Working-tree changes have no reliable author attribution from Git alone.

## Design the series

Map every in-scope change or hunk to a commit with a concrete purpose and a short rationale for its boundary. Show the intended grouping briefly, then proceed when committing is already authorized; proposing the grouping is not another approval gate.

- Separate independently useful features, fixes, infrastructure changes and instruction changes.
- Keep a behavior change with its regressions. Keep generated contracts, lockfile changes, vendor provenance and required notices with the change that produces or requires them.
- Split independent changes within a shared file by hunks. Crossing Rust and TypeScript layers can belong in one commit when they implement one usable bridge capability.
- Put prerequisites before their consumers. Keep a refactor separate when it preserves behavior and leaves a valid intermediate tree; otherwise keep the inseparable parts together.
- Give every commit a reviewer-facing explanation of what changed and why. Follow repository message conventions; avoid vague messages such as “all changes.”

The grouping is complete when every eligible change is assigned, exclusions are identified and no commit depends on a later commit to build or satisfy its contracts.

## Stage and verify each commit

Stage the planned paths or hunks explicitly, then inspect the staged diff, including additions, deletions and renames. For shared files, use interactive staging or a reviewed index patch; preserve unrelated working-tree edits. Compare the staged result with the planned purpose before committing.

Use `unit-tests` for meaningful checks and `cross-platform-validation` when platform-sensitive behavior changes. Full-working-tree success does not prove an earlier commit works: when unstaged changes could mask missing dependencies, validate an isolated snapshot of HEAD plus the index, or regroup the commits. Keep temporary outputs out of the repository and preserve the user's working tree.

Run configured hooks and retain configured commit signing. If a hook changes files, review and restage only changes belonging to the current commit, then rerun the required checks. Resolve failures or report a concrete blocker; bypassing hooks or signing is not an alternative validation strategy.

Create the commit only after its staged contents and relevant checks are satisfactory. Verify the resulting commit's contents before moving to the next group. Organizing working-tree commits does not authorize amending published commits, rewriting history, pushing or force-pushing; use existing explicit authorization for those actions.

## Finish

Compare the completed series and remaining status with the original inventory. Confirm that all requested changes are committed and excluded changes remain intact. Report commit hashes with their purposes, actual validation results and any remaining uncommitted or unverified work. Never report a clean or complete result solely because the last commit succeeded.
