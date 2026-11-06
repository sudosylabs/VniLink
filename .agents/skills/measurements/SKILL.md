---
name: measurements
description: "Measure VniLink's speed, latency or resource cost before and after a change, with bench:service, measure:latency, measure:impaired and test:stress. Use when a change claims to be faster, smoother or cheaper, when investigating a slow, laggy or CPU-heavy path, or when a measurement run fails or looks wrong."
---

# Measurements

A speed change is kept when the same measurement, run before and after on the same computer, shows the gain and nothing else got worse. Pick the tool that exercises the path the change touches; each documents itself at the top of its task file, which is the authority for options and output.

## The tools

| Task | Measures | Evidence | Proves |
|---|---|---|---|
| `node build/run.ts bench:service <service>` | A service's throughput and latency in the test kit (`crates/vnilink-testkit/benches/<service>.rs`), both engines on this computer | `.build/bench/<service>/<time>.json`, compared with the previous run; a regression fails the task | The engine's own path on loopback and simulated round trips (Files: small and large files, loopback and `rtt40`) |
| `node build/run.ts measure:latency` | Glass-to-glass latency through the real app: capture, encode, transport, decode, drawing (`build/lib/measure/latency.ts`) | `.build/latency/<time>/summary.md`, `samples.jsonl`, `host-pipeline.json` | That computer's capture and encoder; one machine runs both sides, so encode and decode share a GPU. `--hold` measures still pictures |
| `node build/run.ts measure:impaired` | The engines over a Linux `tc netem` link with loss, delay and rate limits, in the project container (`build/lib/measure/impaired.ts`) | `.build/impaired-network/<time>/summary.md`, `results.json`, one JSON per profile and scenario | Transport, jobs, keyframe recovery, adaptation and stills under impairment, with a synthetic source and encoder: nothing about a real capture or GPU |
| `node build/run.ts test:stress <service> --times N --load` | A suite run again and again with every core busy | The first failure | That an order the engines do not guarantee does not break a story; use before keeping a new or changed story |

`measure:impaired` takes `--scenarios` (`transfer`, `video`, `combined`, `combined-download`, `stills`) and `--profiles` (`clean`, `delay-50`, `delay-100`, `loss-1`, `loss-burst`, `rate-10`, …, or `quick`); run only what the change touches. `--peer` adds a second computer to `bench:service` and `measure:latency` through the test agent; it is never required.

## Running one

1. Record the baseline at the commit before the change, then the change, with the same options. Name both result folders in the report.
2. Run one measurement at a time: they are load-sensitive, and another build or suite running beside them moves the numbers.
3. `measure:latency` refuses a native addon older than its Rust sources (`assertNativeFresh`): run `npm run build` after a Rust change.
4. `measure:impaired` builds and runs in Docker against the checkout as it is; leave the code alone while it runs, or the run measures a mixture.
5. Read the distribution, not one number: p50 and p95, and the maximum for anything a person waits on. A case that moves by itself between two runs of the same code is noise; run it again alone before believing it.

## Known traps

- The impaired stills scenario holds the screen still for 2.25 s and asks for a still at about 2.2 s. An AV1 encode that takes longer than the remaining window is thrown away as stale, so the scenario reports no stills on a slow machine whatever the change does; lengthen `STILL_PERIOD` for both runs, without committing it, when stills are what is measured.
- `measure:latency`'s viewer only watches. A path that runs only while controlling, such as selecting a display, never runs there.
- The container's cache volumes are named by the checkout's path (`build/lib/containers.ts`). A deleted checkout leaves its volumes behind; `docker system df -v` shows them.
- A debug build measures the debug build. Streaming and encoding numbers come from release builds; label anything else.

## Reporting

Give the tool and options, the computer (hardware class and OS), the before and after folders, a table of the cases that matter, and what the run cannot show. A change kept without a measured gain says why. Platform claims follow `cross-platform-validation`.
