# Keyboard

A Desktop service's `submit_local_key` (`RemoteDesktopService`) is the key path. It takes the physical HID usage and the text the controller operating system produced for that press. The mode decides what the host receives. The choice belongs to this process and is not saved. Release held keys before changing it. A new desktop starts with nothing held.

## Modes

1. `KeyboardMode::Character` is the default. A typed character is inserted on the host. Shift, Alt, AltGr, and Option are not forwarded for that press, so the host does not apply its own chord on top. `@` produced by the top-left key on a Mac French keyboard and `@` produced by AltGr+`0` on a Windows or Linux French keyboard both arrive as `@`.
2. Control and Command are still keys. A shortcut such as Command+C stays a shortcut. `set_swap_control_and_command` exchanges those two before the key is sent.
3. Shift and Alt are sent only when a key that produced no character needs them, such as Shift+Left or Alt+Tab.
4. Enter, Escape, Backspace, Delete, Tab, arrows, and function keys stay physical keys. A newline or tab in the produced text does not become inserted text.
5. A press that produces no character is sent as its physical position, except an explicitly uncommitted `LocalKey::composing` step in Character/Translate mode. The application passes the characters the operating system produced, including repeats. An empty string is not a typed character.
6. `KeyboardMode::Map` sends the physical HID usage and ignores the produced text. The host layout decides the glyph. This is RustDesk's Map mode.
7. `KeyboardMode::Translate` sends a typed character as a request for the host to press the chord in the host layout. Shortcuts stay physical keys, as in Character mode. This is RustDesk's Translate mode: a key combination, not an inserted character. A Linux host reads the X11 keymap and presses Left Shift and Right Alt for group 1. A row of at most four keysyms uses those columns as plain, Shift, Right Alt, and Shift+Right Alt. A wider row follows the X server core packing: group 1 levels 1 and 2 are the first two columns, and levels 3 and 4 are columns 4 and 5. Columns 2 and 3 are the next group, or a copy of the first two, and are not used. A layout that reaches level 3 with a different key still gets Right Alt. A character that is not in group 1 is refused and the session stays up. Windows and macOS hosts refuse Translate. Tests do not open an X server. Wayland injection uses the authorized RemoteDesktop portal; Translate on that adapter is still refused.

## What a physical key does

1. The controller sends a USB HID usage on the Desktop service's stream. The host calls `enigo.raw` with the code for this operating system. macOS uses a Core Graphics keycode. Windows uses a set-1 scan code, with `0xE000` added when the key is extended. Linux uses an X11 keycode, which is the evdev code plus 8. Left and right modifiers stay distinct.
2. HID `0x32` (Non-US `#`, the ISO key next to Enter) uses the same position as backslash. HID `0x64` (Non-US `\`, the ISO key next to Left Shift) is a different key. An unknown usage is refused and the session stays up.
3. `set_swap_control_and_command` exchanges Control and Command before the event is queued. It is off by default, it belongs to this process, and it is not saved. The host injects the usage it receives and releases that same usage when the session ends. Release a held modifier before changing the setting.
4. `submit_text` sends finished characters, tag 8, at most 128 bytes (`MAX_KEY_TEXT`). The host inserts them. This is not a clipboard paste and does not use the clipboard grant. Empty text, a null byte, and a longer string are rejected before they are sent. A refusal does not end the desktop. The text is not logged and is not put on an event.
5. Translate uses tag 9. The payload has the same limit and is not logged. A refusal does not end the desktop.
6. A desktop whose connection dropped does not send the old text or the old chord again. A desktop on the new generation needs a new `submit_local_key` or `submit_text`. Queued text is dropped with the rest of the input queue.

## Files

| File | What it owns |
| --- | --- |
| `src/keyboard/plan.rs` | Character, Map, and Translate for one local key |
| `src/keyboard/chord.rs` | Group-1 chord lookup |
| `src/tests/keyboard.rs` | Plan and chord unit tests. They build the map and do not open a display |
| `src/core/desktop_stream.rs` | A Desktop service's `submit_local_key` and `submit_text`, and the host applying them |
| `src/core/keys.rs` | The engine's mode and swap defaults |
| `src/os_input/chord.rs` | Pressing that chord on Linux |
| `src/tests/keyboard_mode.rs` | Loopback tests of the three modes |
| `src/tests/keyboard_text.rs` | The Control/Command swap and finished text |

## Still open

Windows and Linux injection is not executed by a macOS test run. Enigo sets the Windows extended-key flag when the scan translates to a virtual key on its extended list. That list includes right Control, right Alt, and the arrow and navigation keys. It does not include the Windows key or keypad Enter. Translate on Windows and macOS is refused. The Linux chord lookup is unit-tested against a keymap fixture. Opening the display and pressing the chord is not run by the unit tests. Wayland injection uses the authorized RemoteDesktop portal; Translate on that adapter is still refused. A native window and one-monitor capture are still open. README section 11 is not complete.

## Pointer and composition extension (October 9, 2026)

See [the input engine contract](../../docs/architecture/input-engine.md) for
owner-scoped sensitivity/button/wheel policies, fractional scroll, relative
motion, native side buttons, cursor metadata, and outstanding platform work.
Character/Translate withhold an uncommitted composition key and inject its final
commit once; Map continues to send physical transitions. Native IME integration
and the full layout matrix remain unvalidated.
