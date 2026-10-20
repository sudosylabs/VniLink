# File transfer

This document describes the current jobs-based implementation. The applied
contract for the Files window and engine is specified in
[File transfer window and engine](../../docs/architecture/file-transfer.md).

File transfer is a separate grant from control and from clipboard. A `.vnl`
file and the control confirmation do not turn it on. `Core::authorize` does.
The rules live in `src/core/jobs.rs` (scheduling, resume, peer binding),
`src/core/service_jobs.rs` (Files ownership and adoption),
`src/core/fs.rs` (remote browsing and auxiliary streams),
`src/core/blob.rs` (content-addressed offers) and `src/manifest.rs`
(staging and publish). The single-file byte-buffer API and its dedicated
transfer stream were removed; all movement goes through jobs.

## What happens

1. After Accept, the controller opens the clipboard stream and sends one
   ready byte. The host accepts the clipboard stream, then serves the
   filesystem auxiliary loop. Keyboard and pointer stay on the control
   stream.
2. Each side shares its whole filesystem with
   `Core::enable_filesystem_access` (`/` on Unix, every mounted drive on
   Windows) and names a job store with `Core::set_blob_directory`, created
   the first time a transfer needs it. `Capabilities::file_transfer` is true
   once the filesystem is shared. `Core::set_file_roots` confines browsing
   to chosen folders; only tests use it.
3. The peer browses with `fs_list`/`fs_mkdir`/`fs_rename` against a live
   session. Every request re-checks the Transfer grant; a revoked grant
   answers `TransferDenied` without ending the session.
4. Bulk movement uses jobs. A Files tab submits one job per selection:
   `files_service_upload` / `files_service_download` name a folder and
   the entries of it to copy, and one manifest holds them all
   (`job_submit_upload` / `job_submit_download` take a single path).
   Imports go through anchored, symlink-refusing handles; content is
   verified by hash before it lands under a root. A download the other
   computer takes long to prepare is a `Preparing` job at once: the host
   answers within a second, and the receiver asks for the manifest
   (`TAG_JOB_MANIFEST`) until it is ready.
5. Each computer's background driver (`start_job_driver`) gives each
   active job a worker: a receiver takes file after file until it waits
   on a decision, on admission or on its peer; a sender asks its receiver
   how far it got, soon while that changes and seldom while it does not.
   `job_poll` takes one step, for callers driving a job themselves.
   Records persist in per-job journals (`core/job_journal.rs`) that grow
   by what changed; a step that changes nothing writes nothing.
6. Jobs are peer-scoped: one peer cannot resume, list or adopt another
   peer's job. Job control rides the connection of the Files service
   that owns the job, else the peer's control session. Revoking Files
   interrupts that peer's jobs, and a dropped session interrupts rather
   than replays. A Files job whose owner goes, with its window, pipe or
   connection, is left Interrupted; the next Files owner for that peer
   adopts and resumes it on its own (`files_service_resume_orphans`,
   which the Files channel calls as it opens), and `job_resume` resumes
   any other. Interrupted jobs do not count against the active-job
   limit until they resume. A download still being prepared has nothing
   to resume from, and fails instead.
7. Clipboard file selections are offers of names and sizes; pasting one
   moves its bytes through Files jobs. The steps are in `CLIPBOARD.md`.
