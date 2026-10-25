# Local patch to libva 0.1.4

Source: crates.io libva 0.1.4, BSD-3-Clause (the licence header in each source file).

`Context::create_buffer` builds a `VAProcPipelineParameterBuffer` whose
`surface_region` and `output_region` point into the wrapper that owns the
rectangles, then dropped that wrapper before returning. `vaCreateBuffer` copies
only the pointers; the driver reads the rectangles when the picture renders, so
a region was read after it was freed. The patch keeps the pipeline parameter
wrapper inside the `Buffer` until the buffer is destroyed (`_referenced` in
`src/buffer.rs`). Nothing else is changed, and `Cargo.lock` and `Android.bp`
are left out.

VniLink's video processor scales into the part of an encoder picture that
holds the screen (`vnilink-linux/src/vaapi/convert.rs`), which needs
`output_region`.

Remove this patch when upstream keeps those rectangles alive.
