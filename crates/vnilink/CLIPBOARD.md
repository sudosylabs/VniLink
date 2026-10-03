# Clipboard

Clipboard text is a separate grant from control. A `.vnl` file and the control confirmation do not turn it on. `Core::authorize` does. The rules live in `src/clipboard/`. `src/core/clipboard.rs` opens the stream and checks the grant.

## What happens

1. After Accept, the controller opens a clipboard stream and sends one ready byte. The host accept completes when that byte arrives. Keyboard and pointer stay on the control stream, so a paste does not block them.
2. The controller calls `submit_clipboard` with the session id and generation. The library sends UTF-8 text, at most 256 KiB (`MAX_CLIPBOARD_TEXT`). Empty text is a clear. A larger paste is rejected before it is written.
3. The host applies the text only when that controller's clipboard grant is on and a clipboard adapter is installed. A refusal does not end the control session. The next paste checks the grant again, so `authorize` during the session takes effect.
4. The host calls `publish_clipboard` to send its own text to the controller. The same grant is required. The controller reads `clipboard_text`. The event `ClipboardReceived` names the session and does not carry the characters.
5. Nothing in this path reads or writes the operating-system clipboard. The native app installs `HostClipboard` and copies text in either direction when the person asks it to.
6. A dropped session does not send the old paste again. The new generation needs a new `submit_clipboard`. Disconnect and a host close do not keep the stream.

## Still open

- Keyboard positions, the Control/Command swap, and finished text are `KEYBOARD.md`. File transfer is a separate grant; see `TRANSFER.md`. A native window and one-monitor capture are still open. README section 11 is not complete.
