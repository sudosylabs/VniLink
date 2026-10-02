# Reconnect

A dropped control session comes back for the same saved device. No new `.vnl` file is required. The rules live in `src/reconnect.rs` and `src/address.rs`. `core.rs` dials and admits.

## What happens

1. While a session is active, each computer sends its current home relay URLs and direct sockets on the control stream. The message is tag 6. It has no relay credential and no grants.
2. The receiver stores that list on the paired device whose endpoint id is the Iroh connection. A different device is not updated. A failed save leaves the previous list in place.
3. The next dial uses the stored list. Public mode sends an empty relay list. Custom mode sends the home relay URLs configured for this process.
4. If the connection ends with no close frame, the controller enters `Reconnecting`, drops queued input, waits briefly, and dials again. The host releases held keys and closes that generation before it accepts the new Hello.
5. The new session has a new generation. Input submitted for the old generation is rejected. It is not written on the new connection. A clipboard paste, a file, or finished text from the old generation is not sent again.
6. The controller tries three redials. A dead peer stops there. A session that stays up resets that count for the next drop.
7. Disconnect, shutdown, and a host close frame do not redial. Revoke still removes control, so a later dial is unauthorized.

## Files

| File | What it owns |
| --- | --- |
| `src/address.rs` | The announcement bytes and the rule that grants stay put |
| `src/reconnect.rs` | Which endings redial, and how many times |
| `src/core.rs` | The dial loop, the close frame, and storing the list |
| `src/tests/reconnect.rs` | Loopback tests, included from `src/tests.rs` |

## Still open

- Keyboard positions, the Control/Command swap, and finished text are `KEYBOARD.md`. Clipboard text is `CLIPBOARD.md`. File transfer is `TRANSFER.md`. A native window and one-monitor capture are still open. README section 11 is not complete.
