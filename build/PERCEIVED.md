# Perceived quality of a remote desktop application

`npm run measure:perceived` measures what a person at a Mac sees: how the picture is scaled, how fast and how accurately the pointer follows, how long a key or a click takes to show, how smoothly the picture moves and how sharp it is.

It runs on one Mac by default. The page opens on that Mac and the run measures the Mac's own floor: synthetic input, the browser, the display and capture. Every remote desktop application adds to that floor, so it is the baseline its runs are compared with.

With `--remote`, an optional second computer, it measures a remote desktop application controlling a Linux computer from the Mac. The application is a black box, so VniLink and other applications such as RustDesk are measured the same way and compared side by side, and with the one-Mac baseline.

## How it works

The methods are established ones, adopted rather than invented: on-screen timecodes read back from the viewer's screen (as vDelay and the latency harness in `LATENCY.md` do), synthetic input timed against its effect on screen (as AMD's Frame Latency Meter does), and SSIM through FFmpeg for sharpness (as FFmpeg's own quality filters are used).

- The **stimulus** (`test/perceived/stimulus.ts`) runs maximised in a temporary Electron window: on the Mac by default, on the controlled computer with `--remote`. It is not full screen: on GNOME a full-screen window stops RustDesk's capture after one picture. On every animation frame it draws strips of black and white cells: two anchors at known corners and a column carrying the frame counter, click count, pointer position, test card generation and the page's size. A moving band keeps the encoder busy, and a deterministic test card of lines, checks, text-like strokes and colours swaps between two variants every 8 s. The page hides its own pointer; the strips report where the pointer is on the computer showing it.
- The **viewer driver** (`crates/vnilink-macos/examples/perceived.rs`) captures the Mac's display with ScreenCaptureKit in sRGB. It finds the anchors, which give the scale and placement of the page's picture (`vnilink_media::measure::perceived`; 1:1 when the page runs on the Mac), then reads only the column on each frame. It locks on only once the picture has stayed put for a second, and if the column stops reading in place for half a second, because the window moved, went full screen or was covered, it locks on again and restarts the measurement; only the last, complete measurement counts, and the summary says how often it restarted. It moves the pointer with CGEvent: 24 jumps across a grid, a 6 s circle at 120 Hz, then 20 clicks; then it types keys of this Mac's current keyboard layout, plain, with Shift and with Option, with Backspace and Enter, one every 250 ms and then a burst of 20 one every 30 ms, pressing the keys a person typing here would (`UCKeyTranslate` says what each types); then watches 20 s of pacing and crops the card at fixed times after each swap. Last, a right click freezes the page, and for 10 s the card is cropped from every picture the screen shows: an application may sharpen or replace its picture of a still screen, as VniLink does with a 4:4:4 still.
- The **analysis** (`build/lib/perceived.ts`) turns the driver's `events.jsonl` into the summary: pointer jump latency and error, how far behind the circle the pointer trails and whether that grows over the circle, key-to-picture time and whether every key arrived once with the text as typed (the page shows its key count and a hash of its text), click-to-photon time, frames per second, frame intervals and stutters over 50 ms, the share of host frames shown, the card's SSIM against a reference drawn from the same rectangles, scaled to the size shown, and the still card's SSIM from the first picture after the freeze to the last, with when it last changed. It writes `summary.json` and `summary.md`.

`test/tooling/perceived.test.mjs` keeps the stimulus's layout and the Rust reader in step.

## What it proves and what it does not

On one Mac, it proves the floor of the measurement on that Mac: what synthetic input, Chromium, the compositor, the display and capture add before any application is involved. It added about 70 ms to pointer and click times when first measured. A Mac's floor changes with its display, refresh rate and load, so measure it on the Mac the comparisons run on.

With `--remote`, it proves what reached the Mac's display through the whole path of the application under test: the controlled computer's capture and encoder, the network between the two computers, the viewer's decoder and drawing, and the Mac's compositor. Input latency includes the application's input path and the controlled computer's input handling. Each such run contains the floor, so compare applications with each other and with the floor, not with zero.

It does not cover:
- **Panel response and scan-out.** A photodiode tester such as OSLTT or LDAT would give that ground truth.
- **Other viewers or hosts.** The driver is macOS-only, and with `--remote` the stimulus starts on Linux under X11 or XWayland.
- **The network's variation.** A `--remote` run measures the link as it was; run on the same link for a comparison.

Pacing and latency are quantised to the display's refresh period (16.7 ms at 60 Hz). Use the percentiles, not single readings.

## Running it

The driver runs under the application that starts the task, a terminal or an editor. Allow that application Screen & System Audio Recording and Accessibility in System Settings › Privacy & Security; `npm run doctor` says whether it has both.

### On one Mac

```bash
npm run build
npm run measure:perceived
```

The task opens the page maximised on this Mac, above other windows. Measuring starts when the page is found (`--wait`, 300 s by default) and takes about a minute: the pointer moves and keys are typed by themselves, so leave the mouse and keyboard alone. Evidence goes to `.build/perceived/This-Mac-<time>` unless `--output` names another directory; `--label` names the run. `npm run measure:perceived -- --help` lists the options.

### With a second computer (optional)

Every remote desktop application is measured the same way: the page runs on a Linux computer, and the application shows that computer on this Mac. Build that computer's checkout once, so `out/perceived.cjs` and Electron exist there:

```bash
npm run build
```

On the Mac, with SSH access to that computer:

```bash
npm run measure:perceived -- --remote <ssh destination> --remote-root <checkout on that computer> --label VniLink
```

The task starts the page there, then asks you to open the application under test to that computer on this Mac, full screen. Measuring then runs as on one Mac. `--ssh-config` selects an SSH configuration file. Repeat with `--label RustDesk` or another application.

A later change replaces SSH with a test agent that pairs like the application; until then, this path needs SSH.

### Comparing runs

```bash
npm run measure:perceived -- --compare <directory> <directory>
```

The summaries are shown side by side, one column each: an application's runs and this Mac's baseline.
