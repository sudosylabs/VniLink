# Clipboard

Clipboard text is a separate grant from control. A `.vnl` file and the control confirmation do not turn it on. `Core::authorize` does. The rules live in `src/clipboard/`. `src/core/clipboard.rs` opens the stream and checks the grant.

## What happens

1. After Accept, the controller opens a clipboard stream and sends one ready byte. The host accept completes when that byte arrives. Keyboard and pointer stay on the control stream, so a paste does not block them.
2. The controller calls `submit_clipboard` with the session id and generation. The library sends UTF-8 text, at most 256 KiB (`MAX_CLIPBOARD_TEXT`). Empty text is a clear. A larger paste is rejected before it is written.
3. The host applies the text only when that controller's clipboard grant is on and a clipboard adapter is installed. A refusal does not end the control session. The next paste checks the grant again, so `authorize` during the session takes effect.
4. The host calls `publish_clipboard` to send its own text to the controller. The same grant is required. The controller reads `clipboard_text`. The event `ClipboardReceived` names the session and does not carry the characters.
5. Nothing in this path reads or writes the operating-system clipboard. The native app installs `HostClipboard` and copies text in either direction when the person asks it to.
6. A dropped session does not send the old paste again. The new generation needs a new `submit_clipboard`. Disconnect and a host close do not keep the stream.

## File selections

The same stream and the same clipboard grant carry file selections in either
direction. A selection is metadata only: relative jailed paths, size estimates
and directory markers, at most 256 entries and 64 KiB encoded. No payload byte
rides this path, so offering a selection never downloads anything.

1. The controller calls `submit_clipboard_files` with the session id,
   generation and selection. The host stores it on that hosted session, bumped
   to a fresh revision, and publishes `ClipboardFilesReceived`, which carries
   no paths. No text adapter is needed; file offers store, they never call
   `HostClipboard`.
2. The host calls `publish_clipboard_files` for the selected session only.
   The controller stores it and the native side reads `clipboard_files` or
   `session_clipboard_files`. `incoming_clipboard_files` reads what a
   controller offered to this host.
3. Selections are replaced, not queued. An empty selection clears the peer's
   copy but still bumps the revision, so a paste pinned to any older revision
   stays stale. A dropped session starts the next generation without
   a selection.
4. Withdrawing the clipboard grant or revoking the peer drops that peer's
   stored selections on this core. Unlike inert text, a selection stays a live
   paste capability after the grant is gone. The far side keeps inert metadata
   it already received; any paste against it revalidates with this side.
5. Bounds and jailed paths are validated before anything is sent, and decode
   revalidates: truncation, trailing bytes, bad flags, non-UTF-8 names,
   duplicates and oversized selections fail closed.

Selections ride the session clipboard stream only; the per-Desktop-owner
stream rejects them.

## Pasting a selection

A paste never fetches from selection metadata alone. Pasting an offered
selection moves its bytes through Files jobs, which need the Transfer grant:
the clipboard grant covers the offer, the Transfer grant covers the bytes.
Revisions let a paste name the exact offer it pins, and a source serves only
paths inside its latest own offer (`FileSelection::covers`). Native code owns
destination choice, conflict handling and progress UI.

Native file-manager providers per OS are still open.

## Still open

- Keyboard positions, the Control/Command swap, and finished text are `KEYBOARD.md`. File transfer is a separate grant; see `TRANSFER.md`. A native window and one-monitor capture are still open. README section 11 is not complete.
