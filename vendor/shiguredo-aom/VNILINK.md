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
