# File transfer

This document describes the current single-file implementation. The planned
`iroh-blobs` engine, two-pane Files window, folder transfers and filesystem
renaming are specified in [File transfer window and engine](../../docs/architecture/file-transfer.md).

File transfer is a separate grant from control and from clipboard. A `.vnl` file and the control confirmation do not turn it on. `Core::authorize` does. The rules live in `src/transfer/`. `src/core/files.rs` opens the stream and checks the grant.

## What happens

1. After Accept, the controller opens the clipboard stream and sends one ready byte. It then opens the transfer stream and sends one ready byte. The host accepts the clipboard stream first, then the transfer stream. Two accepts at once can take each other's stream. Keyboard and pointer stay on the control stream.
2. The controller calls `submit_file` with the session id, generation, a relative name, the bytes, and `replace`. The file is at most 8 MiB (`MAX_FILE_BYTES`). An empty file is allowed. A larger file or an unsafe name is rejected before it is written.
3. The host writes the file only when that controller's transfer grant is on and `set_transfer_directory` has chosen a directory. The name must stay inside that directory. `..`, an absolute path, a drive prefix, and a symlink are refused. A refusal does not end the control session. The next file checks the grant again, so `authorize` during the session takes effect.
4. `replace` false leaves an existing file untouched and returns `FileExists`. `replace` true may replace a regular file. The bytes are written to a temporary sibling and renamed, so a dropped transfer leaves no partial file.
5. The host calls `publish_file` to send its own file to the controller. The same grant is required. The controller writes it under its own transfer directory. The event `TransferCompleted` names the session and does not carry the path or the bytes.
6. A dropped session does not send the old file again. The new generation needs a new `submit_file`. Disconnect and a host close do not keep the stream.

## Next direction

The current API carries a complete byte buffer and does not offer remote
directory listings, remote rename, folder jobs or resume. The linked
specification records their intended behavior and the decisions still required
before implementation. Keyboard behavior remains documented in `KEYBOARD.md`.
