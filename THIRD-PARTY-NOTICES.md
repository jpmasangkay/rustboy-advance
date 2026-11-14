# Third-Party Notices

Rustboy Advance is licensed under the **Mozilla Public License 2.0 (MPL-2.0)**.
Complete source code for Covered Software is available at:
- Repository: <https://github.com/jpmasangkay/rustboy-advance>
- Release tag: `v0.1.0`

This document provides attribution and license notices for bundled test suites, fonts, and the statically linked Rust crate dependencies recorded in `Cargo.lock` (generated from `cargo metadata --format-version 1`).

---

## Test ROM Suites & Bundled Content

| Project / Asset | Author / Source | License / Terms | Notes |
| :--- | :--- | :--- | :--- |
| **Tonc GBA Demos (`tonc-bin.zip`)** | Jasper Vijn (`cearn` / Coranac) — <https://www.coranac.com/projects/tonc/> | **MIT License** (`Copyright (c) J Vijn`) | Used as optional local smoke-test ROMs via `scripts/fetch-test-roms.*`; not committed to git |
| **jsmolka/gba-tests** | Jonas Smolka — <https://github.com/jsmolka/gba-tests> | Open-source test ROM suite (see upstream repo) | Headless accuracy regression tests; fetched locally via `scripts/fetch-test-roms.*` |
| **FuzzARM** | DenSinH — <https://github.com/DenSinH/FuzzARM> | Open-source test ROM suite (see upstream repo) | ARM/Thumb instruction test suite; fetched locally |
| **ARMWrestler** | GBA community / jsmolka archive | Freeware / test ROM (see upstream archive) | CPU edge-case suite; fetched locally |
| **NanoBoyAdvance `hw-test`** | fleroviux / nba-emu — <https://github.com/nba-emu/hw-test> | Open-source test ROM suite (see upstream repo) | Hardware timer/IRQ/prefetch/waitstate/DMA test ROMs; fetched locally |
| **Blargg GB/GBC Test ROMs** | Shay Green (Blargg) | Permissive freeware / test ROMs | Optional Game Boy / Game Boy Color validation |
| **dmg-acid2 & cgb-acid2** | Matt Currie — <https://github.com/mattcurrie/dmg-acid2>, <https://github.com/mattcurrie/cgb-acid2> | **MIT License** | GB/GBC PPU reference tests |
| **Mooneye Test Suite (optional)** | Joonas Javanainen (Gekkio) — <https://github.com/Gekkio/mooneye-test-suite> | **MIT License** | Optional locally compiled GB/GBC timing tests |
| **`epaint_default_fonts` (via `egui`)** | Emil Ernerfeldt / Canonical / Google | `(MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0` | Embedded UI fonts (Ubuntu & Noto Emoji) linked via `egui` |

---

## Statically Linked Rust Crates (551 packages in `Cargo.lock`)

| Crate | Version | SPDX License | Repository / Homepage |
| :--- | :--- | :--- | :--- |
| `ab_glyph` | `0.2.32` | `Apache-2.0` | <https://github.com/alexheretic/ab-glyph> |
| `ab_glyph_rasterizer` | `0.1.10` | `Apache-2.0` | <https://github.com/alexheretic/ab-glyph> |
| `adler2` | `2.0.1` | `0BSD OR MIT OR Apache-2.0` | <https://github.com/oyvindln/adler2> |
| `aes` | `0.8.4` | `MIT OR Apache-2.0` | <https://github.com/RustCrypto/block-ciphers> |
| `ahash` | `0.8.11` | `MIT OR Apache-2.0` | <https://github.com/tkaitchuck/ahash> |
| `aho-corasick` | `1.1.5` | `Unlicense OR MIT` | <https://github.com/BurntSushi/aho-corasick> |
| `allocator-api2` | `0.2.21` | `MIT OR Apache-2.0` | <https://github.com/zakarumych/allocator-api2> |
| `alsa` | `0.9.1` | `Apache-2.0/MIT` | <https://github.com/diwic/alsa-rs> |
| `alsa-sys` | `0.3.1` | `MIT` | <https://github.com/diwic/alsa-sys> |
| `android_system_properties` | `0.1.6` | `MIT OR Apache-2.0` | <https://github.com/nical/android_system_properties> |
| `android-activity` | `0.6.1` | `MIT OR Apache-2.0` | <https://github.com/rust-mobile/android-activity> |
| `android-properties` | `0.2.2` | `MIT` | <https://github.com/miklelappo/android-properties> |
| `anyhow` | `1.0.104` | `MIT OR Apache-2.0` | <https://github.com/dtolnay/anyhow> |
| `arbitrary` | `1.5.0` | `MIT OR Apache-2.0` | <https://github.com/rust-fuzz/arbitrary/> |
| `arboard` | `3.4.1` | `MIT OR Apache-2.0` | <https://github.com/1Password/arboard> |
| `arrayvec` | `0.7.8` | `MIT OR Apache-2.0` | <https://github.com/bluss/arrayvec> |
| `ash` | `0.38.0+1.3.281` | `MIT OR Apache-2.0` | <https://github.com/ash-rs/ash> |
| `ashpd` | `0.11.1` | `MIT` | <https://github.com/bilelmoussaoui/ashpd> |
| `as-raw-xcb-connection` | `1.0.1` | `MIT OR Apache-2.0` | <https://github.com/psychon/as-raw-xcb-connection> |
| `async-broadcast` | `0.7.2` | `MIT OR Apache-2.0` | <https://github.com/smol-rs/async-broadcast> |
| `async-channel` | `2.5.0` | `Apache-2.0 OR MIT` | <https://github.com/smol-rs/async-channel> |
| `async-executor` | `1.14.1` | `Apache-2.0 OR MIT` | <https://github.com/smol-rs/async-executor> |
| `async-fs` | `2.2.0` | `Apache-2.0 OR MIT` | <https://github.com/smol-rs/async-fs> |
| `async-io` | `2.6.0` | `Apache-2.0 OR MIT` | <https://github.com/smol-rs/async-io> |
| `async-lock` | `3.5.0` | `Apache-2.0 OR MIT` | <https://github.com/smol-rs/async-lock> |
| `async-net` | `2.0.0` | `Apache-2.0 OR MIT` | <https://github.com/smol-rs/async-net> |
| `async-process` | `2.5.0` | `Apache-2.0 OR MIT` | <https://github.com/smol-rs/async-process> |
| `async-recursion` | `1.2.0` | `MIT OR Apache-2.0` | <https://github.com/dcchut/async-recursion> |
| `async-signal` | `0.2.14` | `Apache-2.0 OR MIT` | <https://github.com/smol-rs/async-signal> |
| `async-task` | `4.7.1` | `Apache-2.0 OR MIT` | <https://github.com/smol-rs/async-task> |
| `async-trait` | `0.1.92` | `MIT OR Apache-2.0` | <https://github.com/dtolnay/async-trait> |
| `atomic-waker` | `1.1.2` | `Apache-2.0 OR MIT` | <https://github.com/smol-rs/atomic-waker> |
| `autocfg` | `1.5.1` | `Apache-2.0 OR MIT` | <https://github.com/cuviper/autocfg> |
| `base64` | `0.22.1` | `MIT OR Apache-2.0` | <https://github.com/marshallpierce/rust-base64> |
| `bindgen` | `0.72.1` | `BSD-3-Clause` | <https://github.com/rust-lang/rust-bindgen> |
| `bitflags` | `1.3.2` | `MIT/Apache-2.0` | <https://github.com/bitflags/bitflags> |
| `bitflags` | `2.13.2` | `MIT OR Apache-2.0` | <https://github.com/bitflags/bitflags> |
| `bit-set` | `0.8.0` | `Apache-2.0 OR MIT` | <https://github.com/contain-rs/bit-set> |
| `bit-vec` | `0.8.0` | `Apache-2.0 OR MIT` | <https://github.com/contain-rs/bit-vec> |
| `block` | `0.1.6` | `MIT` | <http://github.com/SSheldon/rust-block> |
| `block2` | `0.5.1` | `MIT` | <https://github.com/madsmtm/objc2> |
| `block2` | `0.6.2` | `MIT` | <https://github.com/madsmtm/objc2> |
| `block-buffer` | `0.10.4` | `MIT OR Apache-2.0` | <https://github.com/RustCrypto/utils> |
| `blocking` | `1.7.0` | `Apache-2.0 OR MIT` | <https://github.com/smol-rs/blocking> |
| `block-padding` | `0.3.3` | `MIT OR Apache-2.0` | <https://github.com/RustCrypto/utils> |
| `bumpalo` | `3.20.3` | `MIT OR Apache-2.0` | <https://github.com/fitzgen/bumpalo> |
| `bytemuck` | `1.25.2` | `Zlib OR Apache-2.0 OR MIT` | <https://github.com/Lokathor/bytemuck> |
| `bytemuck_derive` | `1.12.1` | `Zlib OR Apache-2.0 OR MIT` | <https://github.com/Lokathor/bytemuck> |
| `byteorder` | `1.5.0` | `Unlicense OR MIT` | <https://github.com/BurntSushi/byteorder> |
| `byteorder-lite` | `0.1.0` | `Unlicense OR MIT` | <https://github.com/image-rs/byteorder-lite> |
| `bytes` | `1.12.1` | `MIT` | <https://github.com/tokio-rs/bytes> |
| `calloop` | `0.13.0` | `MIT` | <https://github.com/Smithay/calloop> |
| `calloop` | `0.14.5` | `MIT` | <https://github.com/Smithay/calloop> |
| `calloop-wayland-source` | `0.3.0` | `MIT` | <https://github.com/smithay/calloop-wayland-source> |
| `calloop-wayland-source` | `0.4.1` | `MIT` | <https://github.com/smithay/calloop-wayland-source> |
| `cbc` | `0.1.2` | `MIT OR Apache-2.0` | <https://github.com/RustCrypto/block-modes> |
| `cc` | `1.11.1` | `MIT OR Apache-2.0` | <https://github.com/rust-lang/cc-rs> |
| `cesu8` | `1.1.0` | `Apache-2.0/MIT` | <https://github.com/emk/cesu8-rs> |
| `cexpr` | `0.6.0` | `Apache-2.0/MIT` | <https://github.com/jethrogb/rust-cexpr> |
| `cfg_aliases` | `0.2.2` | `MIT` | <https://github.com/katharostech/cfg_aliases> |
| `cfg-if` | `1.0.5` | `MIT OR Apache-2.0` | <https://github.com/rust-lang/cfg-if> |
| `cgl` | `0.3.2` | `MIT / Apache-2.0` | <https://github.com/servo/cgl-rs> |
| `cipher` | `0.4.4` | `MIT OR Apache-2.0` | <https://github.com/RustCrypto/traits> |
| `clang-sys` | `1.9.1` | `Apache-2.0` | <https://github.com/KyleMayes/clang-sys> |
| `clipboard-win` | `5.4.3` | `BSL-1.0` | <https://github.com/DoumanAsh/clipboard-win> |
| `cobs` | `0.3.0` | `MIT OR Apache-2.0` | <https://github.com/jamesmunns/cobs.rs> |
| `codespan-reporting` | `0.11.1` | `Apache-2.0` | <https://github.com/brendanzab/codespan> |
| `color_quant` | `1.1.0` | `MIT` | <https://github.com/image-rs/color_quant.git> |
| `combine` | `4.6.9` | `MIT` | <https://github.com/Marwes/combine> |
| `concurrent-queue` | `2.5.0` | `Apache-2.0 OR MIT` | <https://github.com/smol-rs/concurrent-queue> |
| `coreaudio-rs` | `0.11.3` | `MIT/Apache-2.0` | <https://github.com/RustAudio/coreaudio-rs.git> |
| `coreaudio-sys` | `0.2.18` | `MIT` | <https://github.com/RustAudio/coreaudio-sys.git> |
| `core-foundation` | `0.10.1` | `MIT OR Apache-2.0` | <https://github.com/servo/core-foundation-rs> |
| `core-foundation` | `0.9.4` | `MIT OR Apache-2.0` | <https://github.com/servo/core-foundation-rs> |
| `core-foundation-sys` | `0.8.7` | `MIT OR Apache-2.0` | <https://github.com/servo/core-foundation-rs> |
| `core-graphics` | `0.23.2` | `MIT OR Apache-2.0` | <https://github.com/servo/core-foundation-rs> |
| `core-graphics-types` | `0.1.3` | `MIT OR Apache-2.0` | <https://github.com/servo/core-foundation-rs> |
| `cpal` | `0.15.3` | `Apache-2.0` | <https://github.com/rustaudio/cpal> |
| `cpufeatures` | `0.2.17` | `MIT OR Apache-2.0` | <https://github.com/RustCrypto/utils> |
| `crc32fast` | `1.5.5` | `MIT OR Apache-2.0` | <https://github.com/srijs/rust-crc32fast> |
| `crossbeam-utils` | `0.8.23` | `MIT OR Apache-2.0` | <https://github.com/crossbeam-rs/crossbeam> |
| `crunchy` | `0.2.4` | `MIT` | <https://github.com/eira-fransham/crunchy> |
| `crypto-common` | `0.1.7` | `MIT OR Apache-2.0` | <https://github.com/RustCrypto/traits> |
| `ctor` | `0.10.1` | `Apache-2.0 OR MIT` | <https://github.com/mmastrac/rust-ctor> |
| `cursor-icon` | `1.2.0` | `MIT OR Apache-2.0 OR Zlib` | <https://github.com/rust-windowing/cursor-icon> |
| `dasp_sample` | `0.11.0` | `MIT OR Apache-2.0` | <https://github.com/rustaudio/sample.git> |
| `dbus` | `0.9.12` | `Apache-2.0/MIT` | <https://github.com/diwic/dbus-rs> |
| `dbus-secret-service` | `4.1.0` | `MIT OR Apache-2.0` | <https://github.com/brotskydotcom/dbus-secret-service.git> |
| `derive_arbitrary` | `1.5.0` | `MIT OR Apache-2.0` | <https://github.com/rust-fuzz/arbitrary> |
| `digest` | `0.10.7` | `MIT OR Apache-2.0` | <https://github.com/RustCrypto/traits> |
| `dispatch` | `0.2.0` | `MIT` | <http://github.com/SSheldon/rust-dispatch> |
| `dispatch2` | `0.3.1` | `Zlib OR Apache-2.0 OR MIT` | <https://github.com/madsmtm/objc2> |
| `displaydoc` | `0.2.7` | `MIT OR Apache-2.0` | <https://github.com/yaahc/displaydoc> |
| `dlib` | `0.5.3` | `MIT` | <https://github.com/elinorbgr/dlib> |
| `document-features` | `0.2.12` | `MIT OR Apache-2.0` | <https://github.com/slint-ui/document-features> |
| `downcast-rs` | `1.2.1` | `MIT/Apache-2.0` | <https://github.com/marcianx/downcast-rs> |
| `dpi` | `0.1.2` | `Apache-2.0 AND MIT` | <https://github.com/rust-windowing/winit> |
| `drm` | `0.14.1` | `MIT` | <https://github.com/Smithay/drm-rs> |
| `drm-ffi` | `0.9.1` | `MIT` | <https://github.com/Smithay/drm-rs> |
| `drm-fourcc` | `2.2.0` | `MIT` | <https://github.com/danielzfranklin/drm-fourcc-rs> |
| `drm-sys` | `0.8.1` | `MIT` | <https://github.com/Smithay/drm-rs> |
| `dtor` | `0.8.1` | `Apache-2.0 OR MIT` | <https://github.com/mmastrac/rust-ctor> |
| `ecolor` | `0.31.1` | `MIT OR Apache-2.0` | <https://github.com/emilk/egui> |
| `eframe` | `0.31.1` | `MIT OR Apache-2.0` | <https://github.com/emilk/egui/tree/master/crates/eframe> |
| `egui` | `0.31.1` | `MIT OR Apache-2.0` | <https://github.com/emilk/egui> |
| `egui_glow` | `0.31.1` | `MIT OR Apache-2.0` | <https://github.com/emilk/egui/tree/master/crates/egui_glow> |
| `egui-wgpu` | `0.31.1` | `MIT OR Apache-2.0` | <https://github.com/emilk/egui/tree/master/crates/egui-wgpu> |
| `egui-winit` | `0.31.1` | `MIT OR Apache-2.0` | <https://github.com/emilk/egui/tree/master/crates/egui-winit> |
| `either` | `1.19.0` | `MIT OR Apache-2.0` | <https://github.com/rayon-rs/either> |
| `emath` | `0.31.1` | `MIT OR Apache-2.0` | <https://github.com/emilk/egui/tree/master/crates/emath> |
| `embedded-io` | `0.4.0` | `MIT OR Apache-2.0` | <https://github.com/embassy-rs/embedded-io> |
| `embedded-io` | `0.6.1` | `MIT OR Apache-2.0` | <https://github.com/rust-embedded/embedded-hal> |
| `endi` | `1.1.1` | `MIT` | <https://github.com/zeenix/endi> |
| `enumflags2` | `0.7.12` | `MIT OR Apache-2.0` | <https://github.com/meithecatte/enumflags2> |
| `enumflags2_derive` | `0.7.12` | `MIT OR Apache-2.0` | <https://github.com/meithecatte/enumflags2> |
| `epaint` | `0.31.1` | `MIT OR Apache-2.0` | <https://github.com/emilk/egui/tree/master/crates/epaint> |
| `epaint_default_fonts` | `0.31.1` | `(MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0` | <https://github.com/emilk/egui/tree/master/crates/epaint_default_fonts> |
| `equivalent` | `1.0.3` | `Apache-2.0 OR MIT` | <https://github.com/indexmap-rs/equivalent> |
| `errno` | `0.3.14` | `MIT OR Apache-2.0` | <https://github.com/lambda-fairy/rust-errno> |
| `error-code` | `3.4.0` | `BSL-1.0` | <https://github.com/DoumanAsh/error-code> |
| `event-listener` | `5.4.2` | `Apache-2.0 OR MIT` | <https://github.com/smol-rs/event-listener> |
| `event-listener-strategy` | `0.5.4` | `Apache-2.0 OR MIT` | <https://github.com/smol-rs/event-listener-strategy> |
| `fastrand` | `2.5.0` | `Apache-2.0 OR MIT` | <https://github.com/smol-rs/fastrand> |
| `fax` | `0.2.7` | `MIT` | <https://github.com/pdf-rs/fax> |
| `fdeflate` | `0.3.7` | `MIT OR Apache-2.0` | <https://github.com/image-rs/fdeflate> |
| `find-msvc-tools` | `0.1.15` | `MIT OR Apache-2.0` | <https://github.com/rust-lang/cc-rs> |
| `flate2` | `1.1.11` | `MIT OR Apache-2.0` | <https://github.com/rust-lang/flate2-rs> |
| `fnv` | `1.0.7` | `Apache-2.0 / MIT` | <https://github.com/servo/rust-fnv> |
| `foldhash` | `0.1.5` | `Zlib` | <https://github.com/orlp/foldhash> |
| `foreign-types` | `0.3.2` | `MIT/Apache-2.0` | <https://github.com/sfackler/foreign-types> |
| `foreign-types` | `0.5.0` | `MIT/Apache-2.0` | <https://github.com/sfackler/foreign-types> |
| `foreign-types-macros` | `0.2.4` | `MIT/Apache-2.0` | <https://github.com/sfackler/foreign-types> |
| `foreign-types-shared` | `0.1.1` | `MIT/Apache-2.0` | <https://github.com/sfackler/foreign-types> |
| `foreign-types-shared` | `0.3.1` | `MIT/Apache-2.0` | <https://github.com/sfackler/foreign-types> |
| `form_urlencoded` | `1.2.2` | `MIT OR Apache-2.0` | <https://github.com/servo/rust-url> |
| `futures-channel` | `0.3.34` | `MIT OR Apache-2.0` | <https://github.com/rust-lang/futures-rs> |
| `futures-core` | `0.3.34` | `MIT OR Apache-2.0` | <https://github.com/rust-lang/futures-rs> |
| `futures-io` | `0.3.34` | `MIT OR Apache-2.0` | <https://github.com/rust-lang/futures-rs> |
| `futures-lite` | `2.6.1` | `Apache-2.0 OR MIT` | <https://github.com/smol-rs/futures-lite> |
| `futures-macro` | `0.3.34` | `MIT OR Apache-2.0` | <https://github.com/rust-lang/futures-rs> |
| `futures-sink` | `0.3.34` | `MIT OR Apache-2.0` | <https://github.com/rust-lang/futures-rs> |
| `futures-task` | `0.3.34` | `MIT OR Apache-2.0` | <https://github.com/rust-lang/futures-rs> |
| `futures-util` | `0.3.34` | `MIT OR Apache-2.0` | <https://github.com/rust-lang/futures-rs> |
| `gc-arena` | `0.5.3` | `MIT` | <https://github.com/kyren/gc-arena> |
| `gc-arena-derive` | `0.5.3` | `MIT` | <https://github.com/kyren/gc-arena> |
| `generic-array` | `0.14.7` | `MIT` | <https://github.com/fizyk20/generic-array.git> |
| `gethostname` | `1.1.0` | `Apache-2.0` | <https://codeberg.org/swsnr/gethostname.rs.git> |
| `getrandom` | `0.2.17` | `MIT OR Apache-2.0` | <https://github.com/rust-random/getrandom> |
| `getrandom` | `0.3.4` | `MIT OR Apache-2.0` | <https://github.com/rust-random/getrandom> |
| `getrandom` | `0.4.3` | `MIT OR Apache-2.0` | <https://github.com/rust-random/getrandom> |
| `gif` | `0.13.3` | `MIT OR Apache-2.0` | <https://github.com/image-rs/image-gif> |
| `gilrs` | `0.11.2` | `Apache-2.0/MIT` | <https://gitlab.com/gilrs-project/gilrs> |
| `gilrs-core` | `0.6.8` | `Apache-2.0/MIT` | <https://gitlab.com/gilrs-project/gilrs> |
| `gl_generator` | `0.14.0` | `Apache-2.0` | <https://github.com/brendanzab/gl-rs/> |
| `glob` | `0.3.4` | `MIT OR Apache-2.0` | <https://github.com/rust-lang/glob> |
| `glow` | `0.16.0` | `MIT OR Apache-2.0 OR Zlib` | <https://github.com/grovesNL/glow> |
| `glutin` | `0.32.3` | `Apache-2.0` | <https://github.com/rust-windowing/glutin> |
| `glutin_egl_sys` | `0.7.1` | `Apache-2.0` | <https://github.com/rust-windowing/glutin> |
| `glutin_glx_sys` | `0.6.1` | `Apache-2.0` | <https://github.com/rust-windowing/glutin> |
| `glutin_wgl_sys` | `0.6.1` | `Apache-2.0` | <https://github.com/rust-windowing/glutin> |
| `glutin-winit` | `0.5.0` | `MIT` | <https://github.com/rust-windowing/glutin> |
| `gpu-alloc` | `0.6.2` | `MIT OR Apache-2.0` | <https://github.com/zakarumych/gpu-alloc> |
| `gpu-allocator` | `0.27.0` | `MIT OR Apache-2.0` | <https://github.com/Traverse-Research/gpu-allocator> |
| `gpu-alloc-types` | `0.3.1` | `MIT OR Apache-2.0` | <https://github.com/zakarumych/gpu-alloc> |
| `gpu-descriptor` | `0.3.2` | `MIT OR Apache-2.0` | <https://github.com/zakarumych/gpu-descriptor> |
| `gpu-descriptor-types` | `0.2.0` | `MIT OR Apache-2.0` | <https://github.com/zakarumych/gpu-descriptor> |
| `half` | `2.7.1` | `MIT OR Apache-2.0` | <https://github.com/VoidStarKat/half-rs> |
| `hashbrown` | `0.14.5` | `MIT OR Apache-2.0` | <https://github.com/rust-lang/hashbrown> |
| `hashbrown` | `0.15.5` | `MIT OR Apache-2.0` | <https://github.com/rust-lang/hashbrown> |
| `hashbrown` | `0.17.1` | `MIT OR Apache-2.0` | <https://github.com/rust-lang/hashbrown> |
| `heck` | `0.5.0` | `MIT OR Apache-2.0` | <https://github.com/withoutboats/heck> |
| `hermit-abi` | `0.5.3` | `MIT OR Apache-2.0` | <https://github.com/hermit-os/hermit-rs> |
| `hex` | `0.4.3` | `MIT OR Apache-2.0` | <https://github.com/KokaKiwi/rust-hex> |
| `hexf-parse` | `0.2.1` | `CC0-1.0` | <https://github.com/lifthrasiir/hexf> |
| `hkdf` | `0.12.4` | `MIT OR Apache-2.0` | <https://github.com/RustCrypto/KDFs/> |
| `hmac` | `0.12.1` | `MIT OR Apache-2.0` | <https://github.com/RustCrypto/MACs> |
| `icu_collections` | `2.4.0` | `Unicode-3.0` | <https://github.com/unicode-org/icu4x> |
| `icu_locale_core` | `2.4.0` | `Unicode-3.0` | <https://github.com/unicode-org/icu4x> |
| `icu_normalizer` | `2.4.0` | `Unicode-3.0` | <https://github.com/unicode-org/icu4x> |
| `icu_normalizer_data` | `2.4.0` | `Unicode-3.0` | <https://github.com/unicode-org/icu4x> |
| `icu_properties` | `2.4.0` | `Unicode-3.0` | <https://github.com/unicode-org/icu4x> |
| `icu_properties_data` | `2.4.0` | `Unicode-3.0` | <https://github.com/unicode-org/icu4x> |
| `icu_provider` | `2.4.0` | `Unicode-3.0` | <https://github.com/unicode-org/icu4x> |
| `idna` | `1.1.0` | `MIT OR Apache-2.0` | <https://github.com/servo/rust-url/> |
| `idna_adapter` | `1.2.2` | `Apache-2.0 OR MIT` | <https://github.com/hsivonen/idna_adapter> |
| `image` | `0.25.10` | `MIT OR Apache-2.0` | <https://github.com/image-rs/image> |
| `indexmap` | `2.14.2` | `Apache-2.0 OR MIT` | <https://github.com/indexmap-rs/indexmap> |
| `inotify` | `0.11.5` | `ISC` | <https://github.com/hannobraun/inotify-rs> |
| `inotify-sys` | `0.1.8` | `ISC` | <https://github.com/hannobraun/inotify-sys> |
| `inout` | `0.1.4` | `MIT OR Apache-2.0` | <https://github.com/RustCrypto/utils> |
| `itertools` | `0.13.0` | `MIT OR Apache-2.0` | <https://github.com/rust-itertools/itertools> |
| `itoa` | `1.0.19` | `MIT OR Apache-2.0` | <https://github.com/dtolnay/itoa> |
| `jni` | `0.21.1` | `MIT/Apache-2.0` | <https://github.com/jni-rs/jni-rs> |
| `jni` | `0.22.4` | `MIT OR Apache-2.0` | <https://github.com/jni-rs/jni-rs> |
| `jni-macros` | `0.22.4` | `MIT OR Apache-2.0` | <https://github.com/jni-rs/jni-rs> |
| `jni-sys` | `0.3.1` | `MIT OR Apache-2.0` | <https://github.com/jni-rs/jni-sys> |
| `jni-sys` | `0.4.1` | `MIT OR Apache-2.0` | <https://github.com/jni-rs/jni-sys> |
| `jni-sys-macros` | `0.4.1` | `MIT OR Apache-2.0` | <https://github.com/jni-rs/jni-sys> |
| `jobserver` | `0.1.35` | `MIT OR Apache-2.0` | <https://github.com/rust-lang/jobserver-rs> |
| `js-sys` | `0.3.107` | `MIT OR Apache-2.0` | <https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/js-sys> |
| `keyring` | `3.6.3` | `MIT OR Apache-2.0` | <https://github.com/hwchen/keyring-rs.git> |
| `khronos_api` | `3.1.0` | `Apache-2.0` | <https://github.com/brendanzab/gl-rs/> |
| `khronos-egl` | `6.0.0` | `MIT/Apache-2.0` | <https://github.com/timothee-haudebourg/khronos-egl> |
| `lazy_static` | `1.5.1` | `MIT OR Apache-2.0` | <https://github.com/rust-lang-nursery/lazy-static.rs> |
| `libc` | `0.2.191` | `MIT OR Apache-2.0` | <https://github.com/rust-lang/libc> |
| `libdbus-sys` | `0.2.7` | `Apache-2.0/MIT` | <https://github.com/diwic/dbus-rs> |
| `libloading` | `0.8.6` | `ISC` | <https://github.com/nagisa/rust_libloading/> |
| `libmimalloc-sys` | `0.1.49` | `MIT` | <https://github.com/purpleprotocol/mimalloc_rust/tree/master/libmimalloc-sys> |
| `libredox` | `0.1.25` | `MIT` | <https://gitlab.redox-os.org/redox-os/libredox.git> |
| `libudev-sys` | `0.1.4` | `MIT` | <https://github.com/dcuddeback/libudev-sys> |
| `linux-keyutils` | `0.2.5` | `Apache-2.0 OR MIT` | <https://github.com/landhb/linux-keyutils> |
| `linux-raw-sys` | `0.12.1` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | <https://github.com/sunfishcode/linux-raw-sys> |
| `linux-raw-sys` | `0.4.15` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | <https://github.com/sunfishcode/linux-raw-sys> |
| `linux-raw-sys` | `0.9.4` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | <https://github.com/sunfishcode/linux-raw-sys> |
| `litemap` | `0.8.3` | `Unicode-3.0` | <https://github.com/unicode-org/icu4x> |
| `litrs` | `1.0.1` | `MIT OR Apache-2.0` | <https://github.com/LukasKalbertodt/litrs> |
| `lock_api` | `0.4.14` | `MIT OR Apache-2.0` | <https://github.com/Amanieu/parking_lot> |
| `log` | `0.4.35` | `MIT OR Apache-2.0` | <https://github.com/rust-lang/log> |
| `lz4_flex` | `0.14.0` | `MIT` | <https://github.com/pseitz/lz4_flex> |
| `lzma-rust2` | `0.25.0` | `Apache-2.0` | <https://github.com/hasenbanck/lzma-rust2/> |
| `mach2` | `0.4.3` | `BSD-2-Clause OR MIT OR Apache-2.0` | <https://github.com/JohnTitor/mach2> |
| `malloc_buf` | `0.0.6` | `MIT` | <https://github.com/SSheldon/malloc_buf> |
| `md-5` | `0.10.6` | `MIT OR Apache-2.0` | <https://github.com/RustCrypto/hashes> |
| `memchr` | `2.8.3` | `Unlicense OR MIT` | <https://github.com/BurntSushi/memchr> |
| `memmap2` | `0.9.12` | `MIT OR Apache-2.0` | <https://github.com/RazrFalcon/memmap2-rs> |
| `memoffset` | `0.9.1` | `MIT` | <https://github.com/Gilnaa/memoffset> |
| `metal` | `0.31.0` | `MIT OR Apache-2.0` | <https://github.com/gfx-rs/metal-rs> |
| `mimalloc` | `0.1.52` | `MIT` | <https://github.com/purpleprotocol/mimalloc_rust> |
| `minimal-lexical` | `0.2.1` | `MIT/Apache-2.0` | <https://github.com/Alexhuszagh/minimal-lexical> |
| `miniz_oxide` | `0.8.9` | `MIT OR Zlib OR Apache-2.0` | <https://github.com/Frommi/miniz_oxide/tree/master/miniz_oxide> |
| `miniz_oxide` | `0.9.1` | `MIT OR Zlib OR Apache-2.0` | <https://github.com/Frommi/miniz_oxide/tree/master/miniz_oxide> |
| `moxcms` | `0.8.1` | `BSD-3-Clause OR Apache-2.0` | <https://github.com/awxkee/moxcms.git> |
| `naga` | `24.0.0` | `MIT OR Apache-2.0` | <https://github.com/gfx-rs/wgpu/tree/trunk/naga> |
| `native-tls` | `0.2.14` | `MIT OR Apache-2.0` | <https://github.com/sfackler/rust-native-tls> |
| `ndk` | `0.8.0` | `MIT OR Apache-2.0` | <https://github.com/rust-mobile/ndk> |
| `ndk` | `0.9.0` | `MIT OR Apache-2.0` | <https://github.com/rust-mobile/ndk> |
| `ndk-context` | `0.1.1` | `MIT OR Apache-2.0` | <https://github.com/rust-windowing/android-ndk-rs> |
| `ndk-sys` | `0.5.0+25.2.9519653` | `MIT OR Apache-2.0` | <https://github.com/rust-mobile/ndk> |
| `ndk-sys` | `0.6.0+11769913` | `MIT OR Apache-2.0` | <https://github.com/rust-mobile/ndk> |
| `nix` | `0.29.0` | `MIT` | <https://github.com/nix-rust/nix> |
| `nix` | `0.31.3` | `MIT` | <https://github.com/nix-rust/nix> |
| `nohash-hasher` | `0.2.0` | `Apache-2.0 OR MIT` | <https://github.com/paritytech/nohash-hasher> |
| `nom` | `7.1.3` | `MIT` | <https://github.com/Geal/nom> |
| `num` | `0.4.3` | `MIT OR Apache-2.0` | <https://github.com/rust-num/num> |
| `num_enum` | `0.7.6` | `BSD-3-Clause OR MIT OR Apache-2.0` | <https://github.com/illicitonion/num_enum> |
| `num_enum_derive` | `0.7.6` | `BSD-3-Clause OR MIT OR Apache-2.0` | <https://github.com/illicitonion/num_enum> |
| `num-bigint` | `0.4.8` | `MIT OR Apache-2.0` | <https://github.com/rust-num/num-bigint> |
| `num-complex` | `0.4.6` | `MIT OR Apache-2.0` | <https://github.com/rust-num/num-complex> |
| `num-derive` | `0.4.2` | `MIT OR Apache-2.0` | <https://github.com/rust-num/num-derive> |
| `num-integer` | `0.1.47` | `MIT OR Apache-2.0` | <https://github.com/rust-num/num-integer> |
| `num-iter` | `0.1.46` | `MIT OR Apache-2.0` | <https://github.com/rust-num/num-iter> |
| `num-rational` | `0.4.2` | `MIT OR Apache-2.0` | <https://github.com/rust-num/num-rational> |
| `num-traits` | `0.2.19` | `MIT OR Apache-2.0` | <https://github.com/rust-num/num-traits> |
| `objc` | `0.2.7` | `MIT` | <http://github.com/SSheldon/rust-objc> |
| `objc2` | `0.5.3` | `MIT` | <https://github.com/madsmtm/objc2> |
| `objc2` | `0.6.5` | `MIT` | <https://github.com/madsmtm/objc2> |
| `objc2-app-kit` | `0.2.2` | `MIT` | <https://github.com/madsmtm/objc2> |
| `objc2-app-kit` | `0.3.2` | `Zlib OR Apache-2.0 OR MIT` | <https://github.com/madsmtm/objc2> |
| `objc2-cloud-kit` | `0.2.2` | `MIT` | <https://github.com/madsmtm/objc2> |
| `objc2-contacts` | `0.2.2` | `MIT` | <https://github.com/madsmtm/objc2> |
| `objc2-core-data` | `0.2.2` | `MIT` | <https://github.com/madsmtm/objc2> |
| `objc2-core-foundation` | `0.3.2` | `Zlib OR Apache-2.0 OR MIT` | <https://github.com/madsmtm/objc2> |
| `objc2-core-graphics` | `0.3.2` | `Zlib OR Apache-2.0 OR MIT` | <https://github.com/madsmtm/objc2> |
| `objc2-core-image` | `0.2.2` | `MIT` | <https://github.com/madsmtm/objc2> |
| `objc2-core-location` | `0.2.2` | `MIT` | <https://github.com/madsmtm/objc2> |
| `objc2-encode` | `4.1.0` | `MIT` | <https://github.com/madsmtm/objc2> |
| `objc2-foundation` | `0.2.2` | `MIT` | <https://github.com/madsmtm/objc2> |
| `objc2-foundation` | `0.3.2` | `MIT` | <https://github.com/madsmtm/objc2> |
| `objc2-io-kit` | `0.3.2` | `Zlib OR Apache-2.0 OR MIT` | <https://github.com/madsmtm/objc2> |
| `objc2-io-surface` | `0.3.2` | `Zlib OR Apache-2.0 OR MIT` | <https://github.com/madsmtm/objc2> |
| `objc2-link-presentation` | `0.2.2` | `MIT` | <https://github.com/madsmtm/objc2> |
| `objc2-metal` | `0.2.2` | `MIT` | <https://github.com/madsmtm/objc2> |
| `objc2-quartz-core` | `0.2.2` | `MIT` | <https://github.com/madsmtm/objc2> |
| `objc2-quartz-core` | `0.3.2` | `Zlib OR Apache-2.0 OR MIT` | <https://github.com/madsmtm/objc2> |
| `objc2-symbols` | `0.2.2` | `MIT` | <https://github.com/madsmtm/objc2> |
| `objc2-ui-kit` | `0.2.2` | `MIT` | <https://github.com/madsmtm/objc2> |
| `objc2-uniform-type-identifiers` | `0.2.2` | `MIT` | <https://github.com/madsmtm/objc2> |
| `objc2-user-notifications` | `0.2.2` | `MIT` | <https://github.com/madsmtm/objc2> |
| `objc-sys` | `0.3.5` | `MIT` | <https://github.com/madsmtm/objc2> |
| `oboe` | `0.6.1` | `Apache-2.0` | <https://github.com/katyo/oboe-rs> |
| `oboe-sys` | `0.6.1` | `Apache-2.0` | <https://github.com/katyo/oboe-rs> |
| `once_cell` | `1.21.4` | `MIT OR Apache-2.0` | <https://github.com/matklad/once_cell> |
| `openssl` | `0.10.82` | `Apache-2.0` | <https://github.com/rust-openssl/rust-openssl> |
| `openssl-macros` | `0.1.1` | `MIT/Apache-2.0` | — |
| `openssl-probe` | `0.1.6` | `MIT/Apache-2.0` | <https://github.com/alexcrichton/openssl-probe> |
| `openssl-sys` | `0.9.118` | `MIT` | <https://github.com/rust-openssl/rust-openssl> |
| `orbclient` | `0.3.55` | `MIT` | <https://gitlab.redox-os.org/redox-os/orbclient> |
| `ordered-float` | `4.6.0` | `MIT` | <https://github.com/reem/rust-ordered-float> |
| `ordered-stream` | `0.2.0` | `MIT OR Apache-2.0` | <https://github.com/danieldg/ordered-stream> |
| `owned_ttf_parser` | `0.25.1` | `Apache-2.0` | <https://github.com/alexheretic/owned-ttf-parser> |
| `parking` | `2.2.1` | `Apache-2.0 OR MIT` | <https://github.com/smol-rs/parking> |
| `parking_lot` | `0.12.3` | `MIT OR Apache-2.0` | <https://github.com/Amanieu/parking_lot> |
| `parking_lot_core` | `0.9.10` | `MIT OR Apache-2.0` | <https://github.com/Amanieu/parking_lot> |
| `paste` | `1.0.15` | `MIT OR Apache-2.0` | <https://github.com/dtolnay/paste> |
| `percent-encoding` | `2.3.2` | `MIT OR Apache-2.0` | <https://github.com/servo/rust-url/> |
| `piccolo` | `0.3.3` | `MIT` | <https://github.com/kyren/piccolo> |
| `pin-project` | `1.1.13` | `Apache-2.0 OR MIT` | <https://github.com/taiki-e/pin-project> |
| `pin-project-internal` | `1.1.13` | `Apache-2.0 OR MIT` | <https://github.com/taiki-e/pin-project> |
| `pin-project-lite` | `0.2.17` | `Apache-2.0 OR MIT` | <https://github.com/taiki-e/pin-project-lite> |
| `piper` | `0.2.5` | `MIT OR Apache-2.0` | <https://github.com/smol-rs/piper> |
| `pkg-config` | `0.3.34` | `MIT OR Apache-2.0` | <https://github.com/rust-lang/pkg-config-rs> |
| `plain` | `0.2.3` | `MIT/Apache-2.0` | <https://github.com/randomites/plain> |
| `png` | `0.18.1` | `MIT OR Apache-2.0` | <https://github.com/image-rs/image-png> |
| `polling` | `3.11.0` | `Apache-2.0 OR MIT` | <https://github.com/smol-rs/polling> |
| `pollster` | `0.4.0` | `Apache-2.0/MIT` | <https://github.com/zesterer/pollster> |
| `portable-atomic` | `1.15.0` | `Apache-2.0 OR MIT` | <https://github.com/taiki-e/portable-atomic> |
| `portable-atomic-util` | `0.2.8` | `Apache-2.0 OR MIT` | <https://github.com/taiki-e/portable-atomic-util> |
| `postcard` | `1.1.3` | `MIT OR Apache-2.0` | <https://github.com/jamesmunns/postcard> |
| `potential_utf` | `0.1.6` | `Unicode-3.0` | <https://github.com/unicode-org/icu4x> |
| `ppv-lite86` | `0.2.21` | `MIT OR Apache-2.0` | <https://github.com/cryptocorrosion/cryptocorrosion> |
| `presser` | `0.3.1` | `MIT OR Apache-2.0` | <https://github.com/EmbarkStudios/presser> |
| `proc-macro2` | `1.0.107` | `MIT OR Apache-2.0` | <https://github.com/dtolnay/proc-macro2> |
| `proc-macro-crate` | `3.5.0` | `MIT OR Apache-2.0` | <https://github.com/bkchr/proc-macro-crate> |
| `profiling` | `1.0.18` | `MIT OR Apache-2.0` | <https://github.com/aclysma/profiling> |
| `pxfm` | `0.1.30` | `BSD-3-Clause OR Apache-2.0` | <https://github.com/awxkee/pxfm> |
| `quick-error` | `2.0.1` | `MIT/Apache-2.0` | <http://github.com/tailhook/quick-error> |
| `quick-xml` | `0.41.0` | `MIT` | <https://github.com/tafia/quick-xml> |
| `quote` | `1.0.47` | `MIT OR Apache-2.0` | <https://github.com/dtolnay/quote> |
| `rand` | `0.8.8` | `MIT OR Apache-2.0` | <https://github.com/rust-random/rand> |
| `rand` | `0.9.5` | `MIT OR Apache-2.0` | <https://github.com/rust-random/rand> |
| `rand_chacha` | `0.3.1` | `MIT OR Apache-2.0` | <https://github.com/rust-random/rand> |
| `rand_chacha` | `0.9.0` | `MIT OR Apache-2.0` | <https://github.com/rust-random/rand> |
| `rand_core` | `0.6.4` | `MIT OR Apache-2.0` | <https://github.com/rust-random/rand> |
| `rand_core` | `0.9.5` | `MIT OR Apache-2.0` | <https://github.com/rust-random/rand> |
| `range-alloc` | `0.1.5` | `MIT OR Apache-2.0` | <https://github.com/gfx-rs/range-alloc> |
| `raw-window-handle` | `0.6.2` | `MIT OR Apache-2.0 OR Zlib` | <https://github.com/rust-windowing/raw-window-handle> |
| `redox_syscall` | `0.4.1` | `MIT` | <https://gitlab.redox-os.org/redox-os/syscall> |
| `redox_syscall` | `0.5.18` | `MIT` | <https://gitlab.redox-os.org/redox-os/syscall> |
| `redox_syscall` | `0.9.5` | `MIT` | <https://gitlab.redox-os.org/redox-os/kernel> |
| `r-efi` | `5.3.0` | `MIT OR Apache-2.0 OR LGPL-2.1-or-later` | <https://github.com/r-efi/r-efi> |
| `r-efi` | `6.0.0` | `MIT OR Apache-2.0 OR LGPL-2.1-or-later` | <https://github.com/r-efi/r-efi> |
| `regex` | `1.13.1` | `MIT OR Apache-2.0` | <https://github.com/rust-lang/regex> |
| `regex-automata` | `0.4.18` | `MIT OR Apache-2.0` | <https://github.com/rust-lang/regex> |
| `regex-syntax` | `0.8.11` | `MIT OR Apache-2.0` | <https://github.com/rust-lang/regex> |
| `renderdoc-sys` | `1.1.0` | `MIT OR Apache-2.0` | <https://github.com/ebkalderon/renderdoc-rs> |
| `rfd` | `0.15.4` | `MIT` | <https://github.com/PolyMeilex/rfd> |
| `ringbuf` | `0.4.8` | `MIT/Apache-2.0` | <https://github.com/agerasev/ringbuf.git> |
| `rustc_version` | `0.4.1` | `MIT OR Apache-2.0` | <https://github.com/djc/rustc-version-rs> |
| `rustc-hash` | `1.1.0` | `Apache-2.0/MIT` | <https://github.com/rust-lang-nursery/rustc-hash> |
| `rustc-hash` | `2.1.3` | `Apache-2.0 OR MIT` | <https://github.com/rust-lang/rustc-hash> |
| `rustix` | `0.38.44` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | <https://github.com/bytecodealliance/rustix> |
| `rustix` | `1.1.6` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | <https://github.com/bytecodealliance/rustix> |
| `rustversion` | `1.0.23` | `MIT OR Apache-2.0` | <https://github.com/dtolnay/rustversion> |
| `rusty-xinput` | `1.3.0` | `Zlib OR Apache-2.0 OR MIT` | <https://github.com/Lokathor/rusty-xinput> |
| `same-file` | `1.0.6` | `Unlicense/MIT` | <https://github.com/BurntSushi/same-file> |
| `schannel` | `0.1.27` | `MIT` | <https://github.com/steffengy/schannel-rs> |
| `scoped-tls` | `1.0.1` | `MIT/Apache-2.0` | <https://github.com/alexcrichton/scoped-tls> |
| `scopeguard` | `1.2.0` | `MIT OR Apache-2.0` | <https://github.com/bluss/scopeguard> |
| `secret-service` | `4.0.0` | `MIT OR Apache-2.0` | <https://github.com/hwchen/secret-service-rs.git> |
| `security-framework` | `2.11.1` | `MIT OR Apache-2.0` | <https://github.com/kornelski/rust-security-framework> |
| `security-framework` | `3.7.0` | `MIT OR Apache-2.0` | <https://github.com/kornelski/rust-security-framework> |
| `security-framework-sys` | `2.17.0` | `MIT OR Apache-2.0` | <https://github.com/kornelski/rust-security-framework> |
| `semver` | `1.0.28` | `MIT OR Apache-2.0` | <https://github.com/dtolnay/semver> |
| `serde` | `1.0.229` | `MIT OR Apache-2.0` | <https://github.com/serde-rs/serde> |
| `serde_core` | `1.0.229` | `MIT OR Apache-2.0` | <https://github.com/serde-rs/serde> |
| `serde_derive` | `1.0.229` | `MIT OR Apache-2.0` | <https://github.com/serde-rs/serde> |
| `serde_json` | `1.0.154` | `MIT OR Apache-2.0` | <https://github.com/serde-rs/json> |
| `serde_repr` | `0.1.21` | `MIT OR Apache-2.0` | <https://github.com/dtolnay/serde-repr> |
| `serde_spanned` | `0.6.9` | `MIT OR Apache-2.0` | <https://github.com/toml-rs/toml> |
| `sevenz-rust2` | `0.25.0` | `Apache-2.0` | <https://github.com/hasenbanck/sevenz-rust> |
| `sha1` | `0.10.7` | `MIT OR Apache-2.0` | <https://github.com/RustCrypto/hashes> |
| `sha2` | `0.10.9` | `MIT OR Apache-2.0` | <https://github.com/RustCrypto/hashes> |
| `shlex` | `1.3.0` | `MIT OR Apache-2.0` | <https://github.com/comex/rust-shlex> |
| `shlex` | `2.0.1` | `MIT OR Apache-2.0` | <https://github.com/comex/rust-shlex> |
| `signal-hook-registry` | `1.4.8` | `MIT OR Apache-2.0` | <https://github.com/vorner/signal-hook> |
| `simd_cesu8` | `1.2.0` | `Apache-2.0 OR MIT` | <https://github.com/seancroach/simd_cesu8> |
| `simd-adler32` | `0.3.10` | `MIT` | <https://github.com/mcountryman/simd-adler32> |
| `simdutf8` | `0.1.5` | `MIT OR Apache-2.0` | <https://github.com/rusticstuff/simdutf8> |
| `slab` | `0.4.12` | `MIT` | <https://github.com/tokio-rs/slab> |
| `slotmap` | `1.1.1` | `Zlib` | <https://github.com/orlp/slotmap> |
| `smallvec` | `1.16.5` | `MIT OR Apache-2.0` | <https://github.com/servo/rust-smallvec> |
| `smithay-client-toolkit` | `0.19.2` | `MIT` | <https://github.com/smithay/client-toolkit> |
| `smithay-client-toolkit` | `0.20.0` | `MIT` | <https://github.com/smithay/client-toolkit> |
| `smithay-clipboard` | `0.7.3` | `MIT` | <https://github.com/smithay/smithay-clipboard> |
| `smol_str` | `0.2.2` | `MIT OR Apache-2.0` | <https://github.com/rust-analyzer/smol_str> |
| `softbuffer` | `0.4.8` | `MIT OR Apache-2.0` | <https://github.com/rust-windowing/softbuffer> |
| `spirv` | `0.3.0+sdk-1.3.268.0` | `Apache-2.0` | <https://github.com/gfx-rs/rspirv> |
| `sptr` | `0.3.2` | `MIT OR Apache-2.0` | <https://github.com/Gankra/sptr> |
| `stable_deref_trait` | `1.2.1` | `MIT OR Apache-2.0` | <https://github.com/storyyeller/stable_deref_trait> |
| `static_assertions` | `1.1.0` | `MIT OR Apache-2.0` | <https://github.com/nvzqz/static-assertions-rs> |
| `strum` | `0.26.3` | `MIT` | <https://github.com/Peternator7/strum> |
| `strum_macros` | `0.26.4` | `MIT` | <https://github.com/Peternator7/strum> |
| `subtle` | `2.6.1` | `BSD-3-Clause` | <https://github.com/dalek-cryptography/subtle> |
| `syn` | `2.0.119` | `MIT OR Apache-2.0` | <https://github.com/dtolnay/syn> |
| `syn` | `3.0.10` | `MIT OR Apache-2.0` | <https://github.com/dtolnay/syn> |
| `synstructure` | `0.13.2` | `MIT` | <https://github.com/mystor/synstructure> |
| `synstructure` | `0.14.0` | `MIT` | <https://github.com/mystor/synstructure> |
| `tempfile` | `3.27.0` | `MIT OR Apache-2.0` | <https://github.com/Stebalien/tempfile> |
| `termcolor` | `1.4.1` | `Unlicense OR MIT` | <https://github.com/BurntSushi/termcolor> |
| `thiserror` | `1.0.69` | `MIT OR Apache-2.0` | <https://github.com/dtolnay/thiserror> |
| `thiserror` | `2.0.21` | `MIT OR Apache-2.0` | <https://github.com/dtolnay/thiserror> |
| `thiserror-impl` | `1.0.69` | `MIT OR Apache-2.0` | <https://github.com/dtolnay/thiserror> |
| `thiserror-impl` | `2.0.21` | `MIT OR Apache-2.0` | <https://github.com/dtolnay/thiserror> |
| `tiff` | `0.11.3` | `MIT` | <https://github.com/image-rs/image-tiff> |
| `tinystr` | `0.8.5` | `Unicode-3.0` | <https://github.com/unicode-org/icu4x> |
| `tiny-xlib` | `0.2.5` | `MIT OR Apache-2.0 OR Zlib` | <https://github.com/rust-windowing/tiny-xlib> |
| `tokio` | `1.54.1` | `MIT` | <https://github.com/tokio-rs/tokio> |
| `toml` | `0.8.23` | `MIT OR Apache-2.0` | <https://github.com/toml-rs/toml> |
| `toml_datetime` | `0.6.11` | `MIT OR Apache-2.0` | <https://github.com/toml-rs/toml> |
| `toml_datetime` | `1.1.3+spec-1.1.0` | `MIT OR Apache-2.0` | <https://github.com/toml-rs/toml> |
| `toml_edit` | `0.22.27` | `MIT OR Apache-2.0` | <https://github.com/toml-rs/toml> |
| `toml_edit` | `0.25.19+spec-1.1.0` | `MIT OR Apache-2.0` | <https://github.com/toml-rs/toml> |
| `toml_parser` | `1.1.5+spec-1.1.0` | `MIT OR Apache-2.0` | <https://github.com/toml-rs/toml> |
| `toml_write` | `0.1.2` | `MIT OR Apache-2.0` | <https://github.com/toml-rs/toml> |
| `tracing` | `0.1.44` | `MIT` | <https://github.com/tokio-rs/tracing> |
| `tracing-attributes` | `0.1.31` | `MIT` | <https://github.com/tokio-rs/tracing> |
| `tracing-core` | `0.1.36` | `MIT` | <https://github.com/tokio-rs/tracing> |
| `ttf-parser` | `0.25.1` | `MIT OR Apache-2.0` | <https://github.com/harfbuzz/ttf-parser> |
| `twox-hash` | `2.1.5` | `MIT` | <https://github.com/shepmaster/twox-hash> |
| `type-map` | `0.5.1` | `MIT/Apache-2.0` | <https://github.com/kardeiz/type-map> |
| `typenum` | `1.20.1` | `MIT OR Apache-2.0` | <https://github.com/paholg/typenum> |
| `uds_windows` | `1.2.1` | `MIT` | <https://github.com/haraldh/rust_uds_windows> |
| `unicode-ident` | `1.0.27` | `(MIT OR Apache-2.0) AND Unicode-3.0` | <https://github.com/dtolnay/unicode-ident> |
| `unicode-segmentation` | `1.13.3` | `MIT OR Apache-2.0` | <https://github.com/unicode-rs/unicode-segmentation> |
| `unicode-width` | `0.1.14` | `MIT OR Apache-2.0` | <https://github.com/unicode-rs/unicode-width> |
| `unicode-xid` | `0.2.6` | `MIT OR Apache-2.0` | <https://github.com/unicode-rs/unicode-xid> |
| `ureq` | `2.12.1` | `MIT OR Apache-2.0` | <https://github.com/algesten/ureq> |
| `url` | `2.5.8` | `MIT OR Apache-2.0` | <https://github.com/servo/rust-url> |
| `urlencoding` | `2.1.3` | `MIT` | <https://github.com/kornelski/rust_urlencoding> |
| `utf_types` | `1.0.0` | `Apache-2.0 OR MIT` | <https://github.com/hsivonen/utf_types> |
| `utf8_iter` | `1.1.0` | `Apache-2.0 OR MIT` | <https://github.com/hsivonen/utf8_iter> |
| `uuid` | `1.29.2` | `Apache-2.0 OR MIT` | <https://github.com/uuid-rs/uuid> |
| `vcpkg` | `0.2.15` | `MIT/Apache-2.0` | <https://github.com/mcgoo/vcpkg-rs> |
| `vec_map` | `0.8.2` | `MIT/Apache-2.0` | <https://github.com/contain-rs/vec-map> |
| `version_check` | `0.9.5` | `MIT/Apache-2.0` | <https://github.com/SergioBenitez/version_check> |
| `walkdir` | `2.5.0` | `Unlicense/MIT` | <https://github.com/BurntSushi/walkdir> |
| `wasi` | `0.11.1+wasi-snapshot-preview1` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | <https://github.com/bytecodealliance/wasi> |
| `wasip2` | `1.0.4+wasi-0.2.12` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | <https://github.com/bytecodealliance/wasi-rs> |
| `wasm-bindgen` | `0.2.130` | `MIT OR Apache-2.0` | <https://github.com/wasm-bindgen/wasm-bindgen> |
| `wasm-bindgen-futures` | `0.4.80` | `MIT OR Apache-2.0` | <https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/futures> |
| `wasm-bindgen-macro` | `0.2.130` | `MIT OR Apache-2.0` | <https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/macro> |
| `wasm-bindgen-macro-support` | `0.2.130` | `MIT OR Apache-2.0` | <https://github.com/wasm-bindgen/wasm-bindgen/tree/main/crates/macro-support> |
| `wasm-bindgen-shared` | `0.2.130` | `MIT OR Apache-2.0` | <https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/shared> |
| `wayland-backend` | `0.3.17` | `MIT` | <https://github.com/smithay/wayland-rs> |
| `wayland-client` | `0.31.15` | `MIT` | <https://github.com/smithay/wayland-rs> |
| `wayland-csd-frame` | `0.3.0` | `MIT` | <https://github.com/rust-windowing/wayland-csd-frame> |
| `wayland-cursor` | `0.31.14` | `MIT` | <https://github.com/smithay/wayland-rs> |
| `wayland-protocols` | `0.32.13` | `MIT` | <https://github.com/smithay/wayland-rs> |
| `wayland-protocols-experimental` | `20250721.0.1` | `MIT` | <https://github.com/smithay/wayland-rs> |
| `wayland-protocols-misc` | `0.3.12` | `MIT` | <https://github.com/smithay/wayland-rs> |
| `wayland-protocols-plasma` | `0.3.12` | `MIT` | <https://github.com/smithay/wayland-rs> |
| `wayland-protocols-wlr` | `0.3.12` | `MIT` | <https://github.com/smithay/wayland-rs> |
| `wayland-scanner` | `0.31.11` | `MIT` | <https://github.com/smithay/wayland-rs> |
| `wayland-sys` | `0.31.11` | `MIT` | <https://github.com/smithay/wayland-rs> |
| `webbrowser` | `1.2.7` | `MIT OR Apache-2.0` | <https://github.com/amodm/webbrowser-rs> |
| `web-sys` | `0.3.107` | `MIT OR Apache-2.0` | <https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/web-sys> |
| `web-time` | `1.1.0` | `MIT OR Apache-2.0` | <https://github.com/daxpedda/web-time> |
| `weezl` | `0.1.12` | `MIT OR Apache-2.0` | <https://github.com/image-rs/weezl> |
| `wgpu` | `24.0.5` | `MIT OR Apache-2.0` | <https://github.com/gfx-rs/wgpu> |
| `wgpu-core` | `24.0.5` | `MIT OR Apache-2.0` | <https://github.com/gfx-rs/wgpu> |
| `wgpu-hal` | `24.0.4` | `MIT OR Apache-2.0` | <https://github.com/gfx-rs/wgpu> |
| `wgpu-types` | `24.0.0` | `MIT OR Apache-2.0` | <https://github.com/gfx-rs/wgpu> |
| `winapi` | `0.3.9` | `MIT/Apache-2.0` | <https://github.com/retep998/winapi-rs> |
| `winapi-i686-pc-windows-gnu` | `0.4.0` | `MIT/Apache-2.0` | <https://github.com/retep998/winapi-rs> |
| `winapi-util` | `0.1.11` | `Unlicense OR MIT` | <https://github.com/BurntSushi/winapi-util> |
| `winapi-x86_64-pc-windows-gnu` | `0.4.0` | `MIT/Apache-2.0` | <https://github.com/retep998/winapi-rs> |
| `windows` | `0.54.0` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows` | `0.58.0` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_aarch64_gnullvm` | `0.42.2` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_aarch64_gnullvm` | `0.48.5` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_aarch64_gnullvm` | `0.52.6` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_aarch64_gnullvm` | `0.53.1` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_aarch64_msvc` | `0.42.2` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_aarch64_msvc` | `0.48.5` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_aarch64_msvc` | `0.52.6` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_aarch64_msvc` | `0.53.1` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_i686_gnu` | `0.42.2` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_i686_gnu` | `0.48.5` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_i686_gnu` | `0.52.6` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_i686_gnu` | `0.53.1` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_i686_gnullvm` | `0.52.6` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_i686_gnullvm` | `0.53.1` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_i686_msvc` | `0.42.2` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_i686_msvc` | `0.48.5` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_i686_msvc` | `0.52.6` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_i686_msvc` | `0.53.1` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_x86_64_gnu` | `0.42.2` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_x86_64_gnu` | `0.48.5` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_x86_64_gnu` | `0.52.6` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_x86_64_gnu` | `0.53.1` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_x86_64_gnullvm` | `0.42.2` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_x86_64_gnullvm` | `0.48.5` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_x86_64_gnullvm` | `0.52.6` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_x86_64_gnullvm` | `0.53.1` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_x86_64_msvc` | `0.42.2` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_x86_64_msvc` | `0.48.5` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_x86_64_msvc` | `0.52.6` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows_x86_64_msvc` | `0.53.1` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows-core` | `0.54.0` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows-core` | `0.58.0` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows-implement` | `0.58.0` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows-interface` | `0.58.0` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows-link` | `0.2.1` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows-result` | `0.1.2` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows-result` | `0.2.0` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows-strings` | `0.1.0` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows-sys` | `0.45.0` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows-sys` | `0.48.0` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows-sys` | `0.52.0` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows-sys` | `0.59.0` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows-sys` | `0.60.2` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows-sys` | `0.61.2` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows-targets` | `0.42.2` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows-targets` | `0.48.5` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows-targets` | `0.52.6` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `windows-targets` | `0.53.5` | `MIT OR Apache-2.0` | <https://github.com/microsoft/windows-rs> |
| `winit` | `0.30.13` | `Apache-2.0` | <https://github.com/rust-windowing/winit> |
| `winnow` | `0.7.15` | `MIT` | <https://github.com/winnow-rs/winnow> |
| `winnow` | `1.0.5` | `MIT` | <https://github.com/winnow-rs/winnow> |
| `wit-bindgen` | `0.57.1` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | <https://github.com/bytecodealliance/wit-bindgen> |
| `writeable` | `0.6.4` | `Unicode-3.0` | <https://github.com/unicode-org/icu4x> |
| `x11-dl` | `2.21.0` | `MIT` | <https://github.com/AltF02/x11-rs.git> |
| `x11rb` | `0.13.2` | `MIT OR Apache-2.0` | <https://github.com/psychon/x11rb> |
| `x11rb-protocol` | `0.13.2` | `MIT OR Apache-2.0` | <https://github.com/psychon/x11rb> |
| `xcursor` | `0.3.11` | `MIT` | <https://github.com/esposm03/xcursor-rs> |
| `xdg-home` | `1.3.0` | `MIT` | <https://github.com/zeenix/xdg-home> |
| `xkbcommon-dl` | `0.4.2` | `MIT` | <https://github.com/rust-windowing/xkbcommon-dl> |
| `xkeysym` | `0.2.1` | `MIT OR Apache-2.0 OR Zlib` | <https://github.com/notgull/xkeysym> |
| `xml-rs` | `0.8.29` | `MIT` | <https://github.com/kornelski/xml-rs> |
| `yoke` | `0.8.4` | `Unicode-3.0` | <https://github.com/unicode-org/icu4x> |
| `yoke-derive` | `0.8.4` | `Unicode-3.0` | <https://github.com/unicode-org/icu4x> |
| `zbus` | `4.4.0` | `MIT` | <https://github.com/dbus2/zbus/> |
| `zbus` | `5.19.0` | `MIT` | <https://github.com/z-galaxy/zbus/> |
| `zbus_macros` | `4.4.0` | `MIT` | <https://github.com/dbus2/zbus/> |
| `zbus_macros` | `5.19.0` | `MIT` | <https://github.com/z-galaxy/zbus/> |
| `zbus_names` | `3.0.0` | `MIT` | <https://github.com/dbus2/zbus/> |
| `zbus_names` | `4.3.5` | `MIT` | <https://github.com/z-galaxy/zbus/> |
| `zcheapstr` | `1.2.0` | `MIT` | <https://github.com/z-galaxy/zcheapstr/> |
| `zerocopy` | `0.7.35` | `BSD-2-Clause OR Apache-2.0 OR MIT` | <https://github.com/google/zerocopy> |
| `zerocopy` | `0.8.63` | `BSD-2-Clause OR Apache-2.0 OR MIT` | <https://github.com/google/zerocopy> |
| `zerocopy-derive` | `0.7.35` | `BSD-2-Clause OR Apache-2.0 OR MIT` | <https://github.com/google/zerocopy> |
| `zerocopy-derive` | `0.8.63` | `BSD-2-Clause OR Apache-2.0 OR MIT` | <https://github.com/google/zerocopy> |
| `zerofrom` | `0.1.8` | `Unicode-3.0` | <https://github.com/unicode-org/icu4x> |
| `zerofrom-derive` | `0.1.8` | `Unicode-3.0` | <https://github.com/unicode-org/icu4x> |
| `zeroize` | `1.9.1` | `Apache-2.0 OR MIT` | <https://github.com/RustCrypto/utils> |
| `zeroize_derive` | `1.5.0` | `Apache-2.0 OR MIT` | <https://github.com/RustCrypto/utils> |
| `zerotrie` | `0.2.5` | `Unicode-3.0` | <https://github.com/unicode-org/icu4x> |
| `zerovec` | `0.11.9` | `Unicode-3.0` | <https://github.com/unicode-org/icu4x> |
| `zerovec-derive` | `0.11.7` | `Unicode-3.0` | <https://github.com/unicode-org/icu4x> |
| `zip` | `2.6.1` | `MIT` | <https://github.com/zip-rs/zip2.git> |
| `zlib-rs` | `0.6.8` | `Zlib` | <https://github.com/trifectatechfoundation/zlib-rs> |
| `zmij` | `1.0.23` | `MIT` | <https://github.com/dtolnay/zmij> |
| `zopfli` | `0.8.4` | `Apache-2.0` | <https://github.com/zopfli-rs/zopfli> |
| `zune-core` | `0.5.3` | `MIT OR Apache-2.0 OR Zlib` | <https://github.com/etemesi254/zune-image> |
| `zune-jpeg` | `0.5.15` | `MIT OR Apache-2.0 OR Zlib` | <https://github.com/etemesi254/zune-image/tree/dev/crates/zune-jpeg> |
| `zvariant` | `4.2.0` | `MIT` | <https://github.com/dbus2/zbus/> |
| `zvariant` | `5.15.1` | `MIT` | <https://github.com/z-galaxy/zbus/> |
| `zvariant_derive` | `4.2.0` | `MIT` | <https://github.com/dbus2/zbus/> |
| `zvariant_derive` | `5.15.1` | `MIT` | <https://github.com/z-galaxy/zbus/> |
| `zvariant_utils` | `2.1.0` | `MIT` | <https://github.com/dbus2/zbus/> |
| `zvariant_utils` | `4.2.0` | `MIT` | <https://github.com/z-galaxy/zbus/> |

