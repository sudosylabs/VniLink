# Fuzzing peer input

These cargo-fuzz targets feed the parsers of bytes a paired peer sends
with arbitrary input, and check what each parser promises of it. The
parsers are private to `vnilink`; the `fuzzing` feature exposes thin entry
points in `vnilink::fuzzing` for this crate only. Production builds leave it
off.

| Target | Parser | What it checks |
|---|---|---|
| `control_message` | `ControlMessage::decode` | A decoded message encodes to a frame that decodes to the same message. |
| `file_request` | `FileRequest::decode` (`vnilink/files/1`) | Exactly 20 bytes decode, back to themselves. |
| `transfer_receive` | the receiver's stream parser (`transfer/receive.rs`) | An honest stream lands as the file however it is chunked; an edited one never lands. |
| `video_reassemble` | `Reassembler::push`, then `decode_video_body` | Forged and honest fragments never panic; an honest frame comes back exactly when at least its data count of shards arrived. |
| `video_report` | `VideoReport::decode` and the host's smoothing | Reports decode back to their bytes; the parity share stays within its bounds. |
| `host_feedback` | `HostFeedback::decode` and the viewer's adaptation loop | Feedback decodes back to its bytes; no feedback moves quality outside the agreed limits. |

## Running

The crate is not a member of the repository's workspace: cargo-fuzz builds
it with nightly Rust and sanitizer flags. The repository's pinned toolchain
is unchanged; install the tools once:

```bash
rustup toolchain install nightly --profile minimal
cargo +nightly install cargo-fuzz --locked
```

From `crates/vnilink`, run one target for ten minutes:

```bash
cargo +nightly fuzz run transfer_receive -- -max_total_time=600
```

`cargo +nightly fuzz list` names the targets. The corpus and any crash
land in `fuzz/corpus/<target>` and `fuzz/artifacts/<target>`, both
ignored. Replay a crash with `cargo +nightly fuzz run <target> <file>`. Fix
the parser and keep the input as a unit regression test beside it; the
artifact itself is not tracked.

The `[patch.crates-io]` table repeats the workspace's vendored sources;
change both together.
