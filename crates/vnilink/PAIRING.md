# Pairing with a `.vnl` file

One signed file introduces two computers. Iroh carries the authenticated connection after that introduction, using the installation's configured relays and direct addresses. There is no VniLink pairing, directory or presence server.

## Owner confirmation

1. Export a `.vnl` file with incoming pairing approval enabled. This opens a local pairing window for the file's lifetime, at most fifteen minutes; it does not enable sharing.
2. Give the file to the other computer. Import it and confirm the three digits shown by its writer. This remembers the writer's identity, hostname, OS family and addresses.
3. Start a neutral connection with `Core::begin_connect_services`. The writer asks its owner to confirm the importing computer's three digits from `Core::control_check`. `Core::confirm_control_request` remembers that authenticated endpoint. A rejection, mismatch or expired window does not remember it.
4. Once approved, both computers remember each other. Either can connect again or request services without a second file. Each computer must be hosting to accept incoming connections.

The existing `allow_control` export argument and `control_request` API names refer to the local pairing approval window. They do not grant screen control. A pending approval expires after sixty seconds. Pairing approval consumes the local window; later introductions require another export.

## Global sharing

`SharingPolicy` belongs to the installation, not to a device. Fresh installations start with sharing disabled. The owner can allow screen viewing/control, files, terminal, camera, microphone, system audio, clipboard, recording and system actions. A remembered identity is required even when sharing is enabled.

`Core::change_sharing_policy` applies an atomic partial change; `Core::set_sharing_policy` replaces the whole policy. Consent is persisted before revoking live work. Failed persistence leaves the previous consent and work unchanged. Disabling a service denies new requests and closes affected hosted owners across all paired devices. Disabling recording invalidates existing recording leases. Unrelated services remain connected; enabling a service never starts it automatically.

Native permissions, backend availability and remote action rules are independent gates. An OS label is descriptive metadata, never authorization. Forgetting a device removes its remembered identity and ends its connections without changing global sharing.

## Metadata and presence

The signed invitation contains the writer's OS family (`macos`, `windows`, `linux`, or `other`). Authenticated Hello/Accept messages refresh metadata in both directions. The installation record retains it alongside names and addresses.

Presence describes existing sessions: connected, connecting, reconnecting or disconnected. An active connection in either direction wins over older closed records. The workbench listens to engine lifecycle events and refreshes snapshots; it never infers online status from cached addresses or sends new presence probes. A failed engine read is unknown state rather than proof that a peer disconnected.

## Ownership and checks

`src/pairing/` owns file framing and the two three-digit checks. `src/core/pairing.rs` owns approval and remembered identities; `src/core/sharing.rs` owns global consent transactions and revocation. `src/core/link.rs` derives presence. Tests cover signed metadata, malformed handshakes, pairing in both directions without video support, policy persistence/rollback, concurrent partial writes and service teardown.
