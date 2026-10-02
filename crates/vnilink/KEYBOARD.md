# Keyboard

A key names a physical position. HID usage `A` is the key a US keyboard labels A. The host injects that position, and the host's own layout produces the character. The map lives in `src/os_input.rs`. Finished text and the Control/Command swap live in `src/input.rs` and `src/core.rs`.

## What happens

1. The controller sends a USB HID usage on the control stream. The host calls `enigo.raw` with the code for this operating system. macOS uses a Core Graphics keycode. Windows uses a set-1 scan code, with `0xE000` added when the key is extended. Linux uses an X11 keycode, which is the evdev code plus 8. Left and right modifiers stay distinct.
2. HID `0x32` (Non-US `#`, the ISO key next to Enter) uses the same position as backslash. HID `0x64` (Non-US `\`, the ISO key next to Left Shift) is a different key. An unknown usage is refused and the session stays up.
3. `set_swap_control_and_command` exchanges Control and Command before the event is queued. It is off by default, it belongs to this process, and it is not saved. The host injects the usage it receives and releases that same usage when the session ends. Release a held modifier before changing the setting.
4. `submit_text` sends finished characters, tag 8, at most 128 bytes (`MAX_KEY_TEXT`). The host types them. This is not a clipboard paste and does not use the clipboard grant. Empty text, a null byte, and a longer string are rejected before they are sent. A refusal does not end the control session. The text is not logged and is not put on an event.
5. A dropped session does not send the old text again. The new generation needs a new `submit_text`. Queued text is dropped with the rest of the input queue.

## Still open

Windows and Linux injection is not executed by a macOS test run. Enigo sets the Windows extended-key flag when the scan translates to a virtual key on its extended list. That list includes right Control, right Alt, and the arrow and navigation keys. It does not include the Windows key or keypad Enter. Wayland input is not claimed. A native window and one-monitor capture are still open. README section 11 is not complete.
