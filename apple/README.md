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

The script embeds an optimized Rust core, and the local `VnilinkCore` Swift
package optimizes generated frame-buffer bindings, even when Xcode builds the
native UI in Debug mode. For Rust debugging, use `VNILINK_CORE_PROFILE=debug
apple/scripts/build-core.sh`; use the default release core for streaming
measurements. Unoptimized pixel conversion can consume the entire frame budget.

To measure the Linux host's CPU conversion without screen permission, run
`cargo run --release --locked -p vnilink-media --example conversion_bench --
2560 1440 60 16.67` on that host. The last argument sets a p95 time budget in
milliseconds and makes the command fail if conversion alone exceeds it.
This is a stage benchmark, not proof of end-to-end performance or RustDesk parity.

The generated Swift bindings, XCFramework, Xcode project and build products
are ignored. Regenerate the Rust package after changing the FFI. This is a
development app, not a notarized distribution. No iOS target or universal/Intel
distribution is claimed.

For reliable Screen Recording and Accessibility permissions across rebuilds,
use a stable installed development certificate. `Signing.xcconfig` optionally
includes the ignored `apple/Local.xcconfig`, following VniDrop's local signing
pattern. Put the certificate's identity and team in that file:

```xcconfig
CODE_SIGN_IDENTITY = YOUR_INSTALLED_CERTIFICATE_SHA1
DEVELOPMENT_TEAM = YOUR_CERTIFICATE_TEAM_ID
CODE_SIGN_STYLE = Manual
CODE_SIGNING_ALLOWED = YES
```

`security find-identity -v -p codesigning` lists installed identities. Without
local configuration, builds fall back to ad-hoc signing, whose designated
requirement is tied to the binary and can invalidate prior privacy grants.
After switching identities, renew the app's existing permissions in System
Settings and fully quit/relaunch once. Keep the same bundle identifier and
signing identity on subsequent builds; pairing remains in Keychain.

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
- A single main window with Home and independent remote-desktop tabs. VideoToolbox decoding runs
  in Rust; a bounded latest-picture slot hands immutable BGRA frame objects to
  an AppKit canvas. The native image provider retains the pixels until drawing
  finishes, without serializing full pictures through the Swift bindings.
  A view display link polls decoded frames at the screen's refresh cadence;
  session checks run separately so they do not delay presentation.
  Keyboard, mouse, scroll and view-only mode use the session's APIs.
  Modifier presses, text keys and focus-loss releases share the character
  planner so shortcuts and held-key cleanup remain consistent. Navigation
  and physical-key repeats are preserved. Clipboard text syncs in both
  directions during authorized sessions; native remote writes are not echoed
  back, and paste submits local text before queuing the shortcut.
  Files can be sent from the viewer panel or dropped onto the desktop, and
  this Mac can send a file back from Files settings during an incoming session.
  Receipts are acknowledged by the receiver; existing files are not replaced.
  Received files default to Downloads/VniLink; a chosen folder is saved.
  The current file limit is 8 MiB and clipboard limit is 256 KiB of UTF-8 text.
  Input queues are bounded; losing focus releases held keys and buttons.
  Picture quality adapts automatically without a preset picker, with up to
  24 Mbps at 60 fps and 4K on a clear path; the host keeps its source
  resolution when it is below the requested size. Enlarging that desktop on a
  larger monitor does not add source detail. Display size offers Fit to window
  and Actual size (1:1 pixels), accounting for Retina backing pixels. Actual
  size scrolls when the source image exceeds the viewer.
  With a compatible Wayland host, cursor shapes arrive independently of video
  and AppKit moves the native cursor immediately. View-only mode draws the host's
  cursor position over the picture. Other hosts keep the existing cursor path.
- Explicit main-display ScreenCaptureKit sharing and Accessibility input
  setup in This Mac. Capture uses the display's backing pixels and can supply
  60 fps; input coordinates remain logical display points. Actual frame rate
  depends on capture, encoding and viewer decoding. A main-thread AppKit sampler
  publishes cursor shapes and positions independently of video to compatible
  viewers. The public system-cursor API is deprecated; when unavailable at
  sharing startup, ScreenCaptureKit keeps the cursor embedded in video.
  Enabling sharing configures the display; ScreenCaptureKit opens for an
  admitted Desktop instance and stops when that instance ends. The Rust host
  supports service-owned capture and a `mac_desktop_viewer` decoder factory;
  Swift controller tabs still use session-based input/video APIs until migration.
  Opt-in startup preferences can resume main-display sharing
  and allow connections, and resume input, only with existing OS permissions.
  Startup never requests missing permissions; grants remain authoritative.
- Keyboard mode, modifier swap and receive-folder choices survive relaunch.
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

The Mac encoder negotiates H.264 High or Constrained Baseline with the peer.
A macOS test feeds real VideoToolbox output in both profiles at 64×64 and
1920×1080 through Linux's SPS, PPS and IDR/predictive slice parsers. Linux now
passes the 8×8 transform flag and parsed scaling matrices through to VA-API.
Synthetic Mac High-profile IDR and predictive frames were decoded on an Ubuntu
Intel GPU and compared with FFmpeg: about 46.5–46.9 dB PSNR against the reference.
This is codec interoperability evidence; the live reverse-direction session
still needs validation after installing the updated Linux decoder.

Real-machine capture, input, reconnect and cross-platform desktop operation
still need validation with OS permissions granted. Input shortcuts, clipboard
delivery and bidirectional file writes are covered by two loopback cores; native
clipboard worker dispatch and echo prevention are checked with a private AppKit
pasteboard. IME composition and distribution packaging remain unfinished.
The Mac/Linux live clipboard and file-transfer matrix still needs validation.
Login registration across an actual
logout/login cycle and permission-granted sharing resume still need validation;
registration is not enabled automatically by development builds.
A paired-device grant alone does not prove successful desktop operation.

Local desktop regressions can be run without a second computer:

```sh
apple/scripts/build-core.sh
swift test --package-path apple
DYLD_LIBRARY_PATH=/usr/lib/swift MACOSX_DEPLOYMENT_TARGET=15.0 cargo test --locked -p vnilink-ffi --lib desktop_tests
```

The Swift tests use a private pasteboard and temporary files, and never post
keyboard or pointer input to the OS. The two-core contract tests use loopback
connections and a recording desktop adapter.

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

## Remote-session tabs

Home retains the device list and settings sidebar. Connect opens a named tab;
connecting to the same computer focuses its existing tab. Switching tabs keeps
connections and decoders alive. Each close button stops only that tab's session.
Command-Shift-[ / ] selects the previous/next tab; Command-W closes the selected
remote tab. Settings returns to Home. Files windows remain separate utilities.
Clipboard synchronization follows the selected desktop, and hidden connections
keep their latest remote clipboard state. See
[controller-session architecture](../docs/architecture/controller-sessions.md).

For a preview without Keychain changes, launch with `--preview --preview-tabs`.
The two preview session pages are labelled as previews and do not connect to
remote computers.
