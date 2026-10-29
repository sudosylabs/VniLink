# Clipboard

Clipboard text is a separate grant from control. A `.vnl` file and the control confirmation do not turn it on. `Core::authorize` does. Each Desktop service carries its own clipboard pipe: `src/core/desktop_clipboard.rs` opens it and checks the grant, and `src/clipboard/` frames the text.

## What happens

1. A Desktop opens its clipboard pipe on its own stream beside its picture and input, so a paste does not wait behind keyboard input and keyboard input does not wait behind a paste.
2. The controller calls `RemoteDesktopService::submit_clipboard`. The library sends UTF-8 text, at most 256 KiB (`MAX_CLIPBOARD_TEXT`). Empty text is a clear. A larger paste is rejected before it is written. The call returns once the host has applied or refused it.
3. The host applies the text only when that controller's clipboard grant is on and a clipboard adapter is installed (`Core::set_clipboard`). A refusal does not end the Desktop. The next paste checks the grant again, so `authorize` during the Desktop takes effect.
4. The host calls `publish_desktop_clipboard` to send its own text to one Desktop. The same grant is required. The controller reads `RemoteDesktopService::clipboard_text`. The event `DesktopClipboardReceived` names the Desktop and does not carry the characters.
5. Nothing in this path reads or writes the operating-system clipboard. The native app installs `HostClipboard` and copies text in either direction when the person asks it to.
6. A Desktop that closes, or whose connection drops, takes its pipe with it. Its replacement does not send the old paste again.

## Still open

- The workbench sends no clipboard text: the engine bridge has no command for a Desktop's clipboard yet.
- Copying files between the computers' clipboards is not offered. Files move through Files jobs; see `TRANSFER.md`.
- Keyboard positions, the Control/Command swap, and finished text are `KEYBOARD.md`. A native window and one-monitor capture are still open. README section 11 is not complete.
