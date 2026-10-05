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

Install Rust through rustup. The repository's `rust-toolchain.toml` selects
Rust **1.91.0**; Cargo uses the committed lockfile.

```sh
rustup component add rustfmt clippy --toolchain 1.91.0
cargo test --locked -p vnilink -p vnilink-media
```

Platform work requires the native host and libraries for that platform.
macOS capture and codec work requires Xcode's developer tools; Windows work
requires the MSVC toolchain and Windows SDK. Linux capture and media builds
require the development libraries for PipeWire, VA-API, GBM, and their build
dependencies. The GNOME application additionally requires GTK 4.10+,
libadwaita 1.5+, and a session Secret Service. See the
[Linux application guide](linux/README.md) for the app entry point.

## Find the owning component

| Path | Responsibility |
| --- | --- |
| `crates/vnilink/` | Identity, pairing, grants, networking, sessions, input, clipboard, transfer, and encoded-frame transport |
| `crates/vnilink-media/` | Frame formats, pacing, and codec-independent media handling |
| `crates/vnilink-macos/` | macOS capture and VideoToolbox adapters |
| `crates/vnilink-windows/` | Windows capture and Media Foundation adapters |
| `crates/vnilink-linux/` | X11, Wayland portal, PipeWire, and VA-API adapters |
| `linux/` | Native GTK/libadwaita application and keyring integration |
| `assets/`, `apple/` | Artwork and the Apple Icon Composer resource; not evidence of a completed Apple app |
| `docs/validation/` | Reproducible validation methods, results, and remaining cases |

Native applications host the shared Rust library. Keep platform capture,
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

For GNOME changes, also build and run `cargo run --locked -p vnilink-gnome`
in a Linux graphical session and inspect the affected flow. Capture and input
checks may access real devices or request operating-system permission; use a
test environment you control.

A build on macOS does not exercise Windows or Linux native code. Synthetic
frames and loopback sessions do not prove an interactive remote desktop across
the internet. For integration changes, record hardware, OS, desktop session,
network path, steps, and actual outcomes in the
[validation matrix](docs/validation/matrix.md). Clearly distinguish implemented,
tested, proposed, unsupported, and blocked behavior.

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
