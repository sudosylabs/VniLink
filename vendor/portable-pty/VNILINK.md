# Local patch to portable-pty 0.9.0

Source: crates.io portable-pty 0.9.0 (wezterm), MIT license (LICENSE.md). Original crate SHA-256: `b4a596a2b3d2752d94f51fac2d4a96737b8705dddd311a32b9af47211f08671e`.

Before `exec`, `close_random_fds` in `src/unix.rs` closed every inherited descriptor above 2 with `close(2)`. A terminal shell is spawned from the engine, which runs in Electron's utility process. On Linux, Chromium replaces `close(2)` in its processes with one that aborts on a descriptor Chromium owns, and the forked child inherits it. Every shell then died with "Crashing due to FD ownership violation" before it ran; macOS Chromium does not replace `close`.

The patch marks those descriptors close-on-exec with `fcntl(2)` instead, so `exec` closes them and `close(2)` is never called in the child. The shell still inherits only stdio. Nothing else is changed.

Remove this patch when an upstream release stops closing descriptors with `close(2)` before `exec`.
