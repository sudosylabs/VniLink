# VniLink security policy

VniLink is a remote desktop application. A security failure can expose a
computer's screen, inject keyboard or pointer input, disclose clipboard text,
or write files. Reports affecting these boundaries should be made privately.

## Maintained versions

VniLink is in development. Security fixes target the latest `main` branch;
older commits and unofficial builds do not have a separate maintenance track.
This policy will be updated when versioned releases have a support policy.
The workspace version is not a claim that a production release is available.

The [README](README.md) and [validation matrix](docs/validation/matrix.md)
describe implementation and verification limits. Passing library tests does
not establish secure, complete remote desktop support on every platform.

## Report a vulnerability

Email [support@sudosy.fr](mailto:support@sudosy.fr?subject=%5BVniLink%20Security%5D)
with the subject prefix `[VniLink Security]`.

Do not publish vulnerability details in issues, discussions, or pull requests.
If email is unavailable, open an issue titled **Security contact request** with
no technical details or sensitive attachments, and ask for a private channel.
Community conduct reports follow [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

Include, where available:

- The affected commit or build, operating system, and desktop environment.
- The suspected impact and the access, grants, or user actions required.
- Reproduction steps or a minimal proof of concept using disposable test data.
- Whether the connection is direct, uses public relays, or uses a custom relay.
- Redacted logs, relevant evidence, and any suggested mitigation.
- Whether the issue has been shared elsewhere and any proposed disclosure date.

Never include installation secrets, relay credentials, reusable pairing files,
private screen captures, clipboard contents, or personal files. Redact device
identifiers, addresses, and paths when they are not necessary to reproduce the
issue. Do not repeat a dangerous reproduction against your everyday computer
just to test the latest commit.

## Security-sensitive areas

Reports are particularly relevant when they involve:

- Forged, replayed, expired, or incorrectly accepted pairing invitations;
  bypasses of pairing or control confirmation; or abuse of confirmation attempts.
- Control without the target's grant, failure to enforce revocation or local
  stop, or unintended persistence of hosting permission.
- Stale-session input or data being accepted after disconnect or reconnect,
  or held keys and buttons remaining pressed after a session ends.
- Clipboard or file access without the separate feature grant.
- Directory escape, symlink races, unintended overwrite, or partial file
  publication during transfer.
- Exposure of installation records, credentials, screen contents, or peer
  information through storage, errors, events, or diagnostics.
- Malformed frames, media, or peer messages causing memory-safety failures,
  resource exhaustion, crashes, or arbitrary code execution.
- Bypasses of operating-system capture or input permissions, or incorrect
  assumptions about locked, elevated, pre-login, or Wayland sessions.

The current contracts for pairing, consent, reconnect, clipboard and file
transfer are summarised in [ARCHITECTURE.md](ARCHITECTURE.md) and documented in
the modules that implement them. These describe intended boundaries;
they do not replace investigation of a reported failure.

## Handling and disclosure

Maintainers will aim to acknowledge a report, assess the affected code and
platforms, reproduce the issue safely, and develop and verify a fix. Disclosure
and reporter credit will be coordinated with the reporter. Response and fix
times depend on severity, complexity, and maintainer availability; no fixed
deadline is promised.

For a third-party dependency issue, also report it to that dependency's
maintainers. Notify VniLink privately if its use of the dependency is affected.

## Research boundaries

Use computers, accounts, networks, relays, and data you own or have explicit
permission to test. Use isolated installations and disposable files. Stop if
testing reaches another person's data or device, and avoid disruption,
destructive actions, or unnecessary collection of private information.

This policy does not authorize testing public relay infrastructure or other
users' devices, and it does not establish a bug bounty.
