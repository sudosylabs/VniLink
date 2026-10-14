---
name: cpu-profile-analysis
description: "Investigate and measure Electron/Rust CPU or latency costs with reproducible workloads, pipeline attribution and comparable before/after evidence."
---

# CPU and Latency Analysis

Start from a reported symptom or explicit performance goal. Establish a baseline before optimization; a hypothesis can motivate measurement but cannot establish a hotspot.

Record the workload, OS/architecture, native backend/codec, frame dimensions/rate, build profile, network path and warmup. Use representative optimized Rust builds for streaming performance; keep debug results labeled. Capture V8 profiles/traces and Rust/native timing or profiles at the suspected owner.

Trace capture, conversion, encode, transport, bridge buffers, decode and render separately. Attribute self-time and queue/latency costs across repeated captures. Inspect actual control versus binary channel use; binary channels already exist in this repository, so do not assume video is a deferred base64-only path.

Change the measured cost while retaining authorization, exact-shape validation, generation fences and ownership. Faster polling can multiply crossings and copying without solving stale state; inspect event flow/backpressure first. Zero-copy and hardware claims require actual buffer/backend evidence.

Re-measure the same workload/environment and report distribution (including relevant tail latency), throughput and resource tradeoffs. Do not use one synthetic sample to claim all codecs or platforms improved. Choose a useful acceptance budget before judging the result; retain changes for evidenced benefit or another explicitly stated requirement, not "should be faster".

Use maintained benchmarks/diagnostic scenarios where available; integrate repeated investigations into reproducible tooling (`build-test-infrastructure`). Platform-specific results follow `cross-platform-validation`.
