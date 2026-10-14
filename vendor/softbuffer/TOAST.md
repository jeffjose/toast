softbuffer 0.4.8, as published, with one change for toast, the same
one clutch makes.

In `src/backends/wayland/buffer.rs` the buffers are made as `Argb8888`
where they were `Xrgb8888`, twice. That makes the top byte of every
pixel its opacity, which is what lets a notification have soft edges
and a shadow on Wayland: softbuffer has no way to ask for this.

Everything else is untouched. To move to a newer softbuffer, copy it
here and make the same change, or drop this folder and the
`[patch.crates-io]` in toast's Cargo.toml once softbuffer can be asked.
