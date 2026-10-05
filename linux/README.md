# Native GNOME application

`vnilink-gnome` is a GTK 4 and libadwaita window linked directly to the VniLink core.

The installation record — this computer's identity and the computers paired with it — is one item in the session keyring (Secret Service, normally GNOME Keyring). The app does not write that record to a file. A locked keyring, a missing keyring, or more than one item is shown. Nothing is saved anywhere else.

The window keeps this computer and paired computers in a quiet sidebar. The main pane focuses on sharing this desktop, adding a computer, or approving a connection. Creating a `.vnl` pairing file displays a three-digit code; opening that file on another computer asks for that code. Incoming access requests similarly ask for the code shown on the connecting computer. Connection details stay in an expandable row. Native libadwaita controls adapt to light and dark themes, and a narrow window collapses to one pane. An active session has a Stop banner. Active outgoing sessions open a separate remote desktop window. It subscribes to encoded video, decodes with the Linux VA-API decoder off-thread, and uploads the latest BGRA picture to GTK. Keyboard and pointer input carry the active session generation; losing focus releases held input. Full screen, quality limits, diagnostics, disconnect, explicit clipboard text, short text entry, and file sending are available from the session toolbar.

## Run

GTK 4.10+, libadwaita 1.5+, and the workspace Rust toolchain:

```sh
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev
cargo run -p vnilink-gnome
```

The record is not printed.

## Layout

Domain modules own state and unit tests; the GTK layer only renders:

- `error.rs` — user-facing sentences for core and keyring errors.
- `capabilities.rs` — capability state to explicit video cards.
- `devices.rs` — paired-computer rows and selection.
- `pairing.rs` — pairing flow steps (offer, open file, waiting digits).
- `settings.rs` — settings state read off a snapshot.
- `events.rs` — live UI state: control request, connecting flag, toasts.
- `presentation.rs` — user-facing words.
- `storage.rs` — the installation record in the session keyring.
- `app/` — GTK shell: `window` composes `sidebar` and `pages`;
  `dialogs`, `notifications`, and `widgets` support them.

Unit tests run anywhere (`cargo test -p vnilink-gnome`). The GTK widget
suite needs a display; headless:

```sh
sudo apt install xvfb adwaita-icon-theme
xvfb-run -a cargo test -p vnilink-gnome
```

The ignored visual tour renders the real GTK widgets using fixture snapshots,
including pairing, approval, light/dark themes, and 360-pixel navigation:

```sh
VNILINK_UI_SCREENSHOTS=/tmp/vnilink-shots xvfb-run -a cargo test -p vnilink-gnome screenshot_tour -- --ignored --test-threads=1
```

These captures verify presentation, not an end-to-end remote connection.

## Settings and capture

Settings contains Connection (public/custom relays, up to eight URLs, optional masked authentication tokens, screen selection and identity reset), Keyboard (character, physical and translated modes, Control/Command swap), Computers (directional control/clipboard/file grants, revoke and forget), Files (receive folder for this launch), and Pairing (one-time invitation tokens). Relay settings persist in Secret Service and apply after restart. Existing credentials remain opaque and are preserved when their URL is unchanged; turning authentication off removes them.

Choose a screen before hosting video. X11 reads the current desktop and installs native input. Wayland asks through the ScreenCast portal and opens PipeWire capture; its grant provides viewing only because the current input adapter does not inject Wayland input. VA-API encode/decode hardware is required. Capture failures stop the hosted session and show an error.

The visual tour includes the settings pages and a synthetic pixel-upload test for the remote desktop window. It verifies geometry, alpha, and cleanup, and relay saves against a memory installation store. This does not establish a real two-machine remote desktop result.

Paired computers display a local name when set, otherwise the hostname learned from their invitation (refreshed on connection), with an endpoint fragment as fallback. Settings → Computers → Manage lets you save a local name or clear it to use the hostname. Names persist in the keyring record and do not change permissions.
