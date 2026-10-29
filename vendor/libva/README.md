# Libva Rust Wrapper

This crate provides lightweight and (hopefully) safe libva abstractions for use
within Rust code with minimal dependencies. It is developed for use in
ChromeOS, but has no ChromeOS specifics or dependencies and should thus be
usable anywhere.

## Dependencies

By default (`dlopen` feature, enabled via `default`), `libva` is runtime-loaded via [`libloading`](https://crates.io/crates/libloading) (`libva.so.2` / `libva-drm.so.2`), hence runs on systems without libva installed (e.g. generic distros, CI). `Display::open()` will simply return `None`/`Err` if the libraries cannot be loaded.

For compile-time linking, disable default features:

```toml
libva = { version = "0.1", default-features = false }
# or old alias:
# cros-libva = { package = "libva", version = "0.1", default-features = false }
```

This restores the old `pkg-config --libs libva libva-drm` link (`-lva -lva-drm`). You can also explicitly enable `linked` (alias, currently no-op) to document intent.

Headers are still needed at build time unless `vendored` is enabled (bundles `libva/va/va.h` from the submodule). The libva version needs to be 1.20.0 or newer. The VA-API driver corresponding to your hardware is also required: for Intel hardware it will be [intel-media-driver](https://github.com/intel/media-driver), whereas AMD hardware relies on [Mesa](https://gitlab.freedesktop.org/mesa/mesa).

An easy way to see whether everything is in order is to run the `vainfo`
utility packaged with `libva-utils` or as a standalone package in some
distributions. `vainfo` will print the VA-API version, driver string, and a
list of supported profiles and endpoints, i.e.:

```
vainfo: VA-API version: 1.13 (libva 2.13.0)
vainfo: Driver version: Intel iHD driver for Intel(R) Gen Graphics - 22.2.2 ()
vainfo: Supported profile and entrypoints
      VAProfileNone                   : VAEntrypointVideoProc
      VAProfileNone                   : VAEntrypointStats
      VAProfileMPEG2Simple            : VAEntrypointVLD
      VAProfileMPEG2Simple            : VAEntrypointEncSlice
      VAProfileMPEG2Main              : VAEntrypointVLD
      VAProfileMPEG2Main              : VAEntrypointEncSlice
      VAProfileH264Main               : VAEntrypointVLD
      etc
```

For decoding, the desired profile must be supported under `VAEntrypointVLD`.
For example, in order to decode VP8 media, this line must be present in the
output of `vainfo`:

```
      VAProfileVP8Version0_3          : VAEntrypointVLD
```

Whereas to decode H264 Main profile media, this line must be present:

```
      VAProfileH264Main               : VAEntrypointVLD
```

For more information on VA-API and its usage within ChromeOS, see [this
guide](https://chromium.googlesource.com/chromium/src/+/master/docs/gpu/vaapi.md).

cros-libva can also be built in Android. Android.bp files are provided that
should work on AOSP >= 15. Just check this repository into
external/rust/crates/cros-libva and the libcros_libva library target will be
 available.

## Using

Published as [`libva`](https://crates.io/crates/libva) (fork of `cros-libva` to support generic distros + `dlopen` default). Use directly:

```toml
libva = "0.1"
# compat alias for old code:
# cros-libva = { package = "libva", version = "0.1" }
```

## Testing

For a brief introduction on how to use this crate, see the
`libva_utils_mpeg2vldemo` test under `src/lib.rs`. You can also quickly test
MPEG2 decoding by running it:

```
cargo test -- --ignored libva_utils_mpeg2vldemo
```

## Credits

The first/original version of this crate was written by Daniel Almeida and hosted in the
[crosvm repository](https://chromium.googlesource.com/crosvm/crosvm/) before
being split.
