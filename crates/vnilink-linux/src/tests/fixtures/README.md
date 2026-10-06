These are synthetic 64×64 VideoToolbox High-profile access units: an IDR and two
predictive frames. They contain no screen capture or user data. Their PPS enables
8×8 transforms, reproducing the Mac-host/Linux-viewer compatibility failure.

Generate synthetic 64×64 and 1920×1080 frames on macOS with:

```
cargo run -p vnilink-macos --example interop_fixture -- /tmp/mac-high-fixtures
```

The reference BGRA pictures were decoded by FFmpeg 6.1.1 on Ubuntu, by concatenating
the three access units and decoding the complete sequence with `-pix_fmt bgra`.
Linux's hardware regression compares all three pictures with that independent
reference (at least 40 dB PSNR), including reference-picture reuse. The production
VA-API decoder is also runnable against the larger fixtures with:

```
cargo run --release -p vnilink-linux --example decode_interop -- /tmp/mac-high-fixtures
```

VideoToolbox output can vary across hardware and OS versions. Keep the existing
fixtures until intentionally replacing them; the live Mac syntax test also
checks the current encoder at 64×64 and 1920×1080 in Baseline and High profiles.
