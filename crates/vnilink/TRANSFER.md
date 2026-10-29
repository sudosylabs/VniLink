# File transfer

This document describes the current jobs-based implementation. The applied
contract for the Files window and engine is specified in
[File transfer window and engine](../../docs/architecture/file-transfer.md).

File transfer is a separate grant from control and from clipboard. A `.vnl`
file and the control confirmation do not turn it on. `Core::authorize` does.
The rules live in `src/core/jobs.rs` (scheduling, resume, peer binding),
`src/core/service_jobs.rs` (Files ownership and adoption),
`src/core/fs.rs` (the shared disk and serving its listings),
`src/transfer/` (the `vnilink/files/1` protocol), `src/core/transfer.rs` (its connections) and
`src/manifest.rs` (scanning and publish). The single-file byte-buffer API and its dedicated
transfer stream were removed; all movement goes through jobs.

## What happens

1. The controller reaches the other computer's disk through a Files
   service on its link to that computer, opened as the Files tab opens
   it: listings and changes go through the service's pipe, and jobs ride
   the service. Keyboard and pointer stay on the control
   stream.
2. Each side shares its whole filesystem with
   `Core::enable_filesystem_access` (`/` on Unix, every mounted drive on
   Windows) and names a transfer directory with
   `Core::set_transfer_directory`, created the first time a job needs it:
   it holds the job journals and the partial files of arriving jobs. `Capabilities::file_transfer` is true
   once the filesystem is shared. There is no other mode: Files shares
   the whole disk or nothing, and each peer operation needs Files
   sharing with that peer (the `Transfer` grant, shown as Files in the
   app's sharing settings).
3. The peer browses with the Files service's `list`/`mkdir`/`rename`/
   `delete`. Every request re-checks Files sharing; withdrawing it ends
   the Files service and leaves the link. Each listed entry
   carries a token that a rename, delete or move to the Trash hands back,
   so a change made from a stale listing is refused rather than applied to
   whatever took the entry's place. The token guards against a stale
   listing, not against the peer: it is no authorization boundary, and
   Files sharing alone decides what a peer may change. On most volumes the
   token is the object's identity (device, inode, creation time). On a
   volume whose ids are entry positions it is a fingerprint: the entry's
   name, kind, size, times at the volume's granularity (FAT: 2 s
   modification, 10 ms creation, though macOS's msdos reports creation to
   the even second; exFAT: 10 ms) and attributes, on that volume. A file
   renamed, deleted or trashed there, or a folder renamed, goes ahead while
   its fingerprint still matches, read through the folder's handle at that
   moment; deleting a file checks it again on the object removed (the
   deleting handle on Windows; on Unix the entry moved aside under a
   private name, which FAT keeps the times of). Deleting or trashing a
   folder there, or any change without a listed fingerprint, is refused as
   `UnprovenEntry`, since no fingerprint covers what a folder holds.
   Positional volumes are recognised:
   - on Windows by name (`FAT`, `FAT12`, `FAT16`, `FAT32`, `exFAT`), and
     any volume reporting no usable `FileIdInfo`, such as some SMB shares,
     takes the same path: listed by fingerprint, folders not deleted;
   - on macOS by `f_fstypename`: `msdos`, `exfat`, or a third-party driver
     whose name holds `fat` or `msdos`. A macFUSE mount names only
     `macfuse`/`osxfuse`, so a FAT volume served through it cannot be
     recognised and is treated as having stable ids;
   - on Linux by `f_type` (`MSDOS_SUPER_MAGIC`, `EXFAT_SUPER_MAGIC`), and
     for a FUSE mount by the subtype `/proc/self/mountinfo` gives it
     (`fuseblk.exfat`, `fuse.fusefat`). A FUSE driver that sets no subtype
     cannot be recognised.
   On macOS, FSKit's msdos and exfat refuse an exclusive rename; on those
   volumes alone a rename checks the new name is free and then takes it,
   so a file made under that name in the same instant is replaced.
4. Bulk movement uses jobs. A Files tab submits one job per selection:
   `files_service_upload` / `files_service_download` name a folder and
   the entries of it to copy, and one manifest holds them all.
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
   that owns the job. Revoking Files
   interrupts that peer's jobs, and a session that ends interrupts its
   jobs rather than replaying them. A failure that may clear on its own (a
   connection lost or redialing, a stalled stream, a sender that answers
   late) keeps the job going and tries again after 1, 2, 4, 8, 16, then
   30 s; once six tries or two minutes pass without a byte moving, the job
   is interrupted. Every interrupted job says why until it resumes. A Files job whose owner goes, with its window, pipe or
   connection, is left Interrupted, saying which of them went: the window
   closed, this computer or the other one shut down, the other computer
   ended its side or withdrew Files, or the connection ended. The next
   Files owner for that peer
   adopts and resumes it on its own (`files_service_resume_orphans`,
   which the Files channel calls as it opens), and `job_resume` resumes
   any other. Interrupted jobs do not count against the active-job
   limit until they resume. A download still being prepared has nothing
   to resume from, and fails instead.

## Moving the bytes

The receiver dials its own connection to the sender under
`vnilink/files/1` and keeps it for the files that follow, one stream per
file. Control stays on `vnilink/1`, so a large file never queues job
control or the desktop behind it.

1. A request names the sender's job, the file's place among the
   manifest's files and the offset to start from. The sender serves only
   its own sending job for that peer, while the job is active, its owner
   is live and Files is shared with the peer. Anything else is
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
   cancel, a lost connection or a restart, asks for the file from the
   segment after the last journaled one and neither side reads earlier
   bytes again. The bytes of that segment which already arrived are kept:
   the receiver reads them back for the segment's hash, and the request
   names them, so the sender reads them for its hash too and sends only the
   rest. A link that keeps dropping still makes progress, where restarting
   the segment would lose up to 16 MiB each time. A source that changed
   drops them. A segment that fails its check, held bytes a crash tore
   among them, is fetched once more, then fails the job. The last segment
   is checked but not journaled: the file publishes as soon as it checks.
5. While a receiver takes one file, it fetches up to seven of the next
   small files (one segment or less, 64 MiB in all) ahead into their
   partials, so many small files do not cost a round trip each. They still
   publish one at a time in manifest order, through the same conflict and
   permission checks, and one is used only if it arrived whole on the files
   connection still open: revoking closes that connection, so nothing
   fetched after a revoke lands, though a file that had already arrived
   whole when the revoke reached this computer may.
6. The receiver holds the files connection's receive window to what its
   path carries (`transfer/window.rs`), so a transfer yields to the
   desktop session instead of queueing ahead of it. Through a relay every
   connection to the peer shares one queue that never drops, and a
   bottleneck on a direct path queues too; a bulk transfer filled them to
   seconds ahead of the session's packets.
   - The connection is dialed with a 1 MiB window, not the endpoint's
     64 MiB, so the sender cannot have the whole window in flight before
     the path is known.
   - From then on the window follows the round trip, as LEDBAT does
     (RFC 6817). The least round trip of the last two minutes is the
     path's, and anything above it is queue. Once a round trip, the window
     grows by up to an eighth with no queue, eases as the queue nears
     50 ms, and halves beyond 200 ms.
   - It never falls below 32 KiB, and never grows past twice what the path
     carried in a base round trip plus the target. A fast path with no
     queue opens to the full window.
   - The queue is shared, so the transfer sees the delay it would add to
     the session. Measurements are in the impaired-network harness's
     `combined` and `combined-download` scenarios.
7. Publication then follows the conflict choice, re-checking cancel,
   grant and owner under the commit (see `publish_staged`). Cancelled
   and interrupted jobs keep their partials for resume, within the
   resume retention policy; finished, failed and forgotten jobs remove
   them.

