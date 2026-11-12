//! Libretro core implementation (`libretro.h` v1 C ABI) for Rustboy Advance.

pub mod ffi;
pub mod options;
pub mod state;
pub mod video;

use core::ffi::{c_char, c_uint, c_void};
use core::ptr;
use std::panic::{AssertUnwindSafe, catch_unwind};

use gba_core::{GbaCore, HostFrontend, LCD_HEIGHT, LCD_WIDTH, SAMPLE_RATE_HZ};

use crate::ffi::{
    RETRO_API_VERSION, RETRO_REGION_NTSC, retro_audio_sample_batch_t, retro_audio_sample_t,
    retro_environment_t, retro_game_geometry, retro_game_info, retro_input_poll_t,
    retro_input_state_t, retro_system_av_info, retro_system_info, retro_system_timing,
    retro_video_refresh_t,
};

const LIBRARY_VERSION_Z: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");
const GBA_FPS: f64 = 59.7275;
const GBA_ASPECT_RATIO: f32 = 3.0 / 2.0;

pub struct LibretroAdapter {
    pub xrgb1555: [u16; LCD_WIDTH * LCD_HEIGHT],
    pub audio: Vec<i16>,
    pub keyinput: u16,
}

impl LibretroAdapter {
    pub fn new() -> Self {
        Self {
            xrgb1555: [0x7FFF; LCD_WIDTH * LCD_HEIGHT],
            audio: Vec::with_capacity(2048),
            keyinput: 0x03FF,
        }
    }

    pub fn run_frame(&mut self, core: &mut GbaCore) {
        self.audio.clear();
        core.run_frame(self);
    }
}

impl Default for LibretroAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl HostFrontend for LibretroAdapter {
    fn present_frame(&mut self, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
        for (dst, &px) in self.xrgb1555.iter_mut().zip(fb.iter()) {
            // Convert BGR555 -> RGB555 (0RRR RRGG GGGB BBBB)
            let r = px & 0x1F;
            let g = (px >> 5) & 0x1F;
            let b = (px >> 10) & 0x1F;
            *dst = (r << 10) | (g << 5) | b;
        }
    }

    fn push_audio_samples(&mut self, samples: &[i16]) {
        self.audio.extend_from_slice(samples);
    }

    fn poll_keypad(&mut self) -> u16 {
        self.keyinput
    }
}

fn ffi_guard<T>(default: T, f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(default)
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_api_version() -> c_uint {
    RETRO_API_VERSION
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_init() {
    ffi_guard((), state::init);
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_deinit() {
    ffi_guard((), state::deinit);
}

/// # Safety
/// `info` must be null or point to a valid, writable `retro_system_info`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_get_system_info(info: *mut retro_system_info) {
    ffi_guard((), || {
        if info.is_null() {
            return;
        }
        unsafe {
            *info = retro_system_info {
                library_name: c"Rustboy Advance".as_ptr(),
                library_version: LIBRARY_VERSION_Z.as_ptr().cast::<c_char>(),
                valid_extensions: c"gba|agb|bin".as_ptr(),
                need_fullpath: false,
                block_extract: false,
            };
        }
    });
}

/// # Safety
/// `info` must be null or point to a valid, writable `retro_system_av_info`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_get_system_av_info(info: *mut retro_system_av_info) {
    ffi_guard((), || {
        if info.is_null() {
            return;
        }
        unsafe {
            *info = retro_system_av_info {
                geometry: retro_game_geometry {
                    base_width: LCD_WIDTH as c_uint,
                    base_height: LCD_HEIGHT as c_uint,
                    max_width: LCD_WIDTH as c_uint,
                    max_height: LCD_HEIGHT as c_uint,
                    aspect_ratio: GBA_ASPECT_RATIO,
                },
                timing: retro_system_timing {
                    fps: GBA_FPS,
                    sample_rate: SAMPLE_RATE_HZ as f64,
                },
            };
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_set_environment(cb: retro_environment_t) {
    ffi_guard((), || state::set_environment(cb));
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_set_video_refresh(cb: retro_video_refresh_t) {
    ffi_guard((), || state::set_video_refresh(cb));
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_set_audio_sample(cb: retro_audio_sample_t) {
    ffi_guard((), || state::set_audio_sample(cb));
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_set_audio_sample_batch(cb: retro_audio_sample_batch_t) {
    ffi_guard((), || state::set_audio_sample_batch(cb));
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_set_input_poll(cb: retro_input_poll_t) {
    ffi_guard((), || state::set_input_poll(cb));
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_set_input_state(cb: retro_input_state_t) {
    ffi_guard((), || state::set_input_state(cb));
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_set_controller_port_device(port: c_uint, device: c_uint) {
    ffi_guard((), || state::set_controller_port_device(port, device));
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_reset() {
    ffi_guard((), state::reset);
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_run() {
    ffi_guard((), state::run);
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_serialize_size() -> usize {
    ffi_guard(0, state::serialize_size)
}

/// # Safety
/// `data` must be null or point to at least `size` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_serialize(data: *mut c_void, size: usize) -> bool {
    ffi_guard(false, || unsafe { state::serialize(data, size) })
}

/// # Safety
/// `data` must be null or point to at least `size` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_unserialize(data: *const c_void, size: usize) -> bool {
    ffi_guard(false, || unsafe { state::unserialize(data, size) })
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_cheat_reset() {}

/// # Safety
/// `code` must be null or point to a valid NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_cheat_set(
    _index: c_uint,
    _enabled: bool,
    _code: *const c_char,
) {
}

/// # Safety
/// `game` must be null or point to a valid `retro_game_info` struct.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_load_game(game: *const retro_game_info) -> bool {
    ffi_guard(false, || unsafe { state::load_game(game) })
}

/// # Safety
/// `info` must be null or point to `num_info` valid `retro_game_info` elements.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_load_game_special(
    _game_type: c_uint,
    _info: *const retro_game_info,
    _num_info: usize,
) -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_unload_game() {
    ffi_guard((), state::unload_game);
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_get_region() -> c_uint {
    RETRO_REGION_NTSC
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_get_memory_data(id: c_uint) -> *mut c_void {
    ffi_guard(ptr::null_mut(), || state::get_memory_data(id))
}

#[unsafe(no_mangle)]
pub extern "C" fn retro_get_memory_size(id: c_uint) -> usize {
    ffi_guard(0, || state::get_memory_size(id))
}
