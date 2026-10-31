# zstd-sys 2.1.2+zstd.1.5.7, trimmed

Source: crates.io `zstd-sys` 2.1.2+zstd.1.5.7, which carries zstd 1.5.7 (BSD-3-Clause,
see `LICENSE.BSD-3-Clause` and `zstd/LICENSE`; zstd is dual-licensed with GPL-2.0, and
VniLink uses it under the BSD-3-Clause terms).

The file transfer service compresses file segments in transit with zstd
(`crates/vnilink/src/transfer/codec.rs`). The C library is vendored so a build compiles the
same source on every OS without a system zstd.

No source is changed. Only files this build does not read are left out: zstd's legacy
decoders, dictionary builder, deprecated API, cmake and contrib directories, and the
wasm shim. The crate is built with `default-features = false`, so `legacy` and
`zdict_builder` stay off, which is why those directories are not needed. `Cargo.lock` is
left out too.

Remove this copy, and its `[patch.crates-io]` entry, if the transfer service stops using zstd.
