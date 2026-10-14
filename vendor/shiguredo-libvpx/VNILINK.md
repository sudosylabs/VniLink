Based on shiguredo_libvpx 2026.1.0 from crates.io, Apache-2.0.

The encoder configuration used NonZeroUsize for lag_in_frames, preventing an
explicit zero-lag VP9 encoder. This patch accepts Option<usize> and passes zero
to libvpx. No native codec implementation or build revision was changed.
VniLink sets zero lag and tests immediate output over consecutive frames.
