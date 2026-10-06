# Native GNOME application

`vnilink-gnome` is a GTK 4 and libadwaita window linked directly to the VniLink core.

The installation record — this computer's identity and the computers paired with it — is one item in the session keyring (Secret Service, normally GNOME Keyring). The app does not write that record to a file. A locked keyring, a missing keyring, or more than one item is shown. Nothing is saved anywhere else.

The window keeps this computer and paired computers in a quiet sidebar. The main pane focuses on sharing this desktop, adding a computer, or approving a connection. Creating a `.vnl` pairing file displays a three-digit code; opening that file on another computer asks for that code. Incoming access requests similarly ask for the code shown on the connecting computer. Connection details stay in an expandable row. Native libadwaita controls adapt to light and dark themes, and a narrow window collapses to one pane. An active session has a Stop banner. Active outgoing sessions open a separate remote desktop window. It subscribes to encoded video, decodes with the Linux VA-API decoder off-thread, and uploads the latest BGRA picture to GTK. Keyboard and pointer input carry the active session generation; losing focus releases held input. Video quality adjusts automatically using the shared congestion controller; the Linux app has no manual quality picker. Full screen, diagnostics, disconnect, short text entry, and file sending are available from the session toolbar. Clipboard text synchronizes automatically in both directions during an active session, subject to the clipboard grant. Remote clipboard ownership suppresses echoes, and hiding the main window does not stop incoming clipboard writes.

## Run

GTK 4.10+, libadwaita 1.5+, and the workspace Rust toolchain:

```sh
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev
cargo run --release --locked -p vnilink-gnome
```

The record is not printed.

Use an optimized build for live desktop sessions. To measure colour conversion
on the host without recording its screen:

```sh
cargo run --release --locked -p vnilink-media --example conversion_bench -- 2560 1440 60 16.67
```

The final argument is the p95 conversion budget in milliseconds; exceeding it
returns a failure. Conversion is only one stage of streaming, so this result
does not establish end-to-end latency or visual quality.

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

Settings contains Connection (public/custom relays, up to eight URLs, optional masked authentication tokens, screen selection and identity reset), Keyboard (character, physical and translated modes, Control/Command swap), Computers (directional control/clipboard/file grants, revoke and forget), Files (saved receive folder, defaulting to Downloads/VniLink), and Pairing (one-time invitation tokens). Relay settings persist in Secret Service and apply after restart. Existing credentials remain opaque and are preserved when their URL is unchanged; turning authentication off removes them.

Choose a screen before hosting video. X11 reads the current desktop and installs native input. Wayland uses one RemoteDesktop portal session for its screen, keyboard and pointer grant, with PipeWire providing capture. Both keyboard and pointer must be granted; a refused or partial grant is closed and does not install input. VA-API encode/decode hardware is required. The encoder requires VBR or CBR rate control so picture-quality bitrate limits are applied by the driver. Capture failures stop the hosted session and show an error.

Screen selection currently starts local capture immediately. GNOME's sharing
indicator therefore appears before a remote connection; it reports the local
portal capture, not an established VniLink connection. The application does not
yet defer portal setup until a connection request or restore prior screen grants.
An unchanged Wayland desktop reuses its latest captured picture instead of
ending the hosted session after a two-second frame timeout. Genuine capture
failures still end the session; the Linux terminal prints the underlying reason
as well as showing it in a toast.

The visual tour includes the settings pages and a synthetic pixel-upload test for the remote desktop window. It verifies geometry, alpha, and cleanup, and relay saves against a memory installation store. This does not establish a real two-machine remote desktop result.

Paired computers display a local name when set, otherwise the hostname learned from their invitation (refreshed on connection), with an endpoint fragment as fallback. Settings → Computers → Manage lets you save a local name or clear it to use the hostname. Names persist in the keyring record and do not change permissions.

## Background and login startup

Settings → Background contains **Keep running when the window closes** (on by
default), **Launch at login** (off by default), and **Share X11 desktop and allow
connections on launch** (off by default). Closing the window in background mode
retains the same core and current capture; reopening reuses that window. Quit
stops capture and shuts down the core. The tray provides Open, Settings, Stop
remote access and Quit, with a status title for hosting/active incoming sessions.

The tray uses StatusNotifierItem/D-Bus menus through `ksni`. KDE and desktops
with an AppIndicator/StatusNotifier host can show it. GNOME needs a compatible
tray extension. If no tray host is available the app shows its window; background
mode is still available, and launching VniLink again reopens the existing process.

Launch at login writes only this user's
`$XDG_CONFIG_HOME/autostart/com.vnilink.VniLink.desktop` (normally under
`~/.config`), pointing to the current executable with `--background`. Disabling
it removes that entry. Keep the installed binary at the registered path; turn
launch at login off/on after moving it. X11 sharing can resume explicitly;
Wayland still needs a fresh RemoteDesktop portal grant after restarting.
Startup preferences contain no credentials; identity and relay tokens remain in
Secret Service. An unlocked desktop session and codec hardware are required.

For a local installation, build the executable and install the launcher before
enabling launch at login:

```sh
cargo build --release --locked -p vnilink-gnome
install -Dm755 target/release/vnilink-gnome ~/.local/bin/vnilink-gnome
install -Dm644 linux/data/com.vnilink.VniLink.desktop ~/.local/share/applications/com.vnilink.VniLink.desktop
```

Ensure `~/.local/bin` is on the desktop session's PATH. The background lifecycle
test uses an isolated configuration directory and covers close/reopen, login
entry creation/removal and quitting with background mode disabled. It needs
D-Bus and a display (`dbus-run-session -- xvfb-run -a cargo test --locked
-p vnilink-gnome -- --test-threads=1`). A real desktop tray and login cycle still
need manual validation.

Wayland capture currently requests CPU-mappable MemFd/MemPtr buffers and rejects
DMA-BUF delivery. A real-machine backtrace showed a Mesa heap-corruption abort
inside `gbm_bo_unmap`; the production portal no longer probes or maps through
GBM. This is a capture-path mitigation, not evidence that the original driver
corruption is repaired. The producer must support shared-memory delivery; live
capture with the changed negotiation still needs validation on that desktop.

## Wayland control

The desktop must implement both RemoteDesktop and ScreenCast portals. In
Settings → This computer, choose **Enable desktop sharing and control**, then
approve one monitor, keyboard and pointer in the compositor's dialog. GNOME
shows local capture as active immediately; setup is still required before a
connection. Portal permissions are not saved or resumed silently. VniLink's
paired-device grants continue to authorize each remote connection.

Keyboard HID usages map to Linux evdev codes; buttons, absolute pointer and
wheel detents use the portal's D-Bus Notify methods. Pointer pixels scale to the
selected stream's compositor coordinates, including scaled displays. Character
mode and explicit short text use keysyms; physical-key mode uses evdev codes.
Translate mode remains the X11-layout implementation and is refused on Wayland.
When the portal supports cursor metadata, capture keeps the cursor out of video.
Compatible Mac and GTK viewers render its lossless shape as a native local cursor;
motion does not wait for H.264 decoding. Shapes and host positions travel on a
bounded independent stream, fenced by the session generation. Older viewers get
the cursor composited back into video. Portals without metadata keep embedded
capture. X11 does not yet provide independent cursor metadata.
A dedicated worker owns the portal and
serializes input without nesting Tokio runtimes. Stop remote access and Quit
close the grant; cancellation and failed capture setup close it too. Closing a
window with background mode enabled keeps the current sharing session alive.

Tests cover input mapping, revocation and a fake portal on a private D-Bus bus,
including combined session creation, partial-grant rejection and close. Run the
transport test without affecting a real desktop:

```sh
VNILINK_TEST_PORTAL_BUS=private dbus-run-session -- \
  cargo test --locked -p vnilink-linux real_portal_notify_transport_preserves_all_events -- --ignored
```

The app compiles on Linux. A live Ubuntu 24.04 GNOME host → Mac session renders
the desktop and delivers independent cursor metadata, including the view-only
cursor overlay. This does not establish the full platform matrix or RustDesk
performance parity. The isolated service does not verify compositor behavior.

File sending supports a native file picker and dropping one local file onto the remote desktop. Received files go into the saved receive folder, and the app reports completion. The shared protocol still limits individual files to 8 MiB and preserves existing files. Folder transfers, resumable bulk transfers, rich clipboard formats and copying files through the OS clipboard are not implemented; this is not yet RustDesk file-manager parity.
