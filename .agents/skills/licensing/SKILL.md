---
name: licensing
description: "Which license notice a source file carries, how to add it, and how code copied from VS Code or another project is recorded. Use when adding source files, importing or adapting upstream code, vendoring a dependency or changing license metadata."
---

# Licensing

VniLink is AGPL-3.0-only (`LICENSE`); `package.json`, the workspace `Cargo.toml` and the staged application metadata say so, and `check:license` verifies it. The copyright holder is `publisherName` in `product.json`; no other file names it, except `REUSE.toml` (`hygiene` allows a notice's copyright line and that file).

## The notice a file carries

`build/lib/licenseNotice.ts` owns the text and each language's comment syntax; never type a notice by hand.

| Kind | Which files | Notice |
|---|---|---|
| original | Written for this project: Rust, TypeScript, `.mjs`, CSS, shell, PowerShell, batch, C/C++ | The project's AGPL-3.0-only notice. |
| verbatim | Listed in `build/vscode-source.json` without `adapted`, and `build/lint/vscode/` | VS Code's notice, unchanged. A byte of difference fails `check:upstream`. |
| derived | Copied from VS Code and changed: `adapted` manifest entries, and files outside the manifest that start with VS Code's notice | VS Code's notice, then the modification notice (`AGPL-3.0-only AND MIT`). |
| none | `vendor/`, generated bindings, and files that cannot carry a comment | Their own license, or `REUSE.toml` for the project's data files. |

A file is derived when it restates upstream text or names itself an adaptation of it. A comment that says it follows VS Code's shape is not enough. Judge by comparing it with the pinned checkout (`build/vscode-source.json` names the commit): shared statements beyond declarations and imports mean derived. When in doubt, treat it as derived; claiming both licenses costs nothing, and dropping MIT's notice breaks its terms.

## Tasks

```bash
node build/run.ts license:apply            # dry run: lists files that lack a notice
node build/run.ts license:apply --write    # adds them
npm run check:license                      # part of `npm run check` and the pre-commit hook
```

`license:apply` stops, changing nothing, at a file with a copyright line it did not write. Decide it: a foreign notice means the code came from elsewhere, so record its origin and license (below) instead of relabelling it.

## Copying code

- **From VS Code:** import with `vscode import`; the copy keeps its notice. After editing it, run `vscode adapt`, add the line naming the adaptation to VS Code's notice (`Adapted for VniLink: …`), then run `license:apply --write`, which adds the modification notice and records the new hash. `vscode reset` restores upstream text and the manifest entry.
- **A file derived from VS Code that is not in the manifest** (a rewrite that restates upstream code) starts with VS Code's notice, including an `Adapted from VS Code <version> <upstream path>.` line; `license:apply --write` then adds the modification notice.
- **From another project:** keep its license and notice, and do not add the project's. Put a whole dependency in `vendor/` with a `VNILINK.md` for local patches. Check first that its license is compatible with AGPL-3.0; GPL-2.0-only and licenses with advertising or field-of-use restrictions are not.
- **Generated code** from a third party's header (bindgen output) carries that header's license, not the project's.

## Files without a comment

Add the path to `REUSE.toml` if the project owns it. Third-party files keep their license files beside them. Do not add headers to JSON, Markdown, images or lockfiles.

## Changing the notice text

The text and the year (`noticeYear`) are in `licenseNotice.ts`. `license:apply` adds a missing notice and does not rewrite an existing one, so a new wording needs a one-time rewrite of the old text, reviewed like any mechanical change across the repository.
