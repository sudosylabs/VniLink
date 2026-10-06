# VniLink

VniLink is a remote desktop application for controlling your own computers across the internet. It will use a shared Rust library and a native desktop application on each supported platform.

Anyone can install it and use their own devices. Device identity, paired devices, permissions, and relay settings belong to that installation. The owner's three computers are the first validation set.

The native [macOS app](apple/README.md) has a computer browser, Keychain-backed pairing, settings, a separate VideoToolbox desktop viewer, and explicit ScreenCaptureKit/input setup. Synthetic video over a paired loopback session is verified; real-machine Mac capture and control still need validation.

See [Contributing](CONTRIBUTING.md) to work on VniLink,
[Security](SECURITY.md) to report a vulnerability privately, and the
[Code of Conduct](CODE_OF_CONDUCT.md) for community expectations and incident reporting.

The [file transfer window specification](docs/architecture/file-transfer.md)
records the agreed direction: native two-pane browsing, transfers in both
directions using `iroh-blobs`, and filesystem renaming of files and folders.
These are planned capabilities; the current single-file transfer contract is
documented in [TRANSFER.md](crates/vnilink/TRANSFER.md).
The [implementation handoff](docs/handoffs/file-transfer-implementation.md)
provides the build sequence, acceptance tests, Linux Docker workflow and
RustDesk comparison checklist for the agent implementing this feature.

**Status:** implementation started October 1, 2026. `crates/vnilink` is a shared control library pinned to iroh 1.3.0 (Rust 1.91). It owns installation identity, public or custom relay selection, signed invitations, directional grants, one control session per host, and keyboard and pointer input. `cargo test -p vnilink` passes, including two cores on loopback. An authorized controller can send key, button, wheel, and pointer events. The host applies them when it calls `enable_os_input`. Pointer positions collapse to the latest sample. Held keys and buttons are released when the session ends. With an `InstallationStore`, a restart keeps the endpoint id, paired devices, grants, used invitations, and relay settings. The library does not open Keychain, DPAPI, or Secret Service. `Core::start` without a store still keeps that state only for the process. Custom relay mode does not publish through n0 address lookup. The library carries encoded H.264 frames over QUIC datagrams with a reliable-stream fallback and drops predicted frames until the next keyframe. Protocol version 2 negotiates H.264 High profile (CABAC and decoder support for 8x8 transforms) and falls back to a version-1 Baseline hello for older hosts. `vnilink-media` paces BGRA pictures and requires an Annex-B keyframe to carry SPS, PPS, and an IDR. `vnilink-macos` encodes and decodes a synthetic picture with VideoToolbox. Its tests build a one-monitor ScreenCaptureKit configuration and do not start capture or record the screen. `vnilink-windows` describes one interactive-desktop monitor and excludes the secure desktop; Desktop Duplication and Media Foundation compile only as Windows code. `vnilink-linux` keeps X11 and the Wayland portal separate. On Linux, H.264 encode opens VA-API High profile when the session negotiates it, else Constrained Baseline, on the first DRM render node that offers it and returns Annex-B; a machine where no render node offers that entrypoint still reports that VA-API is unavailable. The portal path reads one monitor from the PipeWire node it is given, as BGRA or BGRx. The live portal negotiates shared-memory buffers and refuses DMA-BUF delivery after an observed Mesa GBM-unmap abort. Tests do not open the portal or an X server. A test run on macOS does not execute the Windows or Linux capture paths. Pairing starts from a `.vnl` file one computer writes and the other opens. The file holds a signed invitation. The opener types the three digits shown on the writer's screen, and the writer types the controller's digits when Connect starts. No pairing server is required. A dropped control session dials the saved device again with a new generation and does not replay the old input. While a session is up, each side stores the other's current home relay and direct addresses. Clipboard text is a separate grant, limited to 256 KiB, and the library does not read the operating-system clipboard. File transfer is a separate grant: at most 8 MiB, a relative path inside a directory chosen for that installation, and no replace unless the sender asks. The default keyboard mode inserts the character the controller's operating system produced, so a typed `@` arrives as `@`. Map mode sends the physical key, and Translate mode asks a Linux host to press the chord in its X11 layout. Windows and macOS hosts refuse Translate. Tests do not open an X server for that lookup. Left and right modifiers stay distinct. The controller can exchange Control and Command, and it can send finished text of at most 128 bytes separately from the clipboard. The GNOME application in `linux/` keeps the installation record in the session keyring. This computer creates a `.vnl` pairing file and shows the three digits; another computer opens that file, types the digits, and connects. The computer that shared the file types the controller's digits to allow control. The Linux application now wires screen selection, VA-API decode, a remote desktop window, settings, and input to the shared core. GTK tests render synthetic pixels and exercise settings persistence; a real two-machine desktop session is still unverified. There is no deployment or measured remote desktop. The gates in section 11 are not complete. FileVault preboot, the Windows secure desktop, and a universal Wayland session are not claimed. Sections below still describe the product to build except where they name this library or these crates.

## 1. Product requirements and decision status

### Confirmed by the owner

- Name: **VniLink**.
- Three initial devices: a Mac mini at home, a Windows laptop, and a Linux laptop.
- Every device must be able to control either of the other two: all six directed combinations must work.
- Access must work across the internet, including different networks, rather than only on a home LAN.
- The application should be lightweight.
- The shared Rust part must be a **library**. Native platform applications host it; there is no shared web or cross-platform UI requirement.
- Follow the structural approach of **VniDrop**, with native platform directories and a shared Rust core.
- Build a distinct product rather than shipping Tailscale or RustDesk as the application.
- Each installation uses its own configuration, so other people can install VniLink and point it at their own devices and relays.
- The default relay is the public n0 relay set from the pinned Iroh release. In iroh 1.3.0 that is `RelayMode::Default`, also applied by `presets::N0`. Installing, pairing, and connecting works with those public relays alone.
- An installation may replace that default with one or more custom relay URLs. In iroh 1.3.0 that is `RelayMode::Custom`. The override is local configuration.

### Proposed defaults for implementation

These are recommended starting choices, not additional owner-approved requirements:

- Iroh for authenticated encrypted transport, NAT traversal, and relay fallback.
- SwiftUI on macOS, WinUI 3 on Windows, GTK/libadwaita on Linux, matching the inspected VniDrop layout.
- Explicit device pairing and per-target authorization for unattended control.
- One active controlling session per target initially; a device may host a session while controlling another device if resource limits permit.
- One monitor and keyboard/mouse control for the first end-to-end milestone.
- H.264 as the initial video codec candidate, with hardware acceleration preferred and a tested fallback.
- A device directory only if the discovery design requires one. Relay choice is the confirmed default above.

### Resolve before claiming complete support

Record decisions and platform evidence for:

1. Linux distribution, desktop environment, compositor, and Wayland versus X11.
2. OS versions and CPU/GPU architectures of all three machines.
3. Whether required access includes locked screens, pre-login access after reboot, elevated Windows applications/UAC, and a monitorless Mac mini.
4. Desired clipboard, file transfer, audio, multiple-monitor, and wake functionality.
5. For a custom relay, that operator's traffic accounting, CPU and network throughput, domain/TLS, and existing workloads. VniLink does not provision a relay.
6. Address refresh after pairing for a custom relay. The library pins iroh 1.3.0: public mode is `RelayMode::Default` via `presets::N0`, and custom mode is `RelayMode::Custom`. Custom mode clears n0 address lookup, so a peer is dialed from the relay URLs and direct addresses in the signed invitation. Refreshing those addresses later, and reaching a credentialed relay, still require each installation to hold that relay in its own configuration. The builder signature has changed across releases.

Progress on independent foundations while these details are unresolved. Report concrete limitations rather than assuming platform capabilities.

## 2. User experience

Each installation has two roles: **controller** (displays another computer and forwards input) and **host** (shares its desktop and applies authorized input). Roles are per session, not separate products or builds.

The main window should show this installation, its permission/setup status, relay mode, and saved devices. A saved device should expose availability, authorization, and a connect action. Distinguish offline, connecting, permission missing, authentication rejected, and unsupported desktop states.

Relay mode is either the public default or the installation's custom relay URLs. Changing it is a local action saved at once; the running endpoint keeps the relay it bound at start, so the new setting binds on the next open. Pairing files and address announcements carry the bound relay. Pairing and address refresh include the current home relay so a peer can dial it.

Pairing must require a deliberate local action and authenticate the exchanged identities. After pairing, the target owner may enable unattended access for that specific controller. Pairing alone should not silently enable remote control or transfer privileges.

The first pairing can start from a `.vnl` file, moved by AirDrop, a message, email, or a USB stick. The computer that will be controlled writes it and shows three digits. The other computer opens it and types those digits. On Connect, the controlled computer types the three digits shown on the controller before control is granted. Clipboard and file transfer stay off. After that, Connect uses the saved device. `crates/vnilink/PAIRING.md` is the step list.

During a session show the remote desktop, connection state, direct/relay path when known, display selection when implemented, quality controls, and disconnect. The host must show an active-session indicator and provide a local stop action. Local input should remain usable; exclusive control is a separate future decision.

Closing the visible window and disabling hosting are different actions. When unattended access is enabled, a platform host process must continue running with the authorized configuration. Provide explicit enable/disable hosting and startup controls.

## 3. Repository structure

Present today: `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, the crates under `crates/`, and the GNOME application in `linux/`. Library tests live in each crate's `src/tests` and are included from the crate, the same layout VniDrop uses. Pairing steps for the `.vnl` file are in `crates/vnilink/PAIRING.md`. `apple/` and `windows/` are still the proposed layout. Create a directory when it acquires content.

```text
vnilink/
  README.md
  Cargo.toml                    # Rust workspace
  Cargo.lock
  rust-toolchain.toml           # Rust 1.91.0
  crates/
    vnilink/                    # Shared control library. Input and encoded frames
    vnilink-media/              # Annex-B, frame pacing, one-monitor plan
    vnilink-macos/              # VideoToolbox, ScreenCaptureKit configuration
    vnilink-windows/            # Desktop Duplication and Media Foundation on Windows
    vnilink-linux/              # X11 capture, Wayland portal, PipeWire frames, VA-API H.264
    vnilink-protocol/           # Wire types/validation, if a separate crate earns its cost
    vnilink-ffi/                  # UniFFI control-plane bindings for SwiftUI/WinUI
    uniffi-bindgen/             # Binding generation tooling, pinned to the same UniFFI
  apple/                        # SwiftUI macOS app and host lifecycle
  windows/                      # WinUI app and host/service lifecycle
  linux/                        # GTK 4 / libadwaita app. Keyring record, pairing file, connect
  services/
    relay/                      # Optional custom-relay deployment notes; unused by the public-relay default
  docs/
    architecture/              # Decisions, protocol, trust model, media boundaries
    platforms/                 # Permissions, startup, supported OS/desktop states
    validation/                # Reproducible test matrix and measured results
  scripts/                      # Build, bindings, packaging, verification helpers
  packaging/                    # Platform packaging where not native-project-owned
```

VniDrop is a structural reference, not a template to copy wholesale. The accessible checkout inspected during planning was `/Users/hammed/Projects/vnidrop`; the owner originally named `/home/hammed/Projects/vnidrop`. Locate the actual checkout in the implementation environment before using it. Its README describes SwiftUI, WinUI, and GTK/libadwaita; its Rust crate supports `rlib`, `staticlib`, and `cdylib` and uses UniFFI. Inspect its real platform bridges before choosing VniLink bindings, particularly Windows. Do not copy mobile targets, transfer-consent rules, dependencies, or build machinery without a VniLink need.

## 4. Component ownership

### Shared Rust library

Own device identities, trusted-device records, authorization policy, protocol negotiation, Iroh endpoints, session state, reconnect logic, stream scheduling, bounded queues, transfer state, and structured errors/events.

The crate currently implements identity, relay selection, invitations, grants, session admission, one incoming and one outgoing control session, a bounded event stream, keyboard and pointer input, and encoded H.264 frames over datagrams with a reliable-stream fallback. A full video queue drops predicted frames until the next keyframe. The platform crates capture and encode those frames; `vnilink-media` owns the shared hosting policy behind narrow seams — frame pacing (`FramePump`, `HostVideo`), the hosted session (`HostSession`, with the platform encoder behind `VideoEncoder` and the capture behind `FrameSource`), latest-frame delivery (`FrameChannel`), and the encoder settings derived from the session quality (`RateController`). Clipboard and transfer each run one side stream beside control and video and share one establishment handshake. When the host provides an `InstallationStore`, the crate saves the installation secret, directional trust, grants, used invitations, and relay settings. A dropped session reconnects with a new generation. Clipboard text is a separate grant, limited to 256 KiB. File transfer is a separate grant, limited to 8 MiB, and writes only under a directory the installation chooses.

Input events use USB HID keyboard usages for keys, separate press and release for buttons, absolute pixels on the one host monitor for the pointer, and wheel detents for scroll. `Core::enable_os_input` injects those events with the operating system's input API (Core Graphics on macOS, SendInput on Windows, X11 on Linux) through the MIT-licensed `enigo` crate. The injected value is a physical keycode for that system, not the US character for the HID usage. `submit_local_key` is what a controller calls for a key. The default mode inserts the character the controller operating system produced. Map mode sends the physical key. Translate mode asks the host to press the chord in its own layout; a Linux host reads the X11 keymap, and Windows and macOS hosts refuse that mode. `set_swap_control_and_command` exchanges Control and Command on the controller before a physical key is sent. `submit_text` types a finished string of at most 128 bytes and is not a clipboard paste. VniLink does not use RustDesk's UI, screen-mirroring pipeline, or source code. The host must grant input permission. macOS shows the Accessibility prompt. Wayland uses a separate `WaylandCapture`/`HostDesktop` portal adapter rather than `enable_os_input`; it requires a compositor RemoteDesktop grant. Elevated Windows desktops and the secure login screen are not claimed. Without `enable_os_input`, events are refused and the session stays open. The shared library carries encoded frames and does not capture or draw. `vnilink-macos` can encode and decode a synthetic picture. It has not captured this screen, the Linux window now paints decoded BGRA pictures, but end-to-end desktop operation remains unverified.

Expose semantic operations such as start hosting, pair, authorize device, connect, disconnect, revoke device, unpair device, update session quality, and query capabilities. Native clients observe state/events rather than reconstructing session rules.

Give the runtime a defined owner. Specify startup/shutdown, callback threading, cancellation, event ordering, and how a native UI marshals updates onto its UI thread. Slow subscribers must not block networking or allocate unbounded event queues.

### Native applications and adapters

Own UI, local permission flows, OS startup integration, capture/input APIs, display enumeration, encoder/decoder integration, rendering, and platform credential-store access.

Define a capability interface so the Rust library can negotiate what the host actually supports. Represent unsupported features explicitly. Platform adapters can themselves contain Rust where practical; native UI does not require all platform logic to be written in the UI language.

### Background hosting

The shared library remains a library. A platform executable, user agent, or service loads it for background operation. Privileged helpers are separate processes only where needed; a privileged process should expose a narrow authenticated local interface rather than parsing all network traffic with broad privileges.

Service installation does not automatically provide access to a user's graphical session. Design and validate the service-to-session relationship separately on each OS. Protect local IPC against unauthorized local clients and validate message sizes and caller identity.

## 5. Native bindings and media boundary

Select bindings after checking actual support for Swift, Windows application language, and Rust Linux callers. UniFFI is the control-plane binding: `crates/vnilink-ffi` exposes the session, pairing, grant, input, clipboard, transfer, and video-frame API as records, enums, and byte vectors. The contract is in `docs/architecture/ffi-contract.md`.

Document ownership, lifetime, threading, error mapping, and cancellation for each boundary. Do not expose internal async types or networking implementation details as the native API.

Use a dedicated media interface rather than serializing raw full-resolution video frames through ordinary high-level bindings. Keep capture, hardware encoding, decoding, and GPU rendering close to the platform APIs. Send encoded buffers into/out of the transport with explicit ownership and bounded lifetimes; evaluate shared/native surfaces where feasible, but verify rather than promise zero-copy operation.

The host captures and encodes; the controller decodes and renders. A relay forwards ciphertext and never transcodes video.

## 6. Connectivity and relays

Use Iroh endpoints on each device. Prefer direct connections; retain encrypted relay fallback when NAT or firewall conditions prevent a direct path. No router port forwarding should be required for ordinary device operation. Validate restrictive networks instead of claiming universal reachability.

**Default.** Use the public relay set shipped with the pinned Iroh release. In iroh 1.3.0 that is `RelayMode::Default` (production relays from n0, also applied by `presets::N0`). A new installation needs no relay URL, account, or API key. n0 documents these relays as rate-limited and recommends them for development and testing rather than production workloads. That limit is why a custom relay exists. VniLink still defaults to the public relays so a person can install and connect without operating a server. Say so in setup notes. Treat the public relays as a shared, rate-limited fallback, not as an unlimited production service.

**Custom.** Each installation can replace the default with one or more relay URLs. The library passes that list as `RelayMode::Custom` for the pinned release. An empty list is invalid and must be rejected with a clear error. URLs come from installation configuration and are read at startup. A self-hosted `iroh-relay`, or another relay compatible with the pinned release, is a valid override. If a chosen relay requires a credential, store it in the installation config or the platform credential store, and keep it out of logs. Public-relay mode stores no relay credential.

Each installation keeps its own relay setting. Pairing and later address refresh include the current home relay URL so the peer can dial it. Dialing uses that URL from the peer address. An open custom relay can be reached by a peer that remains on the public default, when the URL is reachable. A relay that requires a credential is different: Iroh attaches that credential from the dialer's own relay map. Installations that should fall back through a credentialed relay must each be configured with that relay and its credential. Recheck this against the pinned release. Knowing a relay or an address never authorizes a session.

The library pins iroh 1.3.0. Public mode keeps n0 address lookup. Custom mode clears it, so endpoint identity alone does not make a custom-relay peer dialable. The signed invitation carries that peer's current relay URLs and direct addresses. After a session is accepted, each computer sends its current home relay URLs and direct addresses on the authenticated control stream. The receiver stores that list for that peer only. The announcement has no relay credential and does not change grants. A later dial, including a reconnect after a dropped link, uses the stored list. The new session has a new generation and does not send input queued for the old one. `crates/vnilink/RECONNECT.md` is the step list.

Choose either explicit paired-device address publication or a small authenticated directory. Add a directory only when the discovery design requires one. Keep directory trust separate from permission to control.

Running a relay is optional. The default install uses the public relays. Operators who want a private relay configure domain and TLS, required ports, health checks, service startup, logs, updates, and abuse protection themselves. Relay access controls and resource limits must match the pinned release; avoid claiming a configuration option exists without checking it. Deployments, paid services, provider changes, and opening infrastructure access require their own task authorization. This README is not permission to provision or deploy.

The owner's VPS, with 12 GB RAM and a stated 10 TB monthly allowance, is one possible custom relay for that owner. Other installations do not use it.

### Bandwidth budget

Anyone operating a relay can use these planning figures. For a 31-day month (744 hours), a continuous one-direction stream uses approximately:

| Average bitrate | Decimal TB/month |
| --- | ---: |
| 2 Mbps | 0.67 |
| 5 Mbps | 1.67 |
| 10 Mbps | 3.35 |
| 20 Mbps | 6.70 |
| 30 Mbps | 10.04 |

These figures exclude protocol overhead, audio, transfers, and other workloads. Relayed traffic enters and leaves the relay host: double the approximate accounting if the provider counts both ingress and egress. Direct desktop traffic bypasses the relay. Simultaneous streams add together. As an illustration, 10 TB supports about 29.9 Mbps continuously with outbound-only accounting, or 14.9 Mbps with combined accounting, before overhead. Public n0 relays apply their own rate limits. A self-hosted machine's RAM or traffic allowance does not by itself prove it can carry desktop sessions. Actual allowance and utilization belong to that operator and must be verified.

Provide configurable bitrate limits and direct/relayed byte counters. `Core::session_paths` reports per-path relay/direct selection, round trips, and cumulative UDP payload bytes for diagnostics.

## 7. Protocol and session lifecycle

Use a versioned application protocol with explicit limits and capability negotiation. Specify messages and invariants before shipping implementations independently on different platforms.

Proposed lifecycle: idle → connecting → authenticated → authorized → negotiating → active → reconnecting or stopping → closed. Include explicit rejection/failure states. Authentication verifies identity; authorization checks the target's local permission for the requested action. Perform authorization before exposing desktop content or applying input.

A session negotiates protocol version, monitor geometry, codec/profile, resolution, frame rate, input support, optional audio, and optional clipboard/transfer permissions. Define incompatible-version behavior and stable errors.

Separate traffic by purpose:

- Reliable ordered control messages, keyboard transitions, and mouse button transitions.
- Coalesced pointer movement so stale positions do not accumulate.
- Independently scheduled video with explicit backpressure and stale-frame recovery.
- Optional audio and bounded file-transfer traffic that cannot starve input/control.

Choose video transport after measuring reliable streams versus datagrams on direct and relayed paths. QUIC datagrams require packet sizing, fragmentation/reassembly limits, loss handling, and decoder recovery. Dropping an arbitrary predicted frame can invalidate later frames; coordinate dropping with codec dependency handling and keyframe requests. Reliable video streams still require bounding queues and recovery from stale data.

Define maximum message and transfer sizes, timeouts, validation, and malformed-peer handling. Use session IDs and connection generations to reject delayed input from an old session. Release all held keys/buttons on disconnect, local stop, permission loss, controller crash, or session takeover. Reauthorization and renegotiation must occur as appropriate after reconnect; do not replay old input.

The implemented input semantics are physical USB HID key usages, press and release for the left, right, and middle buttons, absolute pixels on one monitor, and wheel detents (positive vertical scrolls down). Coordinate mapping for scaling, DPI, rotation, and extra monitors is still open. Finished text is a separate control message of at most 128 bytes. It does not depend on key position, and it is not a clipboard paste.

## 8. Trust and security

Persist an installation identity using the platform credential store: Keychain on macOS, an appropriate Windows-protected store, and Linux Secret Service where available. Handle unavailable/locked stores explicitly. Reinstallation or identity reset should require fresh trust unless an intentional secure recovery design exists.

The shared library defines that record. `Core::open` restores it, and pairing, grants, revocation, and relay changes write it back. The bytes are opaque. They include the installation secret and any relay credential, and they must stay out of logs. Native apps write them to the credential store named above. The library returns a locked, unavailable, or invalid-record error and leaves the previous bytes untouched. Resetting the identity stores a new secret with an empty device list and stops the running endpoint. There is no recovery of the previous identity or its grants. Tests use an in-memory store and do not touch Keychain, DPAPI, or Secret Service.

Pair through an authenticated invitation, verified fingerprint, or a properly designed authenticated pairing exchange. A public endpoint ID is not a secret; a short unauthenticated code is not sufficient proof of identity. Invitations must expire, resist replay, and be excluded from logs. The library can carry one invitation in a `.vnl` file. The person who opens the file types the three digits shown on the computer that wrote it. The computer that will be controlled types the controller's digits when the connection arrives. A file by itself does not grant control.

Trust records are directional: allowing A to control B does not automatically allow B to control A. Support separate control, clipboard, and transfer grants. Re-pairing refreshes addresses without silently revoking previously authorized grants, and approving control adds only the control grant. An installation stores at most 128 paired devices and refuses further pairings with a clear error; there is no eviction. Revocation on a host must stop active sessions and block reconnect. For three devices, remote revocation propagation is useful but must not be claimed until an authenticated mechanism exists; each target's local allowlist is authoritative.

Use Iroh transport encryption and verify peer identity; avoid inventing another encryption scheme. A relay, public or custom, may observe connection metadata even when payloads are encrypted. Do not send private keys to a relay or log screen content, clipboard content, credentials, pairing secrets, or file contents.

Clipboard and file transfer are privileged session features. Clipboard text requires its own grant and is capped at 256 KiB. The library carries that text and does not read the operating-system clipboard. File transfer requires its own grant and is capped at 8 MiB. `set_transfer_directory` chooses the only directory writes may use. Names that leave that directory are refused, an existing file stays unless the sender passes replace, and a symlink inside the directory is not a place to write.

## 9. Platform work and support boundaries

### macOS

`vnilink-macos` encodes and decodes H.264 with VideoToolbox and builds a ScreenCaptureKit configuration for one BGRA monitor, cursor hidden. `ScreenCapture` starts the stream and copies BGRA pictures through an output handler; `HostSession` paces them at the session quality and publishes Annex-B for that hosted session, and `serve_stream` reads a capture that is already open and stops when the session ends. Tests use a synthetic picture. They do not call `SCShareableContent::get` or start a stream, and they have not captured the screen. SwiftUI, permission onboarding, and drawing the decoded picture are still open.

Test locked sessions, user switching, monitor disconnect, and headless capture on the actual Mac mini. FileVault preboot unlock is outside the initial logged-in-desktop milestone; do not promise that an ordinary app or launch agent can control it.

### Windows

`vnilink-windows` targets one monitor on the interactive desktop of the current session. Capture is DXGI Desktop Duplication. `WindowsCapture` polls the duplication output without waiting; the host adapter blocks on newer frames through the shared frame-source seam. `HostSession` paces those pictures at the session quality and publishes Annex-B, and `serve_windows` reads a capture that is already open and stops when the session ends. Encode is the Media Foundation H.264 transform, fed NV12. Its length-prefixed samples are converted to Annex-B, and an IDR is prefixed with the sequence header's SPS and PPS. That conversion is tested here. Decoding reads the picture size from the SPS and decodes Annex-B through the inbox transform to NV12, converted back to BGRA; a keyframe without SPS, PPS, and an IDR is refused before Media Foundation runs. The DXGI and Media Foundation calls are compiled only on Windows, so a macOS test run does not execute them. Windows Graphics Capture, with its consent picker, is not this path.

Ordinary SendInput is restricted by integrity levels. Elevated applications, UAC secure desktop, logon screens, session switching, and service/session isolation need separate validated designs. Do not mark them supported because ordinary desktop control works.

### Linux

X11 and the Wayland ScreenCast portal are separate. X11 reads the current session through x11rb. Tests unpack a 32-bit little-endian image and do not connect to a display. Portal phases are tested without D-Bus. The D-Bus driver is compiled on Linux only. Against ashpd 0.13 it asks for one monitor, a hidden cursor, and no persisted grant, then reads the synchronous portal response and returns a PipeWire file descriptor. `PortalCapture` connects that descriptor to the node and copies BGRA or BGRx frames. The production portal requests MemFd/MemPtr buffers without GPU modifiers and rejects unexpected DMA-BUF delivery. A real Wayland run later aborted inside Mesa during GBM unmapping, so that import path is retained only as an isolated hardware test. Tests do not open a portal session. `HostSession` paces captured frames at the session quality and publishes Annex-B for that hosted session. Its test uses a synthetic picture. `serve_portal` reads a capture that is already open and stops when the session ends. Neither one draws a window. One run on this Ubuntu 24.04 GNOME Wayland session shared a 1920×1080 monitor as BGRx in an Intel Y-tiled DMA-BUF. GBM mapped that modifier, and VA-API encoded the frame as a keyframe with SPS, PPS, and an IDR. That single-frame result did not validate repeated buffer cleanup; the later GBM-unmap abort supersedes it for production use. That grant is the one session the dialog allowed. The DMA-BUF test opens a render node when one is present. H.264 encode is VA-API through libva, fed NV12: High profile with CABAC when the session negotiates it, else Constrained Baseline. The Linux encoder currently leaves 8x8 transforms off; the decoder accepts them from other encoders. A keyframe carries SPS, PPS, and an IDR. The encoder opens `/dev/dri` render nodes in name order and uses the first one that offers low-power slice encode, or slice encode when the low-power entrypoint is absent. A node that opens without that entrypoint is skipped. A build where no node offers it returns unsupported. H.264 decode is VA-API variable-length decode through the same crate: `LinuxDecoder` opens the session from a keyframe's SPS and PPS, tracks references with a sliding-window picture buffer, downloads NV12, and returns BGRA. Baseline, Main, and High progressive pictures decode; fields, B slices, and long-term references are refused explicitly. This tree does not link x264 or a locally compiled OpenH264.

Document the supported desktop combinations. Do not claim universal Linux unattended access from a successful portal session on one environment. Wayland control now uses `WaylandCapture` and its `HostDesktop` adapter through a combined RemoteDesktop/ScreenCast portal grant. Physical keys, character text, pointer, buttons and scroll are wired through D-Bus Notify methods. Real compositor input remains unverified; a private D-Bus fixture verifies the request flow, input calls and grant cleanup.

### Sleeping and powered-off devices

Connectivity libraries cannot receive connections while the target agent is not running. Awake, sleeping, powered off, locked, and preboot are distinct states. Keeping a host awake is an operational option; add Wake-on-LAN or another wake mechanism only after validating network and hardware support. A relay cannot automatically wake a machine behind a router.

## 10. Lightweight operation and measurement

Capture/encode only when a session requires it. Avoid busy polling. Use bounded queues, frame pacing, idle reduction, adaptive bitrate/resolution, and separately transmitted cursor updates. Prefer hardware codecs with explicit fallback and surface failures clearly.

Measure idle CPU/RAM, host capture/encode cost, controller decode/render cost, startup time, sustained bandwidth, reconnect time, and input-to-display latency. Record hardware, OS, resolution, frame rate, codec, network RTT, loss, direct/relay path, and measurement method with results.

A proposed initial operating profile is 1080p at up to 30 fps with adjustable bitrate, plus idle reduction. This is a baseline to test, not a guaranteed capability or final owner requirement. Derive numerical performance acceptance targets from measurements and owner expectations before asserting the app is lightweight.

## 11. Implementation sequence and completion gates

1. **Platform feasibility:** establish capture, authorized input, codec availability, and permission persistence on all actual devices. Deliver a supported-state matrix with evidence for lock/reboot/headless behavior and blockers.
2. **Core and binding contract:** establish library lifecycle, capability/event API, trust persistence, session states, bindings, and versioned protocol. Complete when native hosts can start/stop the core and receive ordered bounded state updates safely.
3. **Networking and pairing:** authenticated pairing, target-local grants, discovery, direct and forced-relay connections, rejection/revocation. Complete when all six directed connections work across distinct networks on the public-relay default, a custom relay URL works without rebuilding, and unauthorized peers are rejected.
4. **First usable remote desktop:** one monitor, negotiated encoded video, keyboard/mouse, local disconnect, indicator, input cleanup. Complete when all six directions perform real interactive desktop tasks; mock adapters alone do not satisfy this gate.
5. **Unattended reliability:** platform startup, UI-independent hosting, network changes, crash/restart recovery, sleep/resume, permission loss, display changes. Complete for the documented supported desktop states with reproducible tests.
6. **Additional features:** clipboard, transfer, audio, multiple monitors, and wake behavior according to owner priority. Each feature needs negotiated permissions and independent completion evidence.
7. **Packaging and operations:** signed platform packages where applicable, permission/setup guides, pinned bindings/dependencies, repeatable platform builds, an optional custom-relay runbook, compatibility documentation, and measured resource results. The default install uses the public relays and ships without a relay deployment.

Keep implemented, tested, proposed, unsupported, and blocked items distinct in documentation. A compiling shell or passing model test is not a finished remote desktop app.

## 12. Verification matrix

Test Mac→Windows, Mac→Linux, Windows→Mac, Windows→Linux, Linux→Mac, and Linux→Windows. For each, record actual device/OS versions and direct versus forced-relay outcomes. Run the internet cases on the public-relay default, and again with a custom relay configured on the installations under test.

Cover same LAN and distinct internet networks; UDP-restricted/relay fallback; address changes; slow links and loss; multiple-hour sessions; resize/DPI/rotation; keyboard layouts; clipboard/transfer permission boundaries when implemented; local stop and revoke; reconnect input cleanup; host/controller crashes; permission denial; locked/reboot/headless states where claimed.

Use deterministic Rust tests for authorization, protocol validation, state transitions, cancellation, limits, and stale-session fencing. Use platform integration tests for native adapters. Use real-machine manual or automated evidence for capture, rendering, input, startup, and operating-system restrictions. Include malformed input and oversized payload tests. Run platform builds on their actual supported toolchains; do not imply a macOS-only build verifies Windows/Linux.

### Cursor implementation and platform matrix

Protocol version 3 negotiates independent cursor delivery only when both the
capture adapter and viewer support it. Older peers negotiate version 2 or 1.
Cursor shapes are bounded lossless BGRA images; position updates reuse the shape.
The cursor stream is independent of video, and reads and publications are fenced
by session identity and generation. Shared tests cover delivery without any video,
visibility changes, unsupported adapters/viewers, and stale generations.

| Host → viewer | Independent cursor implementation | Validation boundary |
| --- | --- | --- |
| Linux Wayland → macOS | Portal/PipeWire metadata → AppKit native cursor | Native builds and shared protocol tests; live Ubuntu GNOME → Mac cursor metadata and view-only overlay verified; responsiveness comparison pending |
| Linux Wayland → Linux GTK | Portal/PipeWire metadata → GDK native cursor | Native Linux build; two-machine GTK viewing not yet verified |
| macOS → macOS or Linux GTK | AppKit system cursor samples → independent cursor stream; embedded ScreenCaptureKit cursor when the public cursor API is unavailable | Native builds and host shape tests passed; live Mac → Ubuntu GTK control verified with an immediate, single correctly shaped cursor. Mac → Mac cursor delivery remains unverified |
| Linux X11 → macOS or Linux GTK | Capture metadata adapter still needed | Existing capture path retained |
| Windows in either direction | Native viewer integration and capture metadata still needed | Windows Rust adapters alone do not establish an end-to-end native app |

Shared transport, pacing and quality changes apply to every caller of that shared
code. PipeWire/VA-API host fixes and AppKit presentation fixes apply to their own
platform and role. A successful Linux-host/Mac-viewer run therefore does not prove
Mac-host/Linux-viewer or Windows behavior. Each direction needs native codec,
capture, permission, input, reconnect and display-scaling validation.

## 13. References and maintenance

Consult current primary documentation when implementing version-dependent behavior:

- [Iroh Rust API](https://docs.rs/iroh/latest/iroh/)
- [Iroh `RelayMode`](https://docs.rs/iroh/latest/iroh/endpoint/enum.RelayMode.html)
- [Iroh: add a relay](https://docs.iroh.computer/add-a-relay) (public n0 relays by default; self-hosted relays use `RelayMode::Custom`)
- [Apple ScreenCaptureKit](https://developer.apple.com/documentation/screencapturekit)
- [Apple accessibility trust API](https://developer.apple.com/documentation/applicationservices/1459186-axisprocesstrustedwithoptions)
- [Microsoft Desktop Duplication](https://learn.microsoft.com/en-us/windows-hardware/drivers/display/desktop-duplication-api)
- [Microsoft SendInput restrictions](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput)
- [XDG RemoteDesktop portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html)

Pin selected dependencies and document the actual supported versions. Replace proposed build layout and instructions with verified commands as implementation lands. Put detailed decisions and evidence in the corresponding docs directories, and keep this README as the entry point describing the final product and remaining scope.

The native Linux and macOS apps now provide background window-close behavior,
tray/menu-bar Open, Settings, Stop remote access and Quit actions, and opt-in
launch at login. Startup sharing is opt-in: macOS requires existing Screen
Recording/Accessibility permissions; Linux can resume X11 sharing, while Wayland
still requires its portal selection. See [Linux background setup](linux/README.md#background-and-login-startup)
and [macOS implementation](apple/README.md). Actual login cycles and unattended
real-machine desktop operation remain validation work.
