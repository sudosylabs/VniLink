# VniLink

VniLink is a remote desktop application with a shared Electron interface written in vanilla TypeScript, HTML and CSS, and a shared Rust library with native macOS, Windows and Linux engines.

The Electron application lives at the repository root. Its sandboxed renderer calls a typed preload API; an Electron utility process hosts the Rust Node addon. Installation state and pairing commands are connected, while the full pairing interface, remaining services and media streaming are pending. The old SwiftUI and GTK application folders and UniFFI generator have been removed; the Rust platform and application adapters remain.

## Packaging preparation

`product.json` is the source of truth for the planned release identity, executable names, invitation association, icon inputs, signing policy and native target matrix. Version remains owned by `package.json`; Rust workspace versions remain in `Cargo.toml`. `npm run check:product` validates the metadata and artwork; it does not package the application or prove OS support.

`appId` is the application identity, `nameShort` the display name **VniLink** and `publisherName` the publisher. The publisher label is not a signing certificate subject or a claim about a registered legal entity. macOS uses executable name `VniLink`; Windows and Linux use `vnilink`. Packaging derives the macOS bundle identifier and Windows application identity from `appId`; Linux uses the corresponding `.desktop` filename. Helper identifiers must derive from this identity when signing is wired.

| Planned platform | Architecture / Rust target | Intended minimum |
| --- | --- | --- |
| macOS | arm64 / `aarch64-apple-darwin` | macOS 14 |
| macOS | x64 / `x86_64-apple-darwin` | macOS 14 |
| Windows | x64 / `x86_64-pc-windows-gnu` | Windows 10 22H2, build 19045 |
| Linux | x64 / `x86_64-unknown-linux-gnu` | Ubuntu 24.04 and Debian 13 are intended validation baselines |

These are planned packaging targets, not verified support claims. macOS 14 follows the native ScreenCaptureKit dependency's enabled APIs; Electron 44 itself requires macOS 13 or later ([Electron release notes](https://www.electronjs.org/blog/electron-44-0)). Linux's current Debian 13 test image does not establish Ubuntu 24.04 binary compatibility: release builds must use an appropriate native-library/sysroot baseline and verify the resulting library dependencies on both distributions. No numeric glibc floor is asserted before that work. PipeWire 1.0 support is required by the current native binding on Wayland. Windows/Linux ARM64, 32-bit builds, universal macOS bundles, store distribution and an update feed are outside the initial matrix.

macOS release signing is planned through Developer ID and notarization using the publisher's Apple Developer account. Initial Windows and Linux packages are unsigned. Unsigned installer generation is available as described below. Signing credentials, invitation file-open handling, platform manifests, tray integration and publishing remain later steps. The running application name and Windows application identity derive from this metadata. OS registration of `.vnl` remains a later step.

The existing Electron user-data location, Rust installation directory and credential service `com.vnilink.storage` / account `installation-key` remain independent storage contracts. Do not rename or migrate them merely to match the packaging identifier. Upgrades must preserve access to the existing encrypted installation. Future installer identities must remain stable across versions and be set before first distribution.

### Release payload build

Run `npm run build:release` on a host in the approved product matrix. This type-checks the sources, builds `vnilink-node` with Cargo's optimized `release` profile for the explicit host target, and compiles production/minified Electron bundles. macOS builds pass the agreed `MACOSX_DEPLOYMENT_TARGET=14.0`; this is a build setting, not proof that every linked dependency works on macOS 14. A conflicting `CARGO_BUILD_TARGET` is rejected. Windows uses the existing GNU/MSYS2 toolchain; Linux uses its native build environment, whose dependency baseline must still be validated against the intended distributions.

The completed payload is published to `.build/release/<platform>-<architecture>/`:

- `app/`: runtime bundles, renderer resources, minimal application `package.json`, product metadata, staged native addon, tray images, notices and relinking sources.
- `packaging-resources/`: the platform icon inputs for the later Electron assembly step.
- `build-manifest.json`: product/version/Electron/native target/profile, an application/build input fingerprint, and relative paths, sizes and SHA-256 hashes for the staged files. The native stamp also records its Rust source fingerprint and explicit build environment.

Release and development share the maintained bundle/resource definitions. Release builds exclude the browser/smoke test entry points, do not copy arbitrary files from `out/`, and leave the development output untouched. Relinking sources intentionally retain Rust source and tests needed to rebuild the native module; generated object files, caches and agent instructions are excluded. No private profiles, keys or mock-device preview files are staged.

Staging uses a private temporary directory and an exclusive per-target build lock. Failed or cancelled work is cleaned up without publishing a partial payload; a previous completed payload is retained if failure occurs before replacement. Source changes during compilation/staging cause failure rather than mixing build inputs. Do not delete an active build lock. An ungracefully terminated runner may require confirming that no build is alive before removing its stale lock.

`npm run verify:release` checks the exact file inventory and opens/shuts down the staged native addon with a temporary engine under Node. It does not use the production installation key or saved devices. It requires native networking permissions for Iroh startup. This establishes staged-addon loading and basic engine lifecycle on the current host; packaged Electron launch, native dependency audits, clean-machine installation, other OS/architecture runtime checks, signing and installer generation remain subsequent steps. These commands do not publish anything or create an Electron application bundle.

### Unpacked application assembly

`npm run package:dir` rebuilds the release payload and assembles an unsigned host-native application in `dist/<platform>-<architecture>/`, using pinned electron-builder 26.15.3 and the installed, pinned Electron runtime. macOS contains `VniLink.app`; Windows/Linux contain `vnilink.exe` / `vnilink`. Application code is archived in ASAR. The utility-process entry and native module remain adjacent under `resources/app.asar.unpacked/` (macOS: `Contents/Resources`). Notices and the native relinking source remain external resources. Signing credential discovery and publishing are disabled for this command.

`npm run verify:package` copies that assembly to a temporary directory outside the checkout, launches its own executable with a temporary engine and isolated profile, checks workbench readiness, engine connectivity and renderer isolation, and saves a screenshot and diagnostic result under `.build/package-tests/`. The explicit `--smoke-test` launch mode requires `VNILINK_TEST_PROFILE`; normal startup uses the persistent installation. The verifier enables localhost-only DevTools on a random port for its own temporary process and removes the temporary application/profile after teardown.

These commands create no installers and perform no Developer ID signing or notarization. They do not establish clean-machine compatibility or validate another architecture/OS. Native Windows/Linux and Intel Mac assembly/runtime checks remain required on their respective hosts. macOS local assembly may require an ad-hoc signature for execution; that is distinct from release signing.

### Unsigned installer generation

`npm run package:installers` builds the host-native release payload, assembles the app and generates its installer formats. `npm run package:installers:staged` only runs the installer stage against an existing completed assembly and release payload. It verifies the complete application inventory first, including framework symlinks, executable flags, Electron resources, native modules and relinking material. Old assemblies without that inventory need rebuilding with `npm run package:dir`.

Outputs live in `dist/installers/<platform>-<architecture>/` with version/OS/architecture in each filename, `SHA256SUMS`, and an `installer-manifest.json` recording checksums and the exact assembled input. Concurrent assembly/installer jobs for the same target are rejected by their shared lock. Failed/cancelled generation does not publish partial installers or remove a previous completed output. Downloads of pinned builder helper tools use electron-builder's cache.

| Host | Configured output | Installation behavior |
| --- | --- | --- |
| macOS | `.dmg` | Drag `VniLink.app` to Applications; the volume contains an Applications link. |
| Windows | NSIS `.exe` | Current-user installation, Start menu shortcut, no desktop shortcut, no automatic app launch; uninstall preserves app data. |
| Linux | `.deb`, `.rpm` | Native package-manager installation with a desktop entry and icon. |

`npm run verify:installers` verifies the artifact hashes. On macOS it additionally verifies the disk image, mounts it read-only, compares the embedded app against its complete input inventory, then launches an isolated copy through the packaged-app smoke driver. It detaches the image and removes temporary state afterwards. It does not install into `/Applications`, touch the real installation store, or assess Gatekeeper/notarization.

Windows/Linux checksums alone do not establish installation, upgrade, uninstall or runtime behavior. Those checks remain native-host gates. Linux currently uses electron-builder's default Electron dependency lists: these must be audited/extended against the linked Rust addon and tested on the supported distribution baselines before distribution. RPM generation does not imply support for a particular RPM-based distribution. Windows minimum-version enforcement and OS invitation associations remain separate integration work.

All outputs in this step are unsigned development artifacts. macOS Developer ID signing, hardened runtime entitlements, nested native-module signing, notarization and stapling are the next stage. No update feed, upload or publishing is configured. The app's own source has no distribution license declared at the repository root; staged metadata uses `UNLICENSED`, without changing the licenses or notices of bundled third-party components.

The configuration follows the pinned builder's [prepackaged CLI](https://www.electron.build/v26/docs/cli/), [DMG](https://www.electron.build/v26/docs/dmg/), [NSIS](https://www.electron.build/v26/docs/nsis/) and [Linux](https://www.electron.build/v26/docs/linux/) targets. Like VS Code's platform packaging tasks, compilation/assembly and installer generation are separate stages; VniLink uses these builder targets rather than copying VS Code's Inno Setup and bespoke Linux packaging toolchain.

## Development

Use Node 22.19 or later, npm and the pinned Rust toolchain:

```sh
npm ci
npm start
```

`npm run build` type-checks the application, builds the Rust Node addon and compiles the Electron entry points. Native Linux builds require the DBus development libraries for Secret Service. Where AppArmor restricts unprivileged user namespaces, as on Ubuntu 23.10 and later, Chromium's sandbox needs a profile allowing them for the development Electron; every task that launches it says so, and `node build/run.ts sandbox:linux` installs that profile once for the checkout (it asks for sudo). `npm test` launches a hidden Electron window with a disposable profile and temporary Rust identity, checks rendering, the preload boundary, native state/invitations, invalid actions and restricted local assets. It uses a temporary engine, so it does not open the production installation database or its key. Normal startup opens that encrypted database and reads the 32-byte key from the platform credential store; failure does not fall back to a temporary identity. After an initial `npm run build`, `npm run dev` launches the development shell with incremental TypeScript/CSS builds, resource watchers and background type checking. Save changes, wait for the watcher to finish, then press Cmd+R on macOS or Ctrl+R on Windows/Linux to reload the window. `npm run watch` runs the same watchers without launching Electron. Main/preload/utility-process changes require restarting the development app; Rust is not watched, so quit and run `npm run build` after Rust changes. Reloading resets renderer state and retires its data channels; it keeps the Rust utility process running. Generated application code lives in `out/`; `dist/` is reserved for distribution packages.

The build uses named native compilation, output preparation, TypeScript bundling and resource/addon staging tasks. Development and launch reject a missing or stale native addon using its source, target/profile and binary identity. Rust changes require a rebuild; renderer reload preserves the engine.

Run independently selectable desktop scenarios after building with `npm run test:ui -- --list` and `npm run test:ui -- --scenario appearance` (repeat `--scenario` for more areas). Test profiles are removed after Electron exits; logs, environment details and screenshots are retained under `.build/test-runs`. `npm run test:linux` runs the checks, Rust tests and headless Electron scenarios in the maintained Debian Trixie container. It requires Docker and isolates the read-only checkout from Linux dependencies/build caches; this environment supplies PipeWire headers compatible with the pinned binding. On native Windows x64, use `scripts/test-windows.ps1` after installing Node, Rust and MSYS2 MinGW GCC/Clang/NASM/Make plus CMake. These are also the Linux and Windows CI entry points. Headless/container checks do not establish physical capture, input, privacy or peer interoperability.

`npm run format` applies the agreed formatting; `npm run lint:fix` fixes supported lint findings. `npm run check` verifies product metadata, that tracked files name no contributor's home directory, handle or stray product identity, formatting, code/CSS lint, types and tooling regression tests. See [contributor guidance](CONTRIBUTING.md#typescript-css-and-formatting) for rules and layer boundaries.

Electron main owns windows, lifecycle, the local protocol and validated IPC. Preload exposes named operations rather than Node/IPC APIs. The browser workbench owns the interface. Common contracts contain environment-independent types; base browser controls are reusable across features. Only implemented modules have directories. No UI framework is required; esbuild is a build tool.

Continuous data uses a separate engine channel. `DataChannelClient.open` requests a session-scoped port through preload; main validates the requesting window and transfers the endpoints to the renderer and utility process. Payload messages then travel directly between them. The native `DataChannel` handles operate outside the JSON command bridge's mutex. Video and terminal output carry binary bytes, with one unacknowledged output packet per channel. The utility process currently checks native queues every 4ms while idle; the renderer acknowledges after consuming the packet. Input has independent delivery and acknowledgements; adjacent absolute pointer positions coalesce without discarding key/button transitions. File channels operate on Rust-owned service jobs and return progress snapshots; a Rust background worker advances transfers independently of UI polling. Cancellation has a separate bounded request slot, so an outstanding progress request cannot block it. Blob transfer and disk I/O stay in Rust.

Use the active session ID and generation when opening a channel; `beginConnectServices` establishes a neutral connection under existing peer admission rules. Video and input channels share an authorized Desktop service for that connection generation, including neutral connections. Closing either channel preserves its sibling; closing the last one retires the Desktop owner. Closing a client, navigating, or destroying the window retires its channels. Terminal services still require the existing host backend and explicit Rust permission; the transport does not enable terminals or grant access. File services still require approved roots and blob storage. The channel metrics expose acknowledgements and local consumer latency. GPU texture import, video decoding/presentation, and terminal/file views remain separate work; this change provides their transport, not a measured end-to-end latency result. The old JSON video commands remain available for compatibility, but new consumers should use binary channels.

See [Contributing](CONTRIBUTING.md), [Security](SECURITY.md) and the [Code of Conduct](CODE_OF_CONDUCT.md). Pairing, grants, reconnect and Files ownership remain Rust contracts; replacing the UI does not replace those engines. The product requirements below describe the intended complete application, not claims that the Electron shell already implements them.

## 1. Project goals and decision status

### Goals

- Name: **VniLink**.
- Any paired computer can control any other, across every supported operating system and in both directions.
- Access must work across the internet, including different networks, rather than only on a home LAN.
- The application should be lightweight.
- The shared Rust part must be a **library**. A shared Electron desktop application will host it through a defined Rust bridge.
- Use root-level TypeScript application sources with shared Rust crates, following a smaller version of VS Code's layer boundaries.
- Build a distinct product rather than shipping Tailscale or RustDesk as the application.
- Each installation uses its own configuration, so other people can install VniLink and point it at their own devices and relays.
- The default relay is the public n0 relay set from the pinned Iroh release. In iroh 1.3.0 that is `RelayMode::Default`, also applied by `presets::N0`. Installing, pairing, and connecting works with those public relays alone.
- An installation may replace that default with one or more custom relay URLs. In iroh 1.3.0 that is `RelayMode::Custom`. The override is local configuration.

### Supported platforms

| Platform | Status |
| --- | --- |
| macOS on Apple silicon (arm64) | Supported |
| Windows x64, built with the GNU toolchain (`x86_64-pc-windows-gnu`) | Supported |
| Linux x64 on Ubuntu and Fedora | Supported |
| Linux arm64; Linux desktops other than GNOME | Unverified |
| macOS on Intel (x64) | Unsupported; the packaging target above is not validated |

Supported means the project develops and tests there; §12 lists the evidence each still needs before a feature counts as complete.

### Proposed defaults for implementation

These are recommended starting choices, not additional project goals:

- Iroh for authenticated encrypted transport, NAT traversal, and relay fallback.
- Vanilla TypeScript, HTML and CSS for the shared Electron UI; native Rust adapters for operating-system integration.
- Explicit device pairing and per-target authorization for unattended control.
- One active controlling session per target initially; a device may host a session while controlling another device if resource limits permit.
- One monitor and keyboard/mouse control for the first end-to-end milestone.
- H.264 as the initial video codec candidate, with hardware acceleration preferred and a tested fallback.
- A device directory only if the discovery design requires one. Relay choice is the confirmed default above.

### Resolve before claiming complete support

Record decisions and platform evidence for:

1. Linux distribution, desktop environment, compositor, and Wayland versus X11.
2. OS versions and CPU/GPU architectures within each supported platform.
3. Whether required access includes locked screens, pre-login access after reboot, elevated Windows applications/UAC, and a Mac with no monitor attached.
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

```text
vnilink/
  package.json                 # Electron entry and npm commands
  package-lock.json
  tsconfig.json
  Cargo.toml                   # Rust workspace
  Cargo.lock
  rust-toolchain.toml
  src/
    main.ts                    # Electron startup
    preload.ts                 # Sandboxed IPC globals (VS Code's preload shape)
    renderer.ts                # Browser startup
    code/electron-main/        # Main-process composition and IPC channels
    base/browser/ui/           # Reusable DOM controls
    base/parts/ipc/            # VS Code IPC transport (Electron and MessagePort)
    platform/*/electron-main/  # Main services and their window-facing channels
    platform/engine/common/    # Control commands and value validation
    platform/engine/electron-main/ # Engine ownership and request queue
    platform/engine/electron-utility/ # Rust addon host
    workbench/browser/         # Workbench composition, parts and layout
    workbench/contrib/         # Feature pages and their local controls/styles
    workbench/services/        # Injected feature/application service contracts
  crates/                      # Shared core and native engines
  assets/                      # Existing branding
  vendor/                      # Patched dependencies
  build/                       # Build, packaging and test tasks (node build/run.ts <task>)
  scripts/                     # Windows validation entry point (test-windows.ps1)
  test/automation/             # Application/window drivers and scenario selection
  test/browser/                # Independent renderer scenarios
  test/smoke/                  # Actual Electron application checks
  test/native/                 # Two-engine Rust binding integration checks
  out/                         # Generated application code; ignored
  dist/                        # Future distribution output; ignored
  docs/                        # Local specifications/evidence; ignored
```

Engine bridge hosting and application services occupy the platform layers; feature pages occupy workbench contributions. Library tests remain in their owning Rust crates. The old `apple/` and `linux/` GUI applications are no longer workspace components.

## 4. Component ownership

### Shared Rust library

Own device identities, trusted-device records, authorization policy, protocol negotiation, Iroh endpoints, session state, reconnect logic, stream scheduling, bounded queues, transfer state, and structured errors/events.

The crate currently implements identity, relay selection, invitations, grants, session admission, one incoming and one outgoing control session, a bounded event stream, keyboard and pointer input, and encoded H.264 frames over datagrams with a reliable-stream fallback. A full video queue drops predicted frames until the next keyframe. The platform crates capture and encode those frames; `vnilink-media` owns the shared hosting policy behind narrow seams — frame pacing (`FramePump`, `HostVideo`), the hosted session (`HostSession`, with the platform encoder behind `VideoEncoder` and the capture behind `FrameSource`), latest-frame delivery (`FrameChannel`), and the encoder settings derived from the session quality (`RateController`). Clipboard and transfer each run one side stream beside control and video and share one establishment handshake. When the host provides an `InstallationStore`, the crate saves the installation secret, directional trust, grants, used invitations, and relay settings. A dropped session reconnects with a new generation. Clipboard text is a separate grant, limited to 256 KiB. File transfer is a separate grant, limited to 8 MiB, and writes only under a directory the installation chooses.

Input events use USB HID keyboard usages for keys, separate press and release for buttons, absolute pixels on the one host monitor for the pointer, and wheel detents for scroll. `Core::enable_os_input` injects those events with the operating system's input API (Core Graphics on macOS, SendInput on Windows, X11 on Linux) through the MIT-licensed `enigo` crate. The injected value is a physical keycode for that system, not the US character for the HID usage. `submit_local_key` is what a controller calls for a key. The default mode inserts the character the controller operating system produced. Map mode sends the physical key. Translate mode asks the host to press the chord in its own layout; a Linux host reads the X11 keymap, and Windows and macOS hosts refuse that mode. `set_swap_control_and_command` exchanges Control and Command on the controller before a physical key is sent. `submit_text` types a finished string of at most 128 bytes and is not a clipboard paste. VniLink does not use RustDesk's UI, screen-mirroring pipeline, or source code. The host must grant input permission. macOS shows the Accessibility prompt. Wayland uses a separate `WaylandCapture`/`HostDesktop` portal adapter rather than `enable_os_input`; it requires a compositor RemoteDesktop grant. Elevated Windows desktops and the secure login screen are not claimed. Without `enable_os_input`, events are refused and the session stays open. The shared library carries encoded frames and does not capture or draw. `vnilink-macos` can encode and decode a synthetic picture. It has not captured this screen, the Linux window now paints decoded BGRA pictures, but end-to-end desktop operation remains unverified.

Expose semantic operations such as start hosting, pair, authorize device, connect, disconnect, revoke device, unpair device, update session quality, and query capabilities. Native clients observe state/events rather than reconstructing session rules.

Give the runtime a defined owner. Specify startup/shutdown, callback threading, cancellation, event ordering, and how the Electron renderer receives bounded updates. Slow subscribers must not block networking or allocate unbounded event queues.

### Electron application and native adapters

The TypeScript workbench owns tabs, devices, settings and viewer controls. Electron main owns windows and validated desktop services. The Rust bridge preserves owner identity, cancellation and bounded events. Platform crates continue to own capture, codecs, device/session APIs and other native behavior; credentials and privileged OS operations do not move into browser code.

### Background hosting

The shared library remains a library. A platform executable, user agent, or service loads it for background operation. Privileged helpers are separate processes only where needed; a privileged process should expose a narrow authenticated local interface rather than parsing all network traffic with broad privileges.

Service installation does not automatically provide access to a user's graphical session. Design and validate the service-to-session relationship separately on each OS. Protect local IPC against unauthorized local clients and validate message sizes and caller identity.

## 5. Rust bindings and media boundary

`crates/vnilink-app` preserves the tested application adapters as plain Rust. `crates/vnilink-node` exposes a bounded Node-API control interface, hosted by an Electron utility process. UniFFI and its generator have been removed. Typed commands cover identity/pairing, connections, grants and remote-service settings; bridge-owned binary channels carry stream and transfer data. Individual product flows and native desktop behavior still require their own validation. The installation record is one authenticated row in a private SQLite database. The platform credential store (Keychain on macOS, Credential Manager on Windows, Secret Service on Linux) holds only the 32-byte key, as `com.vnilink.storage` / `installation-key`. Media streaming and presentation remain deferred.

Document ownership, lifetime, threading, error mapping, and cancellation for each boundary. Do not expose internal async types or networking implementation details as the native API.

Use a dedicated media interface rather than serializing raw full-resolution video frames through ordinary high-level bindings. Keep capture, hardware encoding, decoding, and GPU rendering close to the platform APIs. Send encoded buffers into/out of the transport with explicit ownership and bounded lifetimes; evaluate shared/native surfaces where feasible, but verify rather than promise zero-copy operation.

The host captures and encodes; the controller decodes and renders. A relay forwards ciphertext and never transcodes video.

## 6. Connectivity and relays

Use Iroh endpoints on each device. Prefer direct connections; retain encrypted relay fallback when NAT or firewall conditions prevent a direct path. No router port forwarding should be required for ordinary device operation. Validate restrictive networks instead of claiming universal reachability.

**Default.** Use the public relay set shipped with the pinned Iroh release. In iroh 1.3.0 that is `RelayMode::Default` (production relays from n0, also applied by `presets::N0`). A new installation needs no relay URL, account, or API key. n0 documents these relays as rate-limited and recommends them for development and testing rather than production workloads. That limit is why a custom relay exists. VniLink still defaults to the public relays so a person can install and connect without operating a server. Say so in setup notes. Treat the public relays as a shared, rate-limited fallback, not as an unlimited production service.

**Custom.** Each installation can replace the default with one or more relay URLs. The library passes that list as `RelayMode::Custom` for the pinned release. An empty list is invalid and must be rejected with a clear error. URLs come from installation configuration and are read at startup. A self-hosted `iroh-relay`, or another relay compatible with the pinned release, is a valid override. If a chosen relay requires a credential, store it in the installation record and keep it out of logs. Public-relay mode stores no relay credential.

Each installation keeps its own relay setting. Pairing and later address refresh include the current home relay URL so the peer can dial it. Dialing uses that URL from the peer address. An open custom relay can be reached by a peer that remains on the public default, when the URL is reachable. A relay that requires a credential is different: Iroh attaches that credential from the dialer's own relay map. Installations that should fall back through a credentialed relay must each be configured with that relay and its credential. Recheck this against the pinned release. Knowing a relay or an address never authorizes a session.

The library pins iroh 1.3.0. Public mode keeps n0 address lookup. Custom mode clears it, so endpoint identity alone does not make a custom-relay peer dialable. The signed invitation carries that peer's current relay URLs and direct addresses. After a session is accepted, each computer sends its current home relay URLs and direct addresses on the authenticated control stream. The receiver stores that list for that peer only. The announcement has no relay credential and does not change grants. A later dial, including a reconnect after a dropped link, uses the stored list. The new session has a new generation and does not send input queued for the old one. `crates/vnilink/RECONNECT.md` is the step list.

Choose either explicit paired-device address publication or a small authenticated directory. Add a directory only when the discovery design requires one. Keep directory trust separate from permission to control.

Running a relay is optional. The default install uses the public relays. Operators who want a private relay configure domain and TLS, required ports, health checks, service startup, logs, updates, and abuse protection themselves. Relay access controls and resource limits must match the pinned release; avoid claiming a configuration option exists without checking it. Deployments, paid services, provider changes, and opening infrastructure access require their own task authorization. This README is not permission to provision or deploy.

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

Persist an installation identity in the encrypted installation database. The platform credential store holds only that database's 32-byte key: Keychain on macOS, Credential Manager on Windows, and Secret Service on Linux. Handle a locked or unavailable key explicitly. A missing key with an existing database, or a key with no database, is not a fresh installation. Reinstallation or identity reset requires fresh trust.

The shared library defines the record. `Core::open` restores it, and pairing, grants, revocation, and relay changes write it back before those changes take effect. The bytes are opaque. They include the installation secret and any relay credential, and they must stay out of logs, as must the key and the encrypted row. The native adapter encrypts the record and stores it in the per-user installation database. The library returns a locked, unavailable, or invalid-record error and leaves the previous bytes untouched. An uncertain commit fences the running engine until restart. Resetting the identity stores a new secret with an empty device list, keeps the saved relay settings and the encryption key, and stops the running endpoint. The secret bound in that stopped process stays the key it bound. There is no recovery of the previous identity or its grants. A developer reset, omitted from normal application builds, is `cargo run -p vnilink-node --features reset-installation --bin vnilink-reset-installation -- --confirm-forget-installation`. It preserves the database files before deleting the key. Core tests use an in-memory store. Adapter tests use a fixture directory and do not touch the real Keychain, Credential Manager, or Secret Service entry.

Pair through an authenticated invitation, verified fingerprint, or a properly designed authenticated pairing exchange. A public endpoint ID is not a secret; a short unauthenticated code is not sufficient proof of identity. Invitations must expire, resist replay, and be excluded from logs. The library can carry one invitation in a `.vnl` file. The person who opens the file types the three digits shown on the computer that wrote it. The computer that will be controlled types the controller's digits when the connection arrives. A file by itself does not grant control.

Trust records are directional: allowing A to control B does not automatically allow B to control A. Support separate control, clipboard, and transfer grants. Re-pairing refreshes addresses without silently revoking previously authorized grants, and approving control adds only the control grant. An installation stores at most 128 paired devices and refuses further pairings with a clear error; there is no eviction. Revocation on a host must stop active sessions and block reconnect. For three devices, remote revocation propagation is useful but must not be claimed until an authenticated mechanism exists; each target's local allowlist is authoritative.

Use Iroh transport encryption and verify peer identity; avoid inventing another encryption scheme. A relay, public or custom, may observe connection metadata even when payloads are encrypted. Do not send private keys to a relay or log screen content, clipboard content, credentials, pairing secrets, or file contents.

Clipboard and file transfer are privileged session features. Clipboard text requires its own grant and is capped at 256 KiB. The library carries that text and does not read the operating-system clipboard. File transfer requires its own grant and is capped at 8 MiB. `set_transfer_directory` chooses the only directory writes may use. Names that leave that directory are refused, an existing file stays unless the sender passes replace, and a symlink inside the directory is not a place to write.

## 9. Platform work and support boundaries

### macOS

`vnilink-macos` encodes and decodes H.264 with VideoToolbox and builds a ScreenCaptureKit configuration for one BGRA monitor, cursor hidden. `ScreenCapture` starts the stream and copies BGRA pictures through an output handler; `HostSession` paces them at the session quality and publishes Annex-B for that hosted session, and `serve_stream` reads a capture that is already open and stops when the session ends. Tests use a synthetic picture. They do not call `SCShareableContent::get` or start a stream, and they have not captured the screen. Electron permission onboarding and presentation of decoded pictures remain open.

Test locked sessions, user switching, monitor disconnect, and headless capture on a real Mac with no monitor attached. FileVault preboot unlock is outside the initial logged-in-desktop milestone; do not promise that an ordinary app or launch agent can control it.

### Windows

`vnilink-windows` targets one monitor on the interactive desktop of the current session. Capture is DXGI Desktop Duplication. `WindowsCapture` polls the duplication output without waiting; the host adapter blocks on newer frames through the shared frame-source seam. `HostSession` paces those pictures at the session quality and publishes Annex-B, and `serve_windows` reads a capture that is already open and stops when the session ends. Encode is the Media Foundation H.264 transform, fed NV12. Its length-prefixed samples are converted to Annex-B, and an IDR is prefixed with the sequence header's SPS and PPS. That conversion is tested here. Decoding reads the picture size from the SPS and decodes Annex-B through the inbox transform to NV12, converted back to BGRA; a keyframe without SPS, PPS, and an IDR is refused before Media Foundation runs. The DXGI and Media Foundation calls are compiled only on Windows, so a macOS test run does not execute them. Windows Graphics Capture, with its consent picker, is not this path.

Ordinary SendInput is restricted by integrity levels. Elevated applications, UAC secure desktop, logon screens, session switching, and service/session isolation need separate validated designs. Do not mark them supported because ordinary desktop control works.

### Linux

X11 and the Wayland ScreenCast portal are separate. X11 reads the current session through x11rb. Tests unpack a 32-bit little-endian image and do not connect to a display. Portal phases are tested without D-Bus. The D-Bus driver is compiled on Linux only. Against ashpd 0.13 it asks for one monitor, a hidden cursor, and no persisted grant, then reads the synchronous portal response and returns a PipeWire file descriptor. `PortalCapture` connects that descriptor to the node and copies BGRA or BGRx frames. The production portal requests MemFd/MemPtr buffers without GPU modifiers and rejects unexpected DMA-BUF delivery. A real Wayland run later aborted inside Mesa during GBM unmapping, so that import path is retained only as an isolated hardware test. Tests do not open a portal session. `HostSession` paces captured frames at the session quality and publishes Annex-B for that hosted session. Its test uses a synthetic picture. `serve_portal` reads a capture that is already open and stops when the session ends. Neither one draws a window. One run on an Ubuntu 24.04 GNOME Wayland session with an Intel GPU shared a 1920×1080 monitor as BGRx in an Intel Y-tiled DMA-BUF. GBM mapped that modifier, and VA-API encoded the frame as a keyframe with SPS, PPS, and an IDR. That single-frame result did not validate repeated buffer cleanup; the later GBM-unmap abort supersedes it for production use. That grant is the one session the dialog allowed. The DMA-BUF test opens a render node when one is present. H.264 encode is VA-API through libva, fed NV12 that the node's video processor converts from the captured BGRA (BT.709, limited range), or that the CPU converts where the node has no video processor: High profile with CABAC when the session negotiates it, else Constrained Baseline. The Linux encoder currently leaves 8x8 transforms off; the decoder accepts them from other encoders. A keyframe carries SPS, PPS, and an IDR. The encoder opens `/dev/dri` render nodes in name order and uses the first one that offers low-power slice encode, or slice encode when the low-power entrypoint is absent. A node that opens without that entrypoint is skipped. A build where no node offers it returns unsupported. H.264 decode is VA-API variable-length decode through the same crate: `LinuxDecoder` opens the session from a keyframe's SPS and PPS, tracks references with a sliding-window picture buffer, downloads NV12, and returns BGRA. Baseline, Main, and High progressive pictures decode; fields, B slices, and long-term references are refused explicitly. This tree does not link x264 or a locally compiled OpenH264.

Document the supported desktop combinations. Do not claim universal Linux unattended access from a successful portal session on one environment. Wayland control now uses `WaylandCapture` and its `HostDesktop` adapter through a combined RemoteDesktop/ScreenCast portal grant. Physical keys, character text, pointer, buttons and scroll are wired through D-Bus Notify methods. Real compositor input remains unverified; a private D-Bus fixture verifies the request flow, input calls and grant cleanup.

### Sleeping and powered-off devices

Connectivity libraries cannot receive connections while the target agent is not running. Awake, sleeping, powered off, locked, and preboot are distinct states. Keeping a host awake is an operational option; add Wake-on-LAN or another wake mechanism only after validating network and hardware support. A relay cannot automatically wake a machine behind a router.

## 10. Lightweight operation and measurement

Capture/encode only when a session requires it. Avoid busy polling. Use bounded queues, frame pacing, idle reduction, adaptive bitrate/resolution, and separately transmitted cursor updates. Prefer hardware codecs with explicit fallback and surface failures clearly.

Measure idle CPU/RAM, host capture/encode cost, controller decode/render cost, startup time, sustained bandwidth, reconnect time, and input-to-display latency. Record hardware, OS, resolution, frame rate, codec, network RTT, loss, direct/relay path, and measurement method with results.

A proposed initial operating profile is 1080p at up to 30 fps with adjustable bitrate, plus idle reduction. This is a baseline to test, not a guaranteed capability or final project requirement. Derive numerical performance acceptance targets from measurements and project goals before asserting the app is lightweight.

## 11. Implementation sequence and completion gates

1. **Platform feasibility:** establish capture, authorized input, codec availability, and permission persistence on each supported platform. Deliver a supported-state matrix with evidence for lock/reboot/headless behavior and blockers.
2. **Core and binding contract:** establish library lifecycle, capability/event API, trust persistence, session states, bindings, and versioned protocol. Complete when native hosts can start/stop the core and receive ordered bounded state updates safely.
3. **Networking and pairing:** authenticated pairing, target-local grants, discovery, direct and forced-relay connections, rejection/revocation. Complete when every directed pair of supported operating systems connects across distinct networks on the public-relay default, a custom relay URL works without rebuilding, and unauthorized peers are rejected.
4. **First usable remote desktop:** one monitor, negotiated encoded video, keyboard/mouse, local disconnect, indicator, input cleanup. Complete when every direction performs real interactive desktop tasks; mock adapters alone do not satisfy this gate.
5. **Unattended reliability:** platform startup, UI-independent hosting, network changes, crash/restart recovery, sleep/resume, permission loss, display changes. Complete for the documented supported desktop states with reproducible tests.
6. **Additional features:** clipboard, transfer, audio, multiple monitors, and wake behavior according to project priority. Each feature needs negotiated permissions and independent completion evidence.
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

The removed native apps previously implemented background window-close, login startup and sharing controls. Those behaviors are not yet integrated into Electron. Startup sharing, permission onboarding and background hosting must be restored through the retained Rust engines and reviewed application services before claiming parity. Earlier native cursor/viewer results above are historical evidence for those implementations, not validation of the new Electron viewer.
