# VniLink Iroh build patch

Pinned crates.io iroh 1.3.0 source, retaining upstream MIT/Apache license files.
Only Cargo.toml's library crate-type is changed from lib+cdylib to lib.
VniLink consumes Iroh as a Rust library, never its standalone foreign library.
The unused cdylib exceeds Windows GNU PE export ordinals in debug builds
(`export ordinal too large: 104809`). Removing that artifact also avoids an
unnecessary shared-library link. No Iroh transport/runtime source is changed.
