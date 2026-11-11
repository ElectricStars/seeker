# Third-party components

Seeker (GPL-3.0-or-later) is built from these Rust crates. All licenses below are
compatible with GPLv3; for dual-licensed crates the GPL-compatible option applies.
The bundled egui fonts (Ubuntu-Light: Ubuntu Font Licence 1.0; Hack/Noto: OFL-1.1/MIT-style) are
included as separate font data under their own licenses.

Regenerate with: `cargo license --avoid-build-deps --avoid-dev-deps --filter-platform x86_64-unknown-linux-gnu`

```
(Apache-2.0 OR MIT) AND BSD-3-Clause (1): encoding_rs
(Apache-2.0 OR MIT) AND OFL-1.1 AND Ubuntu-font-1.0 (1): epaint_default_fonts
(Apache-2.0 OR MIT) AND Unicode-3.0 (2): unicode-ident, unicode-properties
0BSD OR Apache-2.0 OR MIT (1): adler2
Apache-2.0 (10): ab_glyph, ab_glyph_rasterizer, codespan-reporting, gethostname, glutin, glutin_egl_sys, glutin_glx_sys, owned_ttf_parser, unicode-general-category, winit
Apache-2.0 AND MIT (1): dpi
Apache-2.0 OR Apache-2.0 WITH LLVM-exception OR MIT (4): linux-raw-sys, linux-raw-sys, rustix, rustix
Apache-2.0 OR BSD-2-Clause OR MIT (2): zerocopy, zerocopy-derive
Apache-2.0 OR BSD-3-Clause (2): moxcms, pxfm
Apache-2.0 OR BSL-1.0 (1): ryu
Apache-2.0 OR GPL-2.0 (1): self_cell
Apache-2.0 OR MIT (123): accesskit, ahash, arboard, arrayvec, as-raw-xcb-connection, bit-set, bit-vec, bitflags, bstr, cfg-if, chrono, color, core_detect, crc32fast, crossbeam-channel, crossbeam-deque, crossbeam-epoch, crossbeam-utils, document-features, downcast-rs, ecolor, eframe, egui, egui-wgpu, egui-winit, egui_extras, egui_glow, either, emath, encoding_rs_io, enum-map, enum-map-derive, enumn, epaint, equivalent, errno, euclid, fdeflate, fearless_simd, fixedbitset, flate2, font-types, getrandom, glifo, guillotiere, half, hashbrown, hashbrown, iana-time-zone, image, indexmap, itertools, itoa, kurbo, libc, linebender_resource_handle, litrs, lock_api, log, memmap2, multiversion_no_op, naga, naga-types, nohash-hasher, num-traits, once_cell, parking_lot, parking_lot_core, peniko, percent-encoding, petgraph, pin-project-lite, png, polling, pollster, polycool, proc-macro2, profiling, quick-error, quote, read-fonts, regex, regex-automata, regex-syntax, renderdoc-sys, ron, rustc-hash, rustc-hash, scoped-tls, scopeguard, serde, serde_core, serde_derive, simdutf8, skrifa, smallvec, smol_str, static_assertions, syn, syn, thiserror, thiserror, thiserror-impl, thiserror-impl, ttf-parser, type-map, typeid, unicode-segmentation, unicode-width, uuid, vello_common, vello_cpu, web-time, weezl, wgpu, wgpu-core, wgpu-core-deps-windows-linux-android, wgpu-hal, wgpu-naga-bridge, wgpu-types, wl-clipboard-rs, x11rb, x11rb-protocol
Apache-2.0 OR MIT OR Zlib (10): bytemuck, bytemuck_derive, cursor-icon, glow, miniz_oxide, miniz_oxide, raw-window-handle, xkeysym, zune-core, zune-jpeg
BSD-2-Clause (1): arrayref
BSD-3-Clause (2): tiny-skia, tiny-skia-path
GPL-3.0-or-later (1): seeker
ISC (1): libloading
MIT (41): calloop, calloop, calloop-wayland-source, calloop-wayland-source, dlib, fax, glutin-winit, harfrust, is-docker, is-wsl, libm, nom, open, os_pipe, quick-xml, rfd, sctk-adwaita, simd-adler32, slab, smithay-client-toolkit, smithay-client-toolkit, smithay-clipboard, strict-num, tiff, tracing, tracing-core, tree_magic_mini, wayland-backend, wayland-client, wayland-csd-frame, wayland-cursor, wayland-protocols, wayland-protocols-experimental, wayland-protocols-misc, wayland-protocols-plasma, wayland-protocols-wlr, wayland-scanner, wayland-sys, x11-dl, xcursor, xkbcommon-dl
MIT OR Unlicense (12): aho-corasick, byteorder-lite, csv, csv-core, globset, grep-matcher, grep-regex, grep-searcher, ignore, memchr, same-file, walkdir
Zlib (3): foldhash, foldhash, zlib-rs
```
