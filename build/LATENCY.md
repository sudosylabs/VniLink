# Glass-to-glass latency on one Mac

`npm run measure:latency` measures how long a change on this Mac's screen takes to appear in a VniLink viewer on the same screen. Use it to record numbers before and after a capture, encode, transport, decode or drawing change.

## How it works

Two temporary VniLink instances, each with its own identity and profile, pair and stream over loopback:

- The **host** hosts the screen from a temporary engine that may capture. It opens a stimulus window across the top of the main display. On every animation frame the stimulus draws a strip of black and white cells carrying that frame's time in ms.
- The **viewer** shows the host's screen in a real workbench window below the strip. Its picture contains a copy of the strip, older by the latency.
- The **reader** (`crates/vnilink-macos/examples/glass_latency.rs`) captures the display with ScreenCaptureKit and decodes every strip in each picture. The live strip and its copy come from the same composited picture, so their difference is the latency, with no clocks to align. The display's scan-out and the reader's own capture delay are shared by both strips and cancel out.

The viewer shows the display it is on, so its picture holds smaller copies of itself. Those copies are older still, and the reader ignores them. The strip layout is in `vnilink_media::measure::timecode`; `test/latency/stimulus.ts` draws the same layout, and `test/tooling/latency.test.mjs` keeps the two in step.

## What it proves and what it does not

It proves the time from a picture on this screen to the same picture shown by a viewer, through the product's path:
- ScreenCaptureKit and VideoToolbox on the host;
- QUIC on loopback;
- the utility process, WebCodecs and the canvas in the viewer;
- the compositor.

It does not cover:
- **A network.** Loopback has none. Add the impaired-network harness's numbers for that, and say so.
- **Panel response.** Both strips share it.
- **Another computer or operating system.**
- **Independent hardware.** Host and viewer share one CPU, GPU and media engine, which inflates encode and decode times compared with two computers.

The result is quantised to the display's refresh period (16.7 ms at 60 Hz). Use percentiles over many samples, not single readings.

To check the reader, film the screen once with a 240 fps camera and compare a few readings by hand.

## Running it

```bash
npm run build
npm run measure:latency -- --seconds 20
```

Both sides start through LaunchServices (`open`), so macOS asks Screen Recording of the development VniLink in this checkout (`.build/electron/VniLink.app`) and not of the terminal or tool that started the task. The reader runs as the host's child and uses the same permission. On the first run the host asks macOS and stops: allow VniLink in System Settings › Privacy & Security › Screen & System Audio Recording, then run again.

macOS grants an ad-hoc signed application by its code hash, so each checkout's development VniLink needs its own grant, and the list shows them all as "VniLink". To use a copy that already has it, such as the main checkout's, pass it with `--app`. That application only hosts Electron; the code it runs is this checkout's `out/`.

The host's sharing allows view and control, but its engine prepares video only, so the viewer cannot control this Mac. Leave the display awake and uncovered while it runs.

Evidence lands in `.build/latency/<time>/`:
- `summary.md`: glass-to-glass percentiles and the viewer's own stage times;
- `samples.jsonl`: one line per captured picture, with every strip found and the latency when both were read;
- `stages.jsonl`: the viewer's stage times, once a second;
- `host-start.log`, `viewer-start.log`: what each side saw while the stream started, for a run that fails there;
- `host.log`, `viewer.log`: each side's output;
- `run.json`: the options and machine.

The reader prints the p50, p95, p99 and maximum when it finishes.

| Option | Default | Meaning |
|---|---|---|
| `--seconds` | `20` | How long the reader samples. |
| `--settle` | `3` | Seconds between the first picture and sampling. |
| `--output` | `.build/latency/<time>` | Where the evidence goes. |
| `--app` | this checkout's development VniLink | The `.app` that runs both sides. |

`build/lib/latency.ts` owns the task; `test/latency/main.ts` is the host and viewer.
