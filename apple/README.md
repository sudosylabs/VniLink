# VniLink for macOS

A native SwiftUI application for macOS 15 or later. Rust owns identity,
pairing, permissions and sessions. Swift owns windows, file panels and Keychain.
VniDrop's Apple code is the implementation reference: `App/RootView.swift`
for the source-list navigation, `Features/SavedDevices/SavedDevicesScreen.swift`
for the native device list, `SavedDeviceDetailsView.swift` for inspector
presentation and sizing, `SavedDevicePresentation.swift` for the device avatar,
and `Features/Settings/SettingsScreen.swift` for native settings forms.
These patterns are adapted to VniLink's desktop and directional-grant APIs.

## Development

Requires Xcode, XcodeGen and the repository's Rust 1.91 toolchain.

```sh
# From the repository root, build bindings for this Mac's architecture:
apple/scripts/build-core.sh
(cd apple && xcodegen generate)
xcodebuild -project apple/VniLink.xcodeproj -scheme VniLink \
  -destination 'platform=macOS' -derivedDataPath apple/.build build
open apple/.build/Build/Products/Debug/VniLink.app
```

The generated Swift bindings, XCFramework, Xcode project and build products
are ignored. Regenerate the Rust package after changing the FFI. This is an
ad-hoc signed development app, not a notarized distribution. No iOS target or
universal/Intel distribution is claimed.

## Implemented

- A native left sidebar: Devices, This Mac, Connection, Keyboard, Files and
  Pairing. Devices uses a saved-device list; selecting a row opens a native
  right-hand inspector for its name, grants, Connect, revoke and forget actions.
  Command-comma opens This Mac in the same main window.
- One installation record in the login Keychain. Locked/unavailable storage
  produces an error; there is no plaintext fallback.
- Pairing-file creation/import (including Finder opening), three-digit
  verification, invitation tokens and a paired-computer list.
- Local computer names, directional control/clipboard/file permissions,
  revocation and forgetting. Saved relay URLs and optional masked tokens
  apply on the next open. Stored tokens are never displayed.
- A separate resizable/full-screen desktop window. VideoToolbox decoding runs
  in Rust; a bounded latest-picture slot delivers BGRA to an AppKit canvas.
  Keyboard, mouse, scroll, view-only mode, picture-quality choices, explicit
  clipboard sending and an 8 MiB file-send panel use the session's APIs.
  Input queues are bounded; losing focus releases held keys and buttons.
- Explicit main-display ScreenCaptureKit sharing and Accessibility input
  setup in This Mac. Opt-in startup preferences can resume main-display sharing
  and allow connections, and resume input, only with existing OS permissions.
  Startup never requests missing permissions; grants remain authoritative.
- Keyboard mode and receive-folder choices apply to the current launch.
  Identity reset opens the new stored identity. Closing the main window leaves
  the app running when background mode is enabled; Quit stops media and shuts
  down the core off the UI thread.
- A native menu bar item with Open, Settings, active/sharing status, Stop remote
  access and Quit. This Mac includes persistent background/startup preferences.
  Launch at login uses `SMAppService.mainApp`, reads registration status directly,
  and offers Login Items settings when approval is needed. Login launches hide
  the main window when background mode is enabled; ordinary launches reopen it.

## Validation and remaining work

The macOS app builds locally. A synthetic H.264 picture passes through a paired
loopback session, VideoToolbox decoding and the FFI frame poller; the test also
checks decoder subscription release. Capture idle-frame reuse has a regression
check. Tests do not request Screen Recording or inject system input.

The Mac encoder explicitly requests H.264 Constrained Baseline for compatibility
with the Linux decoder. A macOS test feeds real VideoToolbox output at 64×64 and
1920×1080 through Linux's SPS, PPS and IDR/predictive slice parsers. This guards
against the default encoder enabling the unsupported High-profile 8×8 transform;
it does not replace a real Linux GPU decode or two-machine session test.

Real-machine capture, input, reconnect and cross-platform desktop operation
still need validation with OS permissions granted. Clipboard receiving needs
its native pasteboard adapter; the send action uses the remote host's adapter
and permission. Automatic clipboard synchronization, IME composition,
and distribution packaging remain unfinished. Login registration across an actual
logout/login cycle and permission-granted sharing resume still need validation;
registration is not enabled automatically by development builds.
A paired-device grant alone does not prove successful desktop operation.

For visual inspection, run the app executable with `--preview`. It creates
process-local fixture computers, uses a loopback relay URL, and never opens
or modifies Keychain. Pairing values in preview are fixtures, not user devices.

Debug builds also offer **Development → Preview devices**, which swaps to
process-local fixture identities while preserving the stored installation.
The preview indicator remains visible after actions. Do not use preview fixtures
to assess network connectivity or live desktop operation.

On this Xcode setup, Rust media tests
need the system Swift runtime search path:

```sh
DYLD_LIBRARY_PATH=/usr/lib/swift MACOSX_DEPLOYMENT_TARGET=15.0 \
  cargo test --locked -p vnilink-ffi -p vnilink-macos -p vnilink-media
```
