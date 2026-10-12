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
dependencies. The shared desktop application uses Electron and vanilla TypeScript/CSS.
Install Node 22.19 or later, run `npm ci` from the repository root, then
`npm start`. `npm run build` and `npm test` verify the shell; the Rust control bridge
runs in an Electron utility process; streaming remains deferred.

## Find the owning component

| Path | Responsibility |
| --- | --- |
| `crates/vnilink/` | Identity, pairing, grants, networking, sessions, input, clipboard, transfer, and encoded-frame transport |
| `crates/vnilink-media/` | Frame formats, pacing, and codec-independent media handling |
| `crates/vnilink-macos/` | macOS capture and VideoToolbox adapters |
| `crates/vnilink-windows/` | Windows capture and Media Foundation adapters |
| `crates/vnilink-linux/` | X11, Wayland portal, PipeWire, and VA-API adapters |
| `crates/vnilink-app/`, `crates/vnilink-node/` | Shared application adapters and Node-API control bindings |
| `src/app/`, `src/platform/` | Electron lifecycle, windows, IPC and desktop service boundaries |
| `src/workbench/`, `src/base/browser/` | Browser interface and reusable DOM controls |
| `build/`, `scripts/`, `test/` | Application build, development and smoke verification |
| `assets/` | Branding and icons |
| `docs/validation/` | Reproducible validation methods, results, and remaining cases |

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

The flat ESLint configuration and `build/lint/layers.mjs` enforce dependency
boundaries: browser/common code cannot import Node/Electron or privileged
implementations, base controls cannot depend on features/services, platform
services cannot depend on the workbench/application, and Electron application
code cannot import browser implementation. Add a narrow preload operation instead
of reaching across those boundaries. The rule covers imports, re-exports,
dynamic imports and literal require calls. Layer tests include deliberately
invalid dependencies; formatter tests check idempotence and literal preservation.

`npm ci` installs the Husky pre-commit hook automatically. It runs lint-staged
with its default backup and partial-staging safeguards: supported staged files
are lint-fixed and formatted without staging unrelated edits. It then runs the
full `npm run check`, Rust formatting checks and `npm test` (native bridge plus
Electron smoke). Any failure blocks the commit. Fix errors and retry; the hook
requires the same native build dependencies as the app. Rust files are checked
but not automatically rewritten; use `cargo fmt --all` before committing.
Ignored docs, vendor files and generated output stay outside formatting scope.

Workspace editor settings recommend ESLint, Prettier and Stylelint integrations.
They use the built-in TypeScript formatter and the project TypeScript SDK.
Dependencies are pinned. TypeScript 6.0.3 is used because the current
`typescript-eslint` release supports TypeScript below 6.1; don't move the compiler
outside its supported range without validating the lint tooling too. VS Code's
extension-specific, localization and dependency-injection rules are not copied.

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
