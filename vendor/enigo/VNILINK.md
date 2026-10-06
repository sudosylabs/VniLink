# Local patch to Enigo 0.6.1

Source: crates.io enigo 0.6.1, MIT license (LICENSE).

The only runtime change is in src/macos/macos_impl.rs: configure the private
Core Graphics event source with zero local-event suppression interval and
permit local mouse, keyboard and system events during both suppression states.
Without this, repeated remote events prevent the Mac owner from using local
hardware. Linux and Windows implementations are unchanged.

The native regression queries the real event source, without posting input:

```sh
CARGO_TARGET_DIR=target cargo test --manifest-path vendor/enigo/Cargo.toml --lib remote_source_preserves_local_mouse_and_keyboard
```

The standalone workspace isolates upstream input-generating tests from the
VniLink workspace test suite. Run only the named regression above here.
Remove this patch when upstream exposes or adopts the same source configuration.
