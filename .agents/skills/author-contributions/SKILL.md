---
name: author-contributions
description: "Identify an author\u2019s committed branch contributions against a confirmed base, tracing renames and distinguishing touched files from final diff attribution."
---

# Author Contributions

Resolve the exact author identity and requested base/branch from Git evidence; substring author matching is only a search aid. Establish whether the comparison is merge-base-to-branch or an explicit two-tree comparison. Use the same base for commit collection, final changed paths and stats.

Collect the author's commits and changed paths. Build rename history across all relevant commits, not only the author's: another contributor may have moved a file. Trace multi-hop renames, accounting for merge commits and Git's rename-detection limits; report uncertain attribution rather than treating a rename heuristic as proof.

Intersect contributions with paths in the final comparison. Distinguish direct touched paths, renamed contributions and deletions if the user requests all changes. File-level net diff stats are not automatically lines written by the author; later edits by others can change them. Working-tree/untracked changes have no reliable commit authorship without trusted evidence.

Use a reproducible script or structured Git output for larger histories, preserving paths safely. Return the comparison scope, identity and evidence in a concise table. Delegate only when authorized and useful; parallelism is not required to perform Git reads. This skill identifies contributions, not permission to move/delete files or rewrite history.
