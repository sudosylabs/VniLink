# File transfer

This document describes the current jobs-based implementation. The applied
contract for the Files window and engine is specified in
[File transfer window and engine](../../docs/architecture/file-transfer.md).

File transfer is a separate grant from control and from clipboard. A `.vnl`
file and the control confirmation do not turn it on. `Core::authorize` does.
The rules live in `src/core/jobs.rs` (scheduling, resume, peer binding),
`src/core/fs.rs` (remote browsing and auxiliary streams),
`src/core/blob.rs` (content-addressed offers) and `src/manifest.rs`
(staging and publish). The single-file byte-buffer API and its dedicated
transfer stream were removed; all movement goes through jobs.

## What happens

1. After Accept, the controller opens the clipboard stream and sends one
   ready byte. The host accepts the clipboard stream, then serves the
   filesystem auxiliary loop. Keyboard and pointer stay on the control
   stream.
2. Each side approves roots with `Core::set_file_roots` and a job store
   with `Core::set_blob_directory`. `Capabilities::file_transfer` is true
   once at least one root is set.
3. The peer browses with `fs_list`/`fs_mkdir`/`fs_rename` against a live
   session. Every request re-checks the Transfer grant; a revoked grant
   answers `TransferDenied` without ending the session.
4. Bulk movement uses jobs: `job_submit_upload` / `job_submit_download`
   return a job id, and both sides drive it with `job_poll` /
   `job_snapshot`. Imports go through anchored, symlink-refusing handles;
   content is verified by hash before it lands under a root.
5. Jobs are peer-scoped: one peer cannot resume, list or adopt another
   peer's job. Revoking Files interrupts that peer's jobs; a dropped
   session interrupts rather than replays, and resume needs an explicit
   `job_resume`.
6. Clipboard file selections paste through the same engine: the receiver
   prepares a paste, the source offers each file as a peer-scoped blob,
   and bytes move only after preparation. The steps are in `CLIPBOARD.md`.
