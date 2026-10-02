# Pairing with a `.vnl` file

Two computers meet by handing each other a file. The signed invitation inside the file is the proof. Connect, after that, uses the Iroh relays this installation already has. There is no pairing server and no mailbox.

## What the person does

1. On the computer that will be controlled, turn on Allow control and export a pairing file. The screen shows three digits. The app saves the bytes as a file ending in `.vnl`.
2. Move that file with AirDrop, a message, email, or a USB stick. Iroh cannot carry this first file, because neither computer knows the other's address yet.
3. On the other computer, open the file. The screen shows the same three digits. Type the digits from the computer that wrote the file. This saves that computer so Connect can dial it. It does not grant control.
4. Connect. The controller's screen shows a second set of three digits, from `control_check`. The controlled computer shows the same digits and asks for them. Type those digits there. Control is granted for this controller only. Clipboard and file transfer stay off.
5. Later, Connect uses the saved device. The file is not asked for again while the saved addresses still work.

Allow control is a switch on the computer that exports the file. It is not stored in the file, so editing the file cannot turn it on. The switch lasts until the invitation expires (at most 15 minutes), until one controller is granted, or until the process stops. A restart needs a new file.

The host has about one minute to type the controller's digits after Connect starts. Reject, or a mismatch, ends that attempt. The file can be used for another attempt until it expires.

## What the library does

1. `Core::export_pairing_file` signs the same `vnl1.` invitation as `create_invitation` and wraps it in `VNLF` version 1 bytes. The app writes those bytes. The library does not choose a path.
2. `Core::open_pairing_file` checks the signature and expiry. `Core::confirm_pairing` calls `accept_invitation` when the typed digits match. A mismatch emits no event.
3. The file digits are the first three decimal digits of a SHA-256 over the writer's endpoint id, the invitation nonce, and the expiry. `pairing/check.rs` owns that layout.
4. `Core::connect` dials the saved relays and direct addresses. When the host's switch is on, an unknown controller is held for up to 60 seconds instead of being rejected at once. `Core::confirm_control_request` saves that authenticated endpoint and grants control. The control digits are a different hash, over the controller's endpoint id only.
5. With the switch off, or with no unexpired export, an unknown controller is rejected immediately.

## Files

| File | What it owns |
| --- | --- |
| `src/pairing/mod.rs` | The steps above |
| `src/pairing/file.rs` | `.vnl` bytes, `PairingFile`, `PairingPreview` |
| `src/pairing/check.rs` | The two three-digit checks |
| `src/core.rs` | `export_pairing_file`, `open_pairing_file`, `confirm_pairing`, `control_check`, `confirm_control_request` |
| `src/tests/pairing.rs` | Loopback tests, included from `src/tests.rs` |

`core.rs` is still one file. The file format and the digits live in `src/pairing/`. The wait for the host's answer sits beside the session handshake because that is the code that accepts or rejects a connection.

## Still open

- Keyboard positions, the Control/Command swap, and finished text are `KEYBOARD.md`. A native window and one-monitor capture are still open. README section 11 is not complete.
- Clipboard and file transfer stay off in this flow. A later `authorize` can turn them on. Clipboard is `CLIPBOARD.md`. File transfer is `TRANSFER.md`. A dropped session dials the saved device again; that path is `RECONNECT.md`.
