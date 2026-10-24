# Glass-to-glass latency on one computer

`npm run measure:latency` measures how long a change on this computer's screen takes to appear in a VniLink viewer on the same screen. It runs on macOS, Windows and Linux. Use it to record numbers before and after a capture, encode, transport, decode or drawing change.

## How it works

Two temporary VniLink instances, each with its own identity and profile, pair and stream over loopback:

- The **host** hosts the screen from a temporary engine that may capture. It opens a stimulus window across the top of the main display. On every animation frame the stimulus draws a strip of black and white cells carrying that frame's time in ms.
- The **viewer** shows the host's screen in a real workbench window below the strip. Its picture contains a copy of the strip, older by the latency.
- The **reader** captures the display and decodes every strip in each picture (`vnilink_media::measure::glass`). On macOS it is `crates/vnilink-macos/examples/glass_latency.rs`, which captures with ScreenCaptureKit and times a picture by its `displayTime`. On Windows it is `crates/vnilink-windows/examples/glass_latency.rs`, which captures with Desktop Duplication and times a picture by its `LastPresentTime`, the QueryPerformanceCounter time the desktop was presented. On Linux it is `crates/vnilink-linux/examples/glass_latency.rs`, which captures through the ScreenCast portal and times a picture by when PipeWire handed it over. The live strip and its copy come from the same composited picture, so their difference is the latency, with no clocks to align. The display's scan-out and the reader's own capture delay are shared by both strips and cancel out.

The viewer shows the display it is on, so its picture holds smaller copies of itself. Those copies are older still, and the reader ignores them. The strip layout is in `vnilink_media::measure::timecode`; `test/latency/stimulus.ts` draws the same layout, and `test/tooling/latency.test.mjs` keeps the two in step.

## What it proves and what it does not

It proves the time from a picture on this screen to the same picture shown by a viewer, through the product's path:
- the platform's capture and encoder on the host (ScreenCaptureKit and VideoToolbox; Desktop Duplication and Media Foundation; the RemoteDesktop portal, PipeWire and VA-API or a software encoder);
- QUIC on loopback;
- the utility process, WebCodecs and the canvas in the viewer;
- the compositor.

It does not cover:
- **A network.** Loopback has none. Add the impaired-network harness's numbers for that, and say so.
- **Panel response.** Both strips share it.
- **Another computer or operating system.** Each run measures the one it ran on.
- **Independent hardware.** Host and viewer share one CPU, GPU and media engine, which inflates encode and decode times compared with two computers.

The result is quantised to the display's refresh period (16.7 ms at 60 Hz). Use percentiles over many samples, not single readings.

To check the reader, film the screen once with a 240 fps camera and compare a few readings by hand.

## Running it

```bash
npm run build
npm run measure:latency -- --seconds 20
```

### macOS

Both sides start through LaunchServices (`open`), so macOS asks Screen Recording of the development VniLink in this checkout (`.build/electron/VniLink.app`) and not of the terminal or tool that started the task. The reader runs as the host's child and uses the same permission. On the first run the host asks macOS and stops: allow VniLink in System Settings › Privacy & Security › Screen & System Audio Recording, then run again.

macOS grants an ad-hoc signed application by its code hash, so each checkout's development VniLink needs its own grant, and the list shows them all as "VniLink". To use a copy that already has it, such as the main checkout's, pass it with `--app`. That application only hosts Electron; the code it runs is this checkout's `out/`.

The host's sharing allows view and control, but its engine prepares video only, so the viewer cannot control this Mac. Leave the display awake and uncovered while it runs.

Only the host prepares native video on macOS; the viewer's window decodes with its own decoders. A viewer engine that also prepared native video, on the same Mac as the host, made the host's capture reads about 19 ms slower (133 against 116 ms p50 on an M1): a cost of measuring both sides on one computer, which a real viewer on another computer does not add. On Windows and Linux the viewer prepares it, as its engine admits a screen only with a decoder of its own.

### Windows

Run it in the signed-in desktop: Desktop Duplication fails in the session SSH opens (session 0). From SSH, start it through a temporary interactive scheduled task (`schtasks /Create ... /IT`), and delete the task afterwards. Windows asks no permission to capture. The sides start the development Electron in `.build/electron` directly; `--app` is macOS only. The reader is `target/x86_64-pc-windows-gnu/release/examples/glass_latency.exe`, built with the GNU toolchain `scripts/test-windows.ps1` sets up.

### Linux

Run it in the graphical session: the sides and the reader need its session bus and the desktop portal. From SSH, start it as a transient user unit, which runs in the user's session environment:

```bash
systemd-run --user --wait --collect --working-directory="$PWD" npm run measure:latency -- --seconds 20
```

The desktop portal asks someone at this computer twice, on the first run only: once for the host and once for the reader. Choose the display both times; the host is only viewed, so it needs no remote interaction. The host and the reader each wait up to five minutes for an answer. The reader's dialog can open behind the stimulus and the viewer, which stay above other windows; the Activities overview shows it. The portal keeps both answers until they are revoked, as the product's host keeps its own: their restore tokens are in `.build/latency/` (`host-screen-grant`, `reader-screen-grant`, readable by this user only), and later runs share the display without asking. Revoke them under Settings, Privacy, Screen Sharing, or delete the files, to be asked again. The macOS and Windows readers ignore the grant file: those systems ask nothing per run. The sides run under XWayland (`--ozone-platform=x11`), because a Wayland window cannot place itself: the stimulus must sit at the top of the display and the viewer below it. Both stay above other windows.

Turn on Do Not Disturb first. GNOME announces a window that opens without focus with a banner ("VniLink is ready") across the top of the display, over the strip, and no picture can then be read.

The reader times a picture by when PipeWire delivered it, not by when the compositor presented it. Both strips share that delay, so the latency is unaffected, but hold mode's showing times are a little late.

### Evidence

Evidence lands in `.build/latency/<time>/`:
- `summary.md`: glass-to-glass percentiles and the viewer's own stage times;
- `samples.jsonl`: one line per captured picture, with every strip found and the latency when both were read;
- `stages.jsonl`: the viewer's stage times, once a second;
- `host-start.log`, `viewer-start.log`: what each side saw while the stream started, for a run that fails there;
- `host.log`, `viewer.log`: each side's output;
- `host-pipeline.json`: the viewer's pipeline as the host's diagnostics hold it at the end of the run: the encoder, whether it is hardware and what it read pictures from (also in `summary.md`), keyframes, encoder reopens and drops;
- `run.json`: the options and machine;
- hold mode only: `schedule.json`, when the stimulus moves and holds, and `stills.jsonl`, the still pictures the viewer had shown, sampled every 100 ms.

The reader prints the p50, p95, p99 and maximum when it finishes.

| Option | Default | Meaning |
|---|---|---|
| `--seconds` | `20` | How long the reader samples. |
| `--settle` | `3` | Seconds between the first picture and sampling. |
| `--output` | `.build/latency/<time>` | Where the evidence goes. |
| `--app` | this checkout's development VniLink | macOS: the `.app` that runs both sides. |
| `--hold` | `0` | Hold mode: seconds the stimulus holds still after each 2 s of motion; `0` is off. |
| `--fps` | the default quality | The viewer's frame rate, at the Smooth preset's 12 Mb/s and 1920x1080. |
| `--remote` | | Two computers: the other one's SSH destination. |
| `--remote-root` | | With `--remote`: the absolute path of a built checkout there. |
| `--hosts` | `here` | With `--remote`: which computer hosts its screen, `here` or `remote`. |
| `--ssh-config` | the user's | With `--remote`: an SSH configuration file for every connection. |

### Two computers

`--remote <destination> --remote-root <path>` runs one side here and the other on a Linux computer over SSH, the pair a person uses, so each side has its own CPU, GPU and media engine and the stream crosses a real network. The viewer picks the codec it would pick in use: a Linux window offers no HEVC, so a Mac host sends it H.264.

No screen shows both strips, so there is no glass-to-glass total and no reader. Each side reports itself once a second instead:
- the host's capture and encode times (`PipelineSummary.capture_us`, `encode_us`; encode includes scaling and copies), its encoder, keyframes, reopens and drops, in `host-stages.jsonl`;
- the viewer's stage times and status line (frame rate, codec, bitrate, round trip) in `stages.jsonl`, and its pipeline (round trip, decoder, conditions) in `viewer-pipeline.jsonl`.

The network leg is the round trip: the clocks differ, so a picture's one-way time is not read. The stimulus still runs on the host: a strip that changes every frame, the motion the stream carries.

The remote side runs as a transient unit of the user's session there (`systemd-run --user`), so it has the graphical session's bus and portal. Its checkout must be built (`npm run build` there). The two sides' rendezvous files are copied over one shared SSH connection, and the remote side's evidence is copied back into the output and removed there, with its profile and rendezvous. A Linux host's portal asks someone there on the first run, as on one computer; its answer is kept in that checkout's `.build/latency/`.

```bash
npm run measure:latency -- --remote lx --remote-root /home/me/Projects/vnilink --hosts here --fps 60 --seconds 30
```

The evidence is `summary.md`, `run.json`, both sides' logs and the files above. Scaling and copies are inside the host's encode time; `encode_probe` in `vnilink-macos` (GPU scaling of a 4K picture) and `conversion_bench` in `vnilink-media` (CPU scaling and conversion) time them alone.

### Hold mode

`--hold <seconds>` measures still pictures: when one shows after the screen stops changing, and whether the first change after it is held up. The summary adds both.

For the host to see a still screen, nothing on the captured display may change during a hold:
- The stimulus fills the display, its strip above a panel of coloured text (what a still is for: the host sends one only when it beats 4:2:0). It covers the menu bar and the Dock above floating windows, since an animated desktop picture behind the Dock is a change.
- The viewer covers the lower half. It is hidden while the stimulus holds: every picture it draws is a change on the captured screen, and ScreenCaptureKit captures a content-protected window regardless. Hidden, it still receives and decodes, and counts the stills it shows.
- The viewer's status bar, which changes every second, is hidden for the run.
- The viewer runs at the Smooth preset (12 Mb/s), whose budget a 1080p still of this panel fits.

What the measurement cannot cover is still a change: another application's window above everything, such as a video call's floating window, keeps the screen from holding. `examples/sck_changes` in `vnilink-macos` shows which application changes the display. On Windows hold mode runs as on macOS but is unmeasured.

`build/lib/latency.ts` owns the task; `test/latency/main.ts` is the host and viewer.
