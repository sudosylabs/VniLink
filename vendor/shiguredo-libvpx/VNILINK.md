Based on shiguredo_libvpx 2026.1.0 from crates.io, Apache-2.0.

The encoder configuration used NonZeroUsize for lag_in_frames, preventing an
explicit zero-lag VP9 encoder. This patch accepts Option<usize> and passes zero
to libvpx. No native codec implementation or build revision was changed.
VniLink sets zero lag and tests immediate output over consecutive frames.

A second patch adds colour: `Vp9Config::color_space` and `full_range` declare the
stream's colour (VP9E_SET_COLOR_SPACE, VP9E_SET_COLOR_RANGE), and
`DecodedFrame::color_space` and `full_range` read back what a bitstream declares.
VniLink's software VP9 encoder declares the session's matrix (BT.709 or BT.601) in studio range, and its software
decoder converts with what the stream declares.

The build checks what it fetches against values recorded in Cargo.toml, under
`[package.metadata.external-dependencies]`, rather than trusting the source:
a source build stops unless the tag it clones names the recorded `commit`, and
a prebuilt archive is checked against the SHA-256 recorded for its target in
`prebuilt-sha256`, where upstream downloaded the expected hash from the same
release as the archive. A target with no recorded hash is refused. Updating
the crate means recording the new tag's commit and each archive's hash.
