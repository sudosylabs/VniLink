# Contributing to VniLink

Contributions can include code, bug reports, documentation, tests, design,
accessibility work, and platform validation. Follow the
[Code of Conduct](CODE_OF_CONDUCT.md). Report suspected vulnerabilities
privately through [SECURITY.md](SECURITY.md).

## Before starting

Read the [README](README.md) and the documentation for the component you want
to change. Search existing issues and pull requests. Discuss substantial
features, protocol changes, permission changes, or new platform support with
maintainers before committing to an implementation. Small corrections do not
need a separate proposal.

Keep changes focused. Do not include unrelated refactors, dependency updates,
or repository-wide formatting. Never commit secrets, installation records,
relay credentials, live pairing files, or another person's screen or files.

## Set up a checkout

Fork [sudosylabs/VniLink](https://github.com/sudosylabs/VniLink), clone your fork,
and create a branch from the current upstream `main`. Branch names should
describe the change, for example `feat/device-status`, `fix/input-cleanup`, or
`docs/platform-setup`. Open pull requests against `main`.

Install what your OS needs, then, from the repository root:

```sh
npm ci            # checks this computer, installs, prepares Rust and Electron
npm run doctor    # what each task needs here: ready, missing (and how to get it), unsupported
npm run dev       # builds what is missing and launches the development app
```

`scripts/code.sh` (`scripts\code.bat` on Windows) does the same as `npm run dev`,
and `scripts/test.sh` (`scripts\test.bat`) runs the UI scenarios; like VS Code's
scripts of those names, they install and build what is missing first. Every
`node build/run.ts` task checks what it needs before it starts and names the
command that installs anything missing; `VNILINK_SKIP_PREFLIGHT=1` skips that
check for setups it misjudges. `build/prerequisites.json` lists each OS's needs
per task; the doctor, `npm ci`, the Linux container, CI and the section below are
all read from it.

On Ubuntu 23.10 and later, AppArmor stops Chromium's sandbox until a profile
allows it: run `npm run sandbox:linux` once per checkout (it asks for sudo).
Normal startup on Linux reads its key from the desktop's Secret Service, such
as GNOME Keyring.

<!-- prerequisites:start -->
<!-- Generated from build/prerequisites.json by `node build/run.ts docs:prerequisites`; edit the manifest, not this section. -->

Every OS needs Node.js 22.19.0 (`.nvmrc`; npm refuses other versions) and Rust 1.91.0 through rustup (`rust-toolchain.toml`). After installing what your OS lists below, `npm ci` installs the dependencies and prepares the Rust toolchain and the development Electron, and `npm run doctor` reports what each task still needs. Every task checks its own needs before it starts.

### macOS

Architectures: arm64 supported; x64 unsupported (Intel Macs cannot build VniLink: the vendored AV1 and VP8/VP9 build scripts accept only Apple silicon (`vendor/shiguredo-aom/build.rs`, `vendor/shiguredo-libvpx/build.rs`)).

- **Node.js**: Install the version in `.nvmrc` with a version manager: `nvm install` (macOS, Linux), `nvm install $(Get-Content .nvmrc)` (nvm-windows), or Volta, which takes it from `package.json`; or from https://nodejs.org.
- **Rust toolchain**: Builds the native addon (crates/vnilink-node). Install rustup from https://rustup.rs; `npm ci` then installs the toolchain `rust-toolchain.toml` names.
- **Xcode Command Line Tools**: Clang, the SDK and the system frameworks the addon links. `xcode-select --install`.
- **git**: The first native build clones codec sources, check:hygiene lists the repository's files, and the VS Code import reads the upstream checkout's commit. `xcode-select --install`.
- **curl**: The first native build downloads codec and CMake sources with it. `xcode-select --install`.
- **CMake**: Builds Opus for the audio codec (opusic-sys); the AV1 build downloads its own CMake. `brew install cmake`.
- **Liquid Glass prebuild (@vnilink/elg)**: The optional npm dependency the main process loads for window material. `npm ci` prepares it.
- **Xcode (actool)** (optional): Compiles the Icon Composer icon of the development app; without it the flat icon is used. Install Xcode from the App Store, then `sudo xcode-select -s /Applications/Xcode.app`.
- **ffprobe** (optional): Reads the recordings the Rust tests write, as a decoder that is not VniLink's; without it that test says it skipped. `brew install ffmpeg`.

Some tasks need more:

| Task | Also needs |
| --- | --- |
| `npm run test:linux` | Docker: Install and start Docker Desktop: https://docs.docker.com/desktop/ |
| `npm run measure:impaired` | Docker: Install and start Docker Desktop: https://docs.docker.com/desktop/ |
| `npm run measure:latency` | Screen Recording for the development VniLink: Run the task; on its first run macOS asks and the run stops. Allow VniLink in System Settings > Privacy & Security > Screen & System Audio Recording, then run it again; A second computer (optional) |
| `npm run measure:perceived` | Screen Recording for the application running the task: Allow that application in System Settings > Privacy & Security > Screen & System Audio Recording, then quit and reopen it; Accessibility for the application running the task: Allow that application in System Settings > Privacy & Security > Accessibility; FFmpeg: `brew install ffmpeg`; A second computer (optional) |
| `npm run assets:tray` | rsvg-convert: `brew install librsvg` |

### Windows

Architectures: x64 supported; arm64 unsupported (Windows on Arm has no GNU (MinGW) build of the native addon). VniLink builds with the GNU toolchain (`x86_64-pc-windows-gnu`, MSYS2's MinGW-w64); Visual Studio and the MSVC toolchain are not needed when rustup's host is GNU.

- **Node.js**: Install the version in `.nvmrc` with a version manager: `nvm install` (macOS, Linux), `nvm install $(Get-Content .nvmrc)` (nvm-windows), or Volta, which takes it from `package.json`; or from https://nodejs.org.
- **Rust toolchain**: Builds the native addon (crates/vnilink-node). Install rustup from https://rustup.rs; `npm ci` then installs the toolchain `rust-toolchain.toml` names.
- **Rust GNU Windows target**: The addon builds for x86_64-pc-windows-gnu with MSYS2's MinGW, not MSVC. `npm ci` prepares it.
- **Rust host toolchain without MSVC**: Build scripts run on the host toolchain: a GNU host needs only MinGW, an MSVC host also needs Visual Studio's linker. `rustup set default-host x86_64-pc-windows-gnu`, then `rustup toolchain install`.
- **MSYS2 MinGW-w64 tools**: Links the addon and builds its C dependencies (gcc, dlltool, libclang, nasm, mingw32-make). `winget install MSYS2.MSYS2`, then in the MSYS2 shell `pacman -S --needed mingw-w64-x86_64-gcc mingw-w64-x86_64-clang mingw-w64-x86_64-nasm mingw-w64-x86_64-make`. Put `C:\msys64\mingw64\bin` on PATH or set `VNILINK_MSYS_ROOT` to the MSYS2 root.
- **git**: The first native build clones codec sources, check:hygiene lists the repository's files, and the VS Code import reads the upstream checkout's commit. `winget install Git.Git`.
- **curl**: The first native build downloads codec and CMake sources with it. Included in Windows 10 and later.
- **CMake**: Builds Opus for the audio codec (opusic-sys); the AV1 build downloads its own CMake. `pacman -S mingw-w64-x86_64-cmake` in the MSYS2 shell, or `winget install Kitware.CMake`.
- **ffprobe** (optional): Reads the recordings the Rust tests write, as a decoder that is not VniLink's; without it that test says it skipped. `pacman -S mingw-w64-x86_64-ffmpeg` in the MSYS2 shell.

Some tasks need more:

| Task | Also needs |
| --- | --- |
| `npm run test:linux` | Docker: Install and start Docker Desktop: https://docs.docker.com/desktop/ |
| `npm run measure:impaired` | Docker: Install and start Docker Desktop: https://docs.docker.com/desktop/ |
| `npm run measure:latency` | Signed-in desktop: Run the task from a terminal on the signed-in desktop; A second computer (optional) |
| `npm run assets:tray` | rsvg-convert: `pacman -S mingw-w64-x86_64-librsvg` in the MSYS2 shell |

### Linux

Architectures: x64 supported; arm64 unverified (it builds in the Linux container on Apple silicon but has not run on an arm64 desktop). Distributions: Ubuntu and Debian, Fedora; others are unverified, and the doctor names the pkg-config modules, libraries and commands to find. Screen capture and input go through the XDG desktop portal; only GNOME's has been exercised. Other desktops are unverified.

Ubuntu and Debian (Debian 13 (the Linux test container) and Ubuntu 26.04; CI runs Ubuntu 24.04):

```bash
sudo apt-get install --no-install-recommends build-essential cmake curl dbus-daemon dbus-x11 ffmpeg git gnome-keyring libasound2-dev libasound2t64 libatspi2.0-0 libclang-dev libdbus-1-dev libdrm2 libegl-dev libgbm-dev libgtk-3-0 libnotify4 libnss3 libpipewire-0.3-dev libsecret-1-0 libspa-0.2-dev libva-dev libwayland-dev libxcb1 libxkbcommon0 libxss1 libxtst6 nasm openbox perl pkg-config procps x11-utils xauth xdg-utils xvfb
```

Fedora (CI runs Fedora 43):

```bash
sudo dnf install alsa-lib alsa-lib-devel at-spi2-core clang-devel cmake curl dbus-daemon dbus-devel dbus-x11 ffmpeg-free gcc gcc-c++ git gnome-keyring gtk3 libXScrnSaver libXtst libdrm libglvnd-devel libnotify libsecret libva-devel libxcb libxkbcommon make mesa-libgbm-devel nasm nss openbox perl pipewire-devel pkgconf-pkg-config procps-ng wayland-devel xdg-utils xorg-x11-server-Xvfb xorg-x11-xauth xprop
```

- **Desktop session**: The application opens windows on an X11 or Wayland display. Run it from a graphical session (`DISPLAY` or `WAYLAND_DISPLAY` set).
- **Secret Service**: Normal startup reads the installation key from the desktop's Secret Service (org.freedesktop.secrets). Install and unlock a Secret Service provider such as GNOME Keyring or KeePassXC.
- **Chromium sandbox**: Where AppArmor restricts unprivileged user namespaces (Ubuntu 23.10 and later), the development Electron needs a profile allowing them. `npm run sandbox:linux` (asks for sudo, once per checkout).
- **UI tests without a display**: `scripts/test.sh` runs them on Xvfb with Openbox and a temporary keyring.

Some tasks need more:

| Task | Also needs |
| --- | --- |
| `npm run test:linux` | Docker: Ubuntu and Debian: `sudo apt-get install docker.io && sudo usermod -aG docker $USER`, then log in again; Fedora: `sudo dnf install moby-engine && sudo systemctl enable --now docker && sudo usermod -aG docker $USER`, then log in again |
| `npm run measure:impaired` | Docker: Ubuntu and Debian: `sudo apt-get install docker.io && sudo usermod -aG docker $USER`, then log in again; Fedora: `sudo dnf install moby-engine && sudo systemctl enable --now docker && sudo usermod -aG docker $USER`, then log in again |
| `npm run measure:impaired:linux` | ip and tc: Ubuntu and Debian: `iproute2`; Fedora: `iproute iproute-tc`; Root: Run it as root (`sudo node build/run.ts measure:impaired:linux`), or run `npm run measure:impaired`, which runs it in a container |
| `npm run measure:latency` | Screen sharing through the desktop portal: Run the task in the graphical session and choose the display in both portal dialogs; the answers are kept in `.build/latency` for later runs; A second computer (optional) |
| `npm run package:installers` | rpmbuild: Ubuntu and Debian: `rpm`; Fedora: `rpm-build` |
| `npm run assets:tray` | rsvg-convert: Ubuntu and Debian: `librsvg2-bin`; Fedora: `librsvg2-tools` |
| `npm run sandbox:linux` | AppArmor |

The first native build downloads, once:

- macOS, Windows, Linux: CMake 4.4 (about 50 MB), into `~/.cache/shiguredo_cmake`.
- macOS: libvpx and libaom prebuilt archives from github.com/shiguredo, into `target/electron`.
- Windows: libvpx prebuilt archive from github.com/shiguredo and the libaom sources from aomedia.googlesource.com, into `target/electron`.
- Linux: the libvpx and libaom sources from chromium.googlesource.com and aomedia.googlesource.com, into `target/electron`.
<!-- prerequisites:end -->

## Find the owning component

| Path | Responsibility |
| --- | --- |
| `crates/vnilink/` | Identity, pairing, grants, networking, sessions, input, clipboard, transfer, and encoded-frame transport |
| `crates/vnilink-media/` | Frame formats, pacing, and codec-independent media handling |
| `crates/vnilink-macos/` | macOS capture and VideoToolbox adapters |
| `crates/vnilink-windows/` | Windows capture and Media Foundation adapters |
| `crates/vnilink-linux/` | X11, Wayland portal, PipeWire, and VA-API adapters |
| `crates/vnilink-app/`, `crates/vnilink-node/` | Shared application adapters and Node-API control bindings |
| `src/code/`, `src/platform/` | Electron lifecycle, windows, IPC and desktop service boundaries |
| `src/workbench/`, `src/base/browser/` | Browser interface and reusable DOM controls |
| `build/`, `scripts/`, `test/` | Build tasks (`node build/run.ts <task>`), prerequisites (`build/prerequisites.json`), launchers, the Windows suite, and tests |
| `assets/` | Branding and icons |

The shared Electron application hosts the Rust library through a Node-API control bridge. Keep platform capture,
rendering, and credential-store integration in their owning adapters or apps.
Follow nearby code style and explain non-obvious invariants in comments.

## Preserve the security contracts

Changes must preserve deliberate pairing and target-local authorization.
Control, clipboard, and file transfer have distinct grants. Keep local stop,
revocation, session-generation checks, reconnect cleanup, and bounded input and
media handling intact. File writes must stay inside the selected transfer
directory and preserve the explicit replacement policy.

Read the relevant [pairing](crates/vnilink/PAIRING.md),
[keyboard](crates/vnilink/KEYBOARD.md),
[reconnect](crates/vnilink/RECONNECT.md),
[clipboard](crates/vnilink/CLIPBOARD.md), and
[transfer](crates/vnilink/TRANSFER.md) contracts before changing those paths.
Update contracts and regression coverage in the same pull request when
behavior changes.

## TypeScript, CSS and formatting

The application follows VS Code's separation of formatting, semantic lint and
architecture checks, adapted to this smaller codebase. TypeScript/JavaScript use
the TypeScript language-service formatter with `tsfmt.json`: tabs at width 4,
spaces around operators and same-line opening braces. ESLint enforces single
quotes, semicolons, braces, strict equality, explicit type imports and correct
promise handling. Relative TypeScript imports always use the emitted `.js` suffix,
including type imports and re-exports; stylesheet imports retain `.css`. NodeNext
resolution checks these paths. The strict type-aware preset also checks unsafe
assignments/assertions, unnecessary conditions, exhaustive switches and explicit
member visibility. Production processes must use asynchronous filesystem APIs. JSON uses two spaces. CSS/HTML/JSON use Prettier, with CSS
correctness checked by Stylelint. Rust continues to use `rustfmt`; generated,
vendored and historical evidence files are outside the JavaScript formatter.

```sh
npm run format          # Format application and tooling files
npm run lint:fix        # Fix supported lint findings
npm run check           # Formatting, ESLint, CSS lint, types and tooling tests
npm test                # Actual Electron shell smoke test
```

The flat ESLint configuration and `build/lint/layers.ts` enforce dependency
boundaries: browser/common code cannot import Node/Electron or privileged
implementations, base controls cannot depend on features/services, platform
services cannot depend on the workbench/application, and Electron application
code cannot import browser implementation. Add a narrow preload operation instead
of reaching across those boundaries. The rule covers imports, re-exports,
dynamic imports and literal require calls. Layer tests include deliberately
invalid dependencies; formatter tests check idempotence and literal preservation.

`npm ci` installs the Husky hooks. The pre-commit hook checks only what is
staged, with lint-staged's backup and partial-staging safeguards: supported staged
files are lint-fixed and formatted without staging unrelated edits, staged
TypeScript runs `npm run typecheck`, and staged Rust runs `cargo fmt --all -- --check`
(it reports but does not rewrite; run `cargo fmt --all`), then `npm run check:hygiene`
rejects personal home paths and author handles anywhere. CI runs the full
suite on macOS, Windows, Ubuntu, Fedora and the Debian container. To run that
suite before each push as well, opt in with `git config vnilink.prePush full`;
`npm run validate` runs it by hand (checks, Rust formatting and tests, native
bridge and UI scenarios).
Ignored docs, vendor files and generated output stay outside formatting scope.

Workspace editor settings recommend ESLint, Prettier and Stylelint integrations.
They use the built-in TypeScript formatter and the project TypeScript SDK.
Dependencies are pinned. TypeScript 6.0.3 is used because the current
`typescript-eslint` release supports TypeScript below 6.1; don't move the compiler
outside its supported range without validating the lint tooling too. VS Code's
extension-specific, localization and dependency-injection rules are not copied.

## Interface icons

Use `Icons` from `src/base/common/icons.ts` and `renderIcon` from
`src/base/browser/ui/icon/icon.ts`. Components use product names, never provider
names or file paths. `renderIcon(Icons.home, { variant: 'filled' })` selects a
filled icon; the default is outlined. Buttons accept an options object, for
example `button('Back', 'nav-item', { icon: Icons.back, onClick: goBack })`.

Icons inherit text color and use `--icon-size` (16px by default). Decorative
icons are hidden from assistive technology. Name icon-only buttons with an
`aria-label` on the button; use the icon's `label` option only when the icon
itself conveys information outside a labeled control.

Add icon IDs to the shared registry and map both variants in `iconAssets.ts`.
Only that provider may import Phosphor. The build bundles the selected SVGs
from the pinned `@phosphor-icons/core` dependency, requires no network requests
or icon font, and copies its MIT license to `out/licenses/phosphor-icons.txt`.
The Electron smoke test verifies every registered variant, sizing, color
inheritance, accessibility, independent DOM instances and button behavior.

## Imported VS Code source

The titlebar and native-window adapters follow Microsoft's VS Code release
`1.141.0`, with `src/vs` flattened into `src`. The running shell registers
services with `ServiceCollection`, `SyncDescriptor`, `registerSingleton` and
`InstantiationService`, then mounts `NativeTitlebarPart`. Electron owns the
native window controls; the renderer owns the draggable titlebar, view tabs,
contextual actions and theme. Rust remains responsible for the application engine.

The titlebar's editor, command-center, menu, toolbar and window-title code has
been removed. Its dependency closure contains only the remaining adapters,
service-container implementation and their required libraries and declarations.
`build/vscode-source.json` records the release, original paths, dependency edges
and hashes. Files marked `adapted` contain VniLink-specific changes and receive
normal formatting, linting and staged fixes. Unchanged upstream files retain
their source formatting. Preserve the Microsoft copyright headers,
`LICENSE.vscode.txt` and `ThirdPartyNotices.vscode.txt` when redistributing them.

`npm run check:upstream` verifies hashes and the complete dependency closure.
When intentionally changing an adapted module, update its reviewed hash and
regenerate dependency edges in the manifest. New application dependencies must
be explicitly listed rather than silently excluded from the graph.

Both TypeScript projects use legacy parameter decorators and compatible compiler
settings. `src/tsconfig.json` checks every retained upstream and adapted module;
the root configuration checks the application entry points and tests. CSS imports
are declared in `src/assets.d.ts`. `npm run typecheck` checks both projects.
The Electron smoke test exercises native controls, window-state IPC, theme
updates, fullscreen layout, tab overflow and request validation on the current platform.

VniLink views implement `WorkbenchView` and open through `ViewHost`. `ITabService`
owns their lifetimes, selection and order. Switching hides the existing element
without disposing it; closing calls optional `canClose`, aborts pending actions
and disposes the view. Each view supplies actions and may emit
`onDidChangeActions` when their availability changes. Actions receive an
`AbortSignal` and must avoid applying results after cancellation. The titlebar
uses a scrolling tab region beside a separate toolbar for the active view.
Its connected presentation adapts VS Code's `modernUI/browser/media/tabs.css`
and `connectedEditorTabs.css`: the active tab shares its content surface, with
curved shoulders and an open bottom edge. A view owns its whole layout,
including any sidebar; closing Computers removes both its sidebar and page.
Computers is the first integrated view; stream and terminal views are deferred.

## Verify the change

Run checks from the repository root. Start with the affected crate's tests,
then broaden checks when the change crosses component boundaries.

```sh
cargo fmt --all -- --check
cargo test --locked -p vnilink -p vnilink-media
git diff --check
```

For a platform adapter, run its tests on that platform:

```sh
cargo test --locked -p vnilink-macos    # macOS
cargo test --locked -p vnilink-windows  # Windows
cargo test --locked -p vnilink-linux   # Linux, with native development libraries
```

On a host with the required dependencies, broader checks are:

```sh
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
```

For Electron changes, run `npm run build` and `npm test`, then inspect the
affected flow in `npm start`. Validate platform-specific window/input/media
behavior on its real OS. Capture/input checks can access devices or request
OS permissions; use a test environment you control.

Every task runs on one computer. The measurements
(`npm run measure:latency`, `npm run measure:perceived`, `npm run measure:impaired`)
run on this computer by default; `--peer` adds a second computer, running the
test agent, to the latency and perceived-quality runs and is never required. `npm run doctor`
reports the OS permissions a task needs, as granted, missing, or unknown until
the run asks for them. [LATENCY.md](build/LATENCY.md) and
[PERCEIVED.md](build/PERCEIVED.md) describe each run and what it proves.

A service's suites (`node build/run.ts test:service <service>` and
`bench:service <service>`) run both engines on this computer. With
`--peer <file.vnl> --peer-check <digits>` the host half runs instead on a second
computer built from the same commit, in `node build/run.ts testkit:agent`: the
agent writes that file and its digits, and asks on its own terminal for the
digits the suite prints. The files, desktop, camera and terminal measurements
have peer scenarios; desktop and camera convert the agent's times to this
computer's clock, within half the fastest round trip of the estimate, and
record that error with the run. The terminal scenario gives the suite a real
shell on the agent's computer, working in a folder of the scenario's own that
the agent names on its terminal. When the scenario stops, or the agent is
interrupted, idles or loses its suite, the agent ends the shell and what it
started, in the background or under `nohup` too, and removes the folder. On
macOS a process that leaves the shell's session after its parent ended is not
found. The scenario runs only when the agent was started with
`--allow terminal`. Files shares a whole disk or nothing, so the files
scenario gives the suite's computer this computer's whole disk while it
runs: it runs only when the agent was started with `--allow files`, the
agent says on its terminal when the disk is shared and when that ends, and
uploads land in a folder of the scenario's own that the agent names and
removes when the scenario stops. Other suites
refuse `--peer`. A peer run is saved apart from this computer's runs and
reports regressions without failing. The latency and perceived-quality
measurements reach the same agent through `vnilink-testkit bridge`, and the
agent starts their other side from its own checkout; the latency sides show
its real screen, so they run only when the agent was started with
`--allow screen`. `--allow` repeats or takes a comma list
(`--allow screen,terminal,files`).

A build on macOS does not exercise Windows or Linux native code. Synthetic
frames and loopback sessions do not prove an interactive remote desktop across
the internet. For integration changes, record hardware, OS, desktop session,
network path, steps, and actual outcomes in the pull request. Clearly
distinguish implemented, tested, proposed, unsupported, and blocked behavior.

For documentation-only changes, check relative links, paths, examples, and
claims against the source, and run `git diff --check`. Runtime tests are not
needed unless the change also affects executable code or configuration.

## Submit a pull request

Explain the problem and resulting behavior, link the relevant issue if there
is one, and list the checks and manual scenarios actually performed. Include
platform and compatibility implications, relevant test coverage, and remaining
limitations. State which checks could not run and why. Do not report skipped
or unavailable checks as passing.

Use concise commit messages describing the outcome. Maintainers may request
changes to scope, design, tests, security, accessibility, or documentation.
Review and merge timing depends on maintainer availability.

The Rust workspace declares Apache-2.0 licensing. Preserve applicable copyright
and license notices and only contribute material you have the right to submit
under the destination's license. Identify the source and license of copied or
adapted material; the [Code of Conduct](CODE_OF_CONDUCT.md) has its own stated
license.
