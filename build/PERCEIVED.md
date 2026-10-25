# Perceived quality of a remote desktop application

`npm run measure:perceived` measures what a person at the viewing Mac sees when controlling a Linux computer through a remote desktop application: how the picture is scaled, how fast and how accurately the pointer follows, how long a click takes to show, how smoothly the picture moves and how sharp it is. The application is a black box, so VniLink and other applications such as RustDesk are measured the same way and compared side by side.

## How it works

The methods are established ones, adopted rather than invented: on-screen timecodes read back from the viewer's screen (as vDelay and the latency harness in `LATENCY.md` do), synthetic input timed against its effect on screen (as AMD's Frame Latency Meter does), and SSIM through FFmpeg for sharpness (as FFmpeg's own quality filters are used).

- The **stimulus** (`test/perceived/stimulus.ts`) runs maximised on the controlled computer in a temporary Electron window. It is not full screen: on GNOME a full-screen window stops RustDesk's capture after one picture. On every animation frame it draws strips of black and white cells: two anchors at known corners and a column carrying the frame counter, click count, pointer position, test card generation and the page's size. A moving band keeps the encoder busy, and a deterministic test card of lines, checks, text-like strokes and colours swaps between two variants every 8 s. The page hides its own pointer; the strips report where the host's pointer is.
- The **viewer driver** (`crates/vnilink-macos/examples/perceived.rs`) captures the Mac's display with ScreenCaptureKit in sRGB. It finds the anchors, which give the scale and placement of the remote picture (`vnilink_media::measure::perceived`), then reads only the column on each frame. It locks on only once the picture has stayed put for a second, and if the column stops reading in place for half a second, because the window moved, went full screen or was covered, it locks on again and restarts the measurement; only the last, complete measurement counts, and the summary says how often it restarted. It moves the pointer with CGEvent: 24 jumps across a grid, a 3 s circle at 120 Hz, then 20 clicks, then watches 20 s of pacing and crops the card at fixed times after each swap. Last, a right click freezes the page, and for 10 s the card is cropped from every picture the screen shows: an application may sharpen or replace its picture of a still screen, as VniLink does with a 4:4:4 still.
- The **analysis** (`build/lib/perceived.ts`) turns the driver's `events.jsonl` into the summary: pointer jump latency and error, how far behind the circle the pointer trails, click-to-photon time, frames per second, frame intervals and stutters over 50 ms, the share of host frames shown, the card's SSIM against a reference drawn from the same rectangles, scaled to the size shown, and the still card's SSIM from the first picture after the freeze to the last, with when it last changed. It writes `summary.json` and `summary.md`.

`test/tooling/perceived.test.mjs` keeps the stimulus's layout and the Rust reader in step.

## What it proves and what it does not

It proves what reached the viewing Mac's display through the whole path of the application under test: the controlled computer's capture and encoder, the network between the two computers, the viewer's decoder and drawing, and the Mac's compositor. Input latency includes the application's input path and the controlled computer's input handling.

It does not cover:
- **The Mac's own floor.** Synthetic input, Chromium, the display and capture add about 70 ms to pointer and click times when the page runs on the Mac itself. Compare applications with each other, not with zero.
- **Panel response and scan-out.** A photodiode tester such as OSLTT or LDAT would give that ground truth.
- **Other viewers or hosts.** The driver is macOS-only and the stimulus starts on Linux under X11 or XWayland.
- **The network's variation.** Each run measures the link as it was; run on the same link for a comparison.

Pacing and latency are quantised to the display's refresh period (16.7 ms at 60 Hz). Use the percentiles, not single readings.

## Running it

On the controlled Linux computer, build the checkout once so `out/perceived.cjs` and Electron exist:

```bash
npm run build
```

On the Mac, with SSH access to that computer:

```bash
npm run measure:perceived -- --remote <ssh destination> --remote-root <checkout on that computer> --label VniLink
```

The task starts the stimulus there, then asks you to open the application under test to that computer, full screen. Measuring starts when the page is found (`--wait`, 300 s by default) and takes about a minute; leave the mouse alone meanwhile. Evidence goes to `.build/perceived/<label>-<time>` unless `--output` names another directory; `--ssh-config` selects an SSH configuration file.

Repeat with `--label RustDesk` or another application, then compare the runs:

```bash
npm run measure:perceived -- --compare <directory> <directory>
```
