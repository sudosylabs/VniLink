# Impaired-network harness

The harness measures file transfer and desktop video between two VniLink engines under controlled loss, delay and rate limits. Use it to record numbers before and after a transfer or video change.

## What it proves and what it does not

It runs in a Linux network namespace pair with `tc netem`. Its results show how the Rust engines behave over an impaired link on Linux: the transport, the job engine, the datagram video path, keyframe requests and adaptation.

It does not show macOS or Windows behaviour. The video source and encoder are synthetic, so it says nothing about ScreenCaptureKit, VideoToolbox, Media Foundation, the renderer or WebCodecs. Docker Desktop runs the container in a virtual machine, so absolute throughput reflects that machine and CPU contention between the two engines. Compare runs made on the same machine; don't compare a run with a number from somewhere else.

## Running it

From macOS or any Docker host:

```bash
npm run measure:impaired -- --profiles quick
```

The `measure:impaired` task builds the project image (`build/containers/Dockerfile.linux`), copies the checkout into a privileged container and runs the `measure:impaired:linux` task there. Evidence lands in `.build/impaired-network/<time>/`:

- `summary.md`: one table per scenario.
- `results.json`: options, profiles, every scenario result, and veth and qdisc counters for each run.
- `environment.txt`: kernel, toolchain, `tc` version, CPU count and memory.
- `<profile>/<scenario>.json`: the raw scenario output.

The release test build is cached in a `vnilink-impaired-release-target-<checkout>` volume, and the Node dependencies the task runner loads in `vnilink-impaired-node-modules-<checkout>`, one of each per checkout, so worktrees never run each other's binaries. A run refuses to start while another is in progress: two runs on one machine compete for its CPUs and skew each other's numbers. The first build is slow; set `CARGO_BUILD_JOBS` on the host to raise the image's single-job default.

On a Linux machine, run the inner task as root: `sudo node build/run.ts measure:impaired:linux --profiles loss-1`. `build/lib/impaired.ts` owns both tasks.

| Option | Default | Meaning |
|---|---|---|
| `--profiles` | `all` | Comma-separated names, `quick` or `all`. |
| `--scenarios` | `transfer,video` | Which scenarios run under each profile: `transfer`, `video` or `combined`. |
| `--seed` | `17` | netem loss seed. The same seed drops the same packet positions. |
| `--small-files`, `--small-size` | `1000`, `4096` | The many-small-files case. |
| `--large-size` | `1GiB` | The large-file case, e.g. `256MiB` for a quicker run or `10GiB`. The combined scenario uploads a file this size. |
| `--transfer-timeout` | `120` | Seconds per case, from submit. At the deadline the job is cancelled on both sides and reported with the bytes and files done so far. |
| `--driver` | `tight` | `tight` advances the receiver back to back. `background` uses only the app's background job driver. |
| `--video-seconds`, `--bursts`, `--burst-ms` | `30`, `5`, `250` | Stream length and the blackouts inside it. |
| `--viewer` | `passive` | `passive` is the product viewer. `request-while-corrupt` asks for a keyframe on every corrupt picture. |
| `--path` | `direct` | `direct` lets the cores reach each other only directly. `relay` makes every byte cross a local iroh relay; see [Relay mode](#relay-mode). |

A case the engine refuses, such as one past its free-space admission, is recorded as `refused` with the engine's error. It is not a harness failure.

## Profiles

Every impairment applies to both directions. Delays are round trips split evenly between the directions, and every impaired profile keeps at least a 20 ms round trip.

| Profile | Impairment |
|---|---|
| `clean` | None. |
| `loss-0.5`, `loss-1`, `loss-2`, `loss-5` | Random loss at that percentage. |
| `loss-burst` | Gilbert–Elliott loss: about 2% overall in runs of about 4 packets. |
| `delay-20`, `delay-50`, `delay-100` | That round trip, with jitter up to a fifth of it. |
| `rate-2`, `rate-5`, `rate-10` | That many Mb/s each way, with about 250 ms of queue. |

Jitter uses netem slots, which hold and release packets in bursts. netem's own jitter would reorder packets, and QUIC would treat the reordering as loss. `quick` is `clean`, `loss-1`, `loss-5`, `loss-burst`, `delay-50` and `rate-5`. `build/lib/netem.ts` owns the catalog.

## How the link works

`build/lib/netem.ts` creates namespaces `vnl-host` and `vnl-view`, joined by one veth pair with a netem qdisc at each end.

The engines send UDP GSO batches. Without help, netem would see each batch as one packet of up to 64 KiB, and drop or pace the whole batch. A 10 Gb/s token-bucket root, which never shapes at these rates, has a 2000-byte burst. It splits every batch into wire-sized segments before they reach netem, so loss, rate and queue limits apply per packet. The veth counters in `results.json` may still count batches. The netem counters count segments. The test cores bind `127.0.0.1` in direct mode. Each namespace keeps that address on loopback, and a policy rule sends locally sent traffic for `127.0.0.1` out through the veth. Received packets still match the local table. As a result, the unchanged loopback fixtures reach each other only through the impaired link. The veth counters in `results.json` show that the traffic crossed it.

Each scenario boots each core on a thread that first enters its namespace with `setns`. The core's runtime, its worker threads and its endpoint socket are therefore created inside that namespace.

## Relay mode

`--path relay` measures the relay fallback instead of the direct path:

```bash
npm run measure:impaired -- --profiles quick --path relay
```

- **Relay.** The harness builds `crates/vnilink/examples/impaired_relay.rs`, an `iroh-relay` 1.3.0 server serving plain HTTP with no QUIC address discovery and no rate limits. It runs inside `vnl-host` on `127.0.0.2:3340`. Both cores use it as their only custom relay through `VNILINK_IMPAIRED_RELAY_URL`; in direct mode they keep a relay on `127.0.0.1:9`, where nothing listens.
- **Routes.** The host core reaches the relay over its own loopback. The viewer's core reaches it from `127.0.0.3` across the veth, so each relayed packet crosses netem once, as a direct packet does. Only the viewer's leg is impaired; the relay's TCP connection, not QUIC, carries that leg's loss recovery.
- **No direct path.** A `clsact` egress filter on each veth drops every UDP packet before it reaches the netem qdisc. The `udpBlocked` counter in `results.json` counts what it kept off the link.
- **Verification.** Each scenario reports the selected path of every connection, from the cores' diagnostics, once connected and once finished. A run fails unless every core finishes on a connection over the expected path, in either mode. The Files connection is not in those diagnostics; with UDP blocked it can only use the relay too.
- **Relay evidence.** The relay prints its cumulative counters once a second. Each run's `relay` entry in `results.json`, and the summary's Relay table, give their difference over the run:
  - datagram frames taken from and written to clients;
  - frames dropped for a gone recipient or a failed write;
  - frames its per-client queues discarded. The server does not count these, so the harness derives them, give or take one queue still waiting at the end;
  - client connections opened and closed. A client reconnects after a relay health-check ping times out, and the server closes one whose writes outlast its timeout.
- **Relay sockets.** During each run, the harness samples every TCP connection to or from the relay with `ss -tin` once a second. `<profile>/<scenario>.relay-sockets.jsonl` holds each sample's Send-Q (bytes not yet acknowledged), TCP round trip, congestion window and retransmissions. The summary gives each leg's peak Send-Q and round trip. A backlog queued inside the clients, ahead of their sockets, does not show in Send-Q. It shows as the QUIC round trip in the transfer scenario's `files_paths`.

This is a local relay on the same Linux host as both engines, over netem. It says nothing about the public n0 relays, their load or distance, TLS, or internet paths.

## Scenarios

**Transfer** (`crates/vnilink/src/tests/impaired_transfer.rs`) uploads three cases from the viewer's core to the host's core through the job engine:

- 1,000 files of 4 KiB;
- one large file;
- a mixed tree of 200 files from 1 KiB to 4 MiB, up to four folders deep, with empty folders.

Each case reports outcome, submit time, elapsed time from submit to completion, MB/s (10⁶ bytes) and files per second. A complete case is compared byte for byte with its source. The report also gives each core's selected path once connected and once finished.

`files_paths` samples each core's Files connections once a second during a case: the receiver's progress, and for every QUIC path its round trip, congestion window, congestion events, packets lost, and datagrams and bytes sent. The sender is the viewer core (`serving`). A path that disappears between samples is a connection that ended and was dialled again.

**Video** (`crates/vnilink-media/src/tests/impaired_video.rs`) runs the real shared-capture fanout with a synthetic source and encoder:

- Encoded sizes follow the encoder's bitrate and frame rate. Keyframes are ten predicted frames in size, and a periodic keyframe comes every 120 frames. A recovery keyframe, which answers a loss, is four frames in size: a model of the cap the platform encoders apply (`vnilink-media` `RecoveryQp`), not a real encoder's output.
- Each picture carries its sequence number, so the viewer can tell a clean picture from one decoded against a missing reference.
- The link is blacked out for each burst.

The report includes:

- time to picture after each burst: from the end of the blackout until a picture made after it shows clean;
- keyframes by cause (encoder open, forced by a request, periodic);
- delivery and clean ratios;
- viewer keyframe requests;
- one-way latency percentiles, measured on one clock in the same process;
- each core's selected path once connected and once finished.

**Combined** (`impaired_desktop_stream_during_upload`, beside the video scenario) runs the video scenario while the viewer's core uploads one file of `--large-size` to the host's core. The viewer opens a Files service beside its desktop service on the same session, as the app does, and the upload crosses the Files connection. In relay mode the stream, the Files connection and the session's control all share each core's one relay connection.

- The upload starts with the stream and runs until it completes or the stream ends, when it is cancelled. Its outcome, the bytes it moved and the time it took are reported with the video results, with `files_paths` as in the transfer scenario.
- `longest_freeze_ms` is the longest time without a new clean picture after the first one, whatever caused it. `session_interruptions` counts how often the viewer's session left the active phase, sampled every 100 ms with the timeline. The video scenario reports both too.
- The Relay table's client connections opened and closed show the relay clients reconnecting.

Every scenario runs as an ignored test through the harness. A small version of each runs in ordinary `cargo test` over plain loopback so the scenario code stays working.

## Baseline tests

`crates/vnilink/src/tests/keyframe_recovery.rs` pins the current keyframe-request behaviour of the reassembler and video queue under seeded fragment loss. It needs no network. Comments marked "Planned" show what the keyframe-discipline change is expected to tighten.
