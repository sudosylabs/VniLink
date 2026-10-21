# File transfer

This document describes the current jobs-based implementation. The applied
contract for the Files window and engine is specified in
[File transfer window and engine](../../docs/architecture/file-transfer.md).

File transfer is a separate grant from control and from clipboard. A `.vnl`
file and the control confirmation do not turn it on. `Core::authorize` does.
The rules live in `src/core/jobs.rs` (scheduling, resume, peer binding),
`src/core/service_jobs.rs` (Files ownership and adoption),
`src/core/fs.rs` (remote browsing and auxiliary streams),
`src/transfer/` (the `vnilink/files/1` protocol), `src/core/transfer.rs` (its connections) and
`src/manifest.rs` (scanning and publish). The single-file byte-buffer API and its dedicated
transfer stream were removed; all movement goes through jobs.

## What happens

1. After Accept, the controller opens the clipboard stream and sends one
   ready byte. The host accepts the clipboard stream, then serves the
   filesystem auxiliary loop. Keyboard and pointer stay on the control
   stream.
2. Each side shares its whole filesystem with
   `Core::enable_filesystem_access` (`/` on Unix, every mounted drive on
   Windows) and names a transfer directory with
   `Core::set_transfer_directory`, created the first time a job needs it:
   it holds the job journals and the partial files of arriving jobs. `Capabilities::file_transfer` is true
   once the filesystem is shared. `Core::set_file_roots` confines browsing
   to chosen folders; only tests use it.
3. The peer browses with `fs_list`/`fs_mkdir`/`fs_rename` against a live
   session. Every request re-checks the Transfer grant; a revoked grant
   answers `TransferDenied` without ending the session.
4. Bulk movement uses jobs. A Files tab submits one job per selection:
   `files_service_upload` / `files_service_download` name a folder and
   the entries of it to copy, and one manifest holds them all
   (`job_submit_upload` / `job_submit_download` take a single path).
   The sender scans its sources into a manifest of paths, sizes and
   modification times and reads nothing more ahead of time. A download the other
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
   interrupts that peer's jobs, and a session that ends interrupts its
   jobs rather than replaying them. A failure that may clear on its own (a
   connection lost or redialing, a stalled stream, a sender that answers
   late) keeps the job going and tries again after 1, 2, 4, 8, 16, then
   30 s; once six tries or two minutes pass without a byte moving, the job
   is interrupted. Every interrupted job says why until it resumes. A Files job whose owner goes, with its window, pipe or
   connection, is left Interrupted; the next Files owner for that peer
   adopts and resumes it on its own (`files_service_resume_orphans`,
   which the Files channel calls as it opens), and `job_resume` resumes
   any other. Interrupted jobs do not count against the active-job
   limit until they resume. A download still being prepared has nothing
   to resume from, and fails instead.
7. Clipboard file selections are offers of names and sizes; pasting one
   moves its bytes through Files jobs. The steps are in `CLIPBOARD.md`.

## Moving the bytes

The receiver dials its own connection to the sender under
`vnilink/files/1` and keeps it for the files that follow, one stream per
file. Control stays on `vnilink/1`, so a large file never queues job
control or the desktop behind it.

1. A request names the sender's job, the file's place among the
   manifest's files and the offset to start from. The sender serves only
   its own sending job for that peer, while the job is active, its owner
   is live and the peer holds the Transfer grant. Anything else is
   refused before a byte is read.
2. The sender opens the file through the leg's pinned root by handles,
   never a retained absolute path, refusing links. It must still be the
   file scanned: the same file id (inode or Windows file index), size and
   modification time. A file renamed over it, even with the same size, is
   refused as changed.
3. A file moves in 16 MiB segments, each a subtree of the file's BLAKE3
   tree (`transfer/hash.rs`). The sender reads the held file once from the
   requested segment, hashing as it reads, and follows each segment with
   its chaining value. It re-checks the job and grant every few megabytes,
   so a cancel or revoke stops a file mid-way, and checks the source's
   size and time as each segment ends; a source that changed is reset
   instead of carried on.
4. The receiver appends to the file's partial in the transfer directory
   and checks each segment as it lands. A checked segment before the last
   is synced and then journaled beside the partial, so a resume, after a
   cancel or a restart, asks for the file from the segment after the last
   journaled one and neither side reads earlier bytes again; unjournaled
   bytes are dropped. A segment that fails its check is fetched once more,
   then fails the job. The last segment is checked but not journaled: the
   file publishes as soon as it checks.
5. While a receiver takes one file, it fetches up to seven of the next
   small files (one segment or less, 64 MiB in all) ahead into their
   partials, so many small files do not cost a round trip each. They still
   publish one at a time in manifest order, through the same conflict and
   permission checks, and one is used only if it arrived whole on the files
   connection still open: revoking closes that connection, so nothing
   fetched after a revoke lands, though a file that had already arrived
   whole when the revoke reached this computer may.
6. Publication then follows the conflict choice, re-checking cancel,
   grant and owner under the commit (see `publish_staged`). Cancelled
   and interrupted jobs keep their partials for resume, within the
   resume retention policy; finished, failed and forgotten jobs remove
   them.

