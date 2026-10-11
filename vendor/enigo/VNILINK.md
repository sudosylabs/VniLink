# Local patch to Enigo 0.6.1

Source: crates.io enigo 0.6.1, MIT license (LICENSE).

The macOS patch in src/macos/macos_impl.rs configures the private
Core Graphics event source with zero local-event suppression interval and
permit local mouse, keyboard and system events during both suppression states.
Without this, repeated remote events prevent the Mac owner from using local
hardware. The pointer contract also adds `Mouse::scroll_precise`: 120 units per
wheel detent, positive down/right. macOS posts continuous Core Graphics
fixed-point line deltas (with ten points per line); Windows forwards the
signed native wheel delta to SendInput. X11 uses the core adapter's signed
detent accumulator instead of claiming fractional XTest support.

Windows `raw` now uses the physical E0 prefix for extended flags and supplies
the low scan byte to SendInput. This keeps keypad Enter distinct from Enter
and includes both Windows keys, without relying on a virtual-key allowlist.

The native regression queries the real event source, without posting input:

```sh
CARGO_TARGET_DIR=target cargo test --manifest-path vendor/enigo/Cargo.toml --lib remote_source_preserves_local_mouse_and_keyboard
```

The standalone workspace isolates upstream input-generating tests from the
VniLink workspace test suite. Run only the named regression above here.
Remove this patch when upstream exposes or adopts the same source configuration.
