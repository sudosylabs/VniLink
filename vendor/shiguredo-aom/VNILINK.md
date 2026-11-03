Based on shiguredo_aom 2026.1.0 from crates.io, Apache-2.0
(https://github.com/shiguredo/aom-rs). Cargo.lock and Cargo.toml.orig from the
published crate are left out.

This patch adds `DecodedFrame::matrix_coefficients` and `full_range`, which
read the colour a sequence header declares from libaom's image. VniLink's
software decoder converts AV1 pictures with that colour. No native codec
implementation or build revision was changed.

The build also pins `CMAKE_INSTALL_LIBDIR` to `lib`: CMake otherwise installs
to `lib64` on Fedora and other distributions that use it, where the build
script does not look for the static library.

The build checks what it fetches against values recorded in Cargo.toml, under
`[package.metadata.external-dependencies]`, rather than trusting the source:
a source build stops unless the tag it clones names the recorded `commit`, and
a prebuilt archive is checked against the SHA-256 recorded for its target in
`prebuilt-sha256`, where upstream downloaded the expected hash from the same
release as the archive. A target with no recorded hash is refused. Updating
the crate means recording the new tag's commit and each archive's hash.
