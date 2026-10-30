# VniLink architecture

This guide summarises the contracts that span modules and says where each one is implemented. How a single module works is documented in that module's header comment and on its items; this guide names the code and does not restate it. Documentation may cite code. Code never cites documentation, so a comment names symbols and the behaviour they implement.

For the product, the repository layout and the verification matrix, read the [README](README.md). The layer rules for contributors are in `src/AGENTS.md` and `crates/AGENTS.md`.

## Boundary

Rust owns identity, pairing, authorization, connections, remote services, media and transfers. Electron TypeScript owns the application and window lifecycle, the workbench, presentation and local integration. The sandboxed renderer calls a typed preload API; an Electron utility process hosts the Rust Node addon (`vnilink-node`), which calls the facade (`vnilink-app`), which composes the core (`vnilink`) and the platform backends (`vnilink-macos`, `vnilink-windows`, `vnilink-linux`). Core contracts depend on neither Electron nor a platform crate.

## Pairing

One signed `.vnl` file introduces two computers. Iroh carries the authenticated connection afterwards; there is no VniLink pairing, directory or presence server.

- `Core::export_pairing_file` opens one `PairingWindow` for at most `invitation::MAX_TTL`; a later export replaces it. `Core::open_pairing_file` and `Core::confirm_pairing` check the digits the writer shows (`pairing::file_digits`) and save the writer. The invitation's nonce goes in `ReplayCache`, so an invitation is accepted once.
- The writer's owner then confirms the importing computer. An unpaired Hello reaches `wait_for_control_approval` in `core/host.rs`, which waits up to `CONTROL_APPROVAL_WAIT` only while the window is open and `allow_control` was set. It publishes `CoreEvent::ControlRequested` with the digits from `Core::control_check`; `Core::confirm_control_request` pairs the endpoint and consumes the window. A mismatch, rejection, expiry or stop refuses the Hello and remembers nothing.
- Pairing remembers an identity and grants nothing. Persistence goes through `persist_paired`, which rolls back when the save fails.

## Consent and admission

- `SharingPolicy` is installation-wide consent, not a per-device setting, and starts with everything off. `Core::change_sharing_policy` applies a partial change and `Core::set_sharing_policy` replaces it, both through `Core::update_sharing_policy`, which serialises changes, saves first and rolls back on a failed save. A grant is `Trust::granted(peer, Grant::…)`, which also requires a remembered device.
- After a successful save, `reconcile_grants` makes live work match the new policy: it releases held input, closes hosted services as `CloseCause::Revoked`, ends file pipes and jobs, stops terminals, releases privacy mode and invalidates recording leases. Enabling a service never starts it.
- `session::admit_services` decides whether a connection becomes a link: hosting, protocol version, a paired device, then the link limit (`NotHosting`, `IncompatibleVersion`, `NotPaired`, `Busy`). `Core::begin_connect_services` returns the one live link per peer, and each service attaches to it as its own owner.
- Native permissions, backend availability and the remote action rules are independent gates. An OS label is metadata, never authorization. `Core::unpair` removes an identity without changing the policy.

## Links, generations and reconnect

- After admission each side sends `ControlMessage::Addresses` (`address.rs`): its home relay URLs and direct sockets, never a credential. `remember_peer_addresses` stores them for that authenticated peer only, and the next dial uses them.
- A session that ends without a close frame is a `LinkEnd::Dropped`; only that redials (`run_controller`). It waits `reconnect::PAUSE` and allows `reconnect::ATTEMPTS` consecutive drops, and a link that stayed up resets the count. Only an admitted session retries, and only for errors `reconnect::retries` accepts. A local disconnect, a host close and unpair do not redial. After the attempts run out `presence::keep_link` starts a new session with its own backoff.
- Each admitted connection raises `LiveSession::generation`, and `register_incoming` stops the peer's older host session. Host services of the lost connection end as `CloseCause::ParentLoss` and release held keys. Handles, leases and pipes of the old generation fail with `Error::StaleSession`; queued input and clipboard pipes die with their Desktop runtime. Controller services go `Suspended` and return `Active` under the new generation (`services::synchronize`).
- Presence is observed, never inferred from cached addresses: `link::presence` and `presence::set_presence` publish `PresenceChanged` from session phases. `DevicePresence` is `Connected`, `Connecting`, `Reconnecting`, `NotAccepting` or `Disconnected`. Hello and Accept carry the hostname and `OperatingSystem`, stored only for paired peers.

## Desktop

- Video, input and clipboard are one service on the link: the host calls `Core::publish_desktop_video`, the viewer reads `RemoteDesktopService::subscribe_video`, and input goes the other way.
- `KeyboardMode` is `Character` (insert the typed character), `Map` (send the physical position) or `Translate` (ask the host to press the chord in its own layout). `keyboard::plan::KeyPlan::apply` chooses the output, and it reaches the input queue as `Outbound::{Event, Text, Chord}`. `ControlMessage::Text` and `Chord` are applied only if the stamp matches the Desktop. `HostDesktop::type_chord` refuses by default; only X11 `os_input::OsInput` implements it. `input::swap_command_key` exchanges Control and Command before a key is queued, and `input::MAX_KEY_TEXT` bounds finished text.
- `Core::set_keyboard_mode` and `Core::set_swap_control_and_command` set defaults for desktops opened later. A service copies them and refuses changes while it holds keys or buttons.
- Clipboard text is a separate grant. Each Desktop runtime owns a clipboard pipe (`core/desktop_clipboard.rs`) limited to `MAX_CLIPBOARD_TEXT` and a short timeout, with one text outstanding and one queued. The host re-checks the grant and the `HostClipboard` adapter for every text. `RemoteDesktopService::submit_clipboard` goes controller to host, `Core::publish_desktop_clipboard` goes host to controller, and `CoreEvent::DesktopClipboardReceived` names the Desktop without carrying the text. Nothing in this path touches the operating-system clipboard, and the engine bridge has no command that sends clipboard text.

## Files

- Files shares a whole disk or nothing: `Core::enable_filesystem_access` shares `/` or every drive, and each operation also needs `Grant::Transfer` (`SharingPolicy.files`). Withdrawing it closes hosted Files services and interrupts their jobs.
- Each peer request is fenced against the session generation and the grant under the same lock as the change (`serve_aux_host`, `dispatch_aux_host` in `core/fs.rs`), and `jobs::rename_blocked_with` refuses changes to entries an active job uses.
- A listing carries `fs::EntryToken`s. A token guards a change against a stale listing, not against the peer: sharing alone decides what a peer may change. On most volumes it is the object's identity. On volumes whose ids are entry positions (FAT, exFAT, some network shares) it is a fingerprint; a file may then be renamed or deleted while its fingerprint still matches, and anything without a proven fingerprint, including every folder delete, is refused as `Error::UnprovenEntry`.
- All movement goes through jobs. `Core::start_job_driver` runs one worker per drivable job; transient failures retry up to `MAX_RETRIES` within `RETRY_WINDOW` and then interrupt the job; `load_jobs` makes in-flight jobs `Interrupted` on start. Jobs persist as per-job journals (`job_journal`).
- A job is bound to a Files service and generation (`service_jobs::JobOwner`). A job whose service is gone is orphaned, and `Core::files_service_resume_orphans` adopts and resumes it once the host confirms and the rebinding is saved.
- Bytes move over the `vnilink/files/1` connection (`transfer::FILES_ALPN`), one stream per file, in `SEGMENT_LEN` BLAKE3 segments; a mismatched segment is fetched once more, then the job fails. A served file must match the scan's id, size and modification time (`jobs::open_served_file`), and `ServeGate` bounds concurrent serving. `transfer::window::Window` paces the receiver.

## Verification

- Unit tests sit beside their module. A service's behaviour across two engines belongs in its `vnilink-testkit` suite (`node build/run.ts test:service <service>`), its speed in the matching bench (`bench:service`).
- `npm run measure:latency` measures glass-to-glass latency, and `npm run measure:impaired` measures transfers and video over impaired links in a Linux container. Each documents what its evidence proves and does not cover at the top of its task (`build/lib/latency.ts`, `build/lib/impaired.ts`).
- A local macOS pass, a successful cross-compile or a CI configuration is not evidence that another platform works.
