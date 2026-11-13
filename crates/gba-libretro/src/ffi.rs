//! Hand-written FFI bindings for `libretro.h` (API version 1).
//!
//! Covers the core C ABI structs, constants, environment commands, and callback
//! function pointer types needed to expose Rustboy Advance as a libretro core.

#![allow(non_camel_case_types)]
#![allow(dead_code)]

use core::ffi::{c_char, c_int, c_uint, c_void};

/// Libretro API version implemented by this core.
pub const RETRO_API_VERSION: c_uint = 1;

// -----------------------------------------------------------------------------
// Regions
// -----------------------------------------------------------------------------
pub const RETRO_REGION_NTSC: c_uint = 0;
pub const RETRO_REGION_PAL: c_uint = 1;

// -----------------------------------------------------------------------------
// Controller / Input devices & RetroPad button IDs
// -----------------------------------------------------------------------------
pub const RETRO_DEVICE_MASK: c_uint = 0xFF;
pub const RETRO_DEVICE_NONE: c_uint = 0;
pub const RETRO_DEVICE_JOYPAD: c_uint = 1;
pub const RETRO_DEVICE_MOUSE: c_uint = 2;
pub const RETRO_DEVICE_KEYBOARD: c_uint = 3;
pub const RETRO_DEVICE_LIGHTGUN: c_uint = 4;
pub const RETRO_DEVICE_ANALOG: c_uint = 5;
pub const RETRO_DEVICE_POINTER: c_uint = 6;

pub const RETRO_DEVICE_ID_JOYPAD_B: c_uint = 0;
pub const RETRO_DEVICE_ID_JOYPAD_Y: c_uint = 1;
pub const RETRO_DEVICE_ID_JOYPAD_SELECT: c_uint = 2;
pub const RETRO_DEVICE_ID_JOYPAD_START: c_uint = 3;
pub const RETRO_DEVICE_ID_JOYPAD_UP: c_uint = 4;
pub const RETRO_DEVICE_ID_JOYPAD_DOWN: c_uint = 5;
pub const RETRO_DEVICE_ID_JOYPAD_LEFT: c_uint = 6;
pub const RETRO_DEVICE_ID_JOYPAD_RIGHT: c_uint = 7;
pub const RETRO_DEVICE_ID_JOYPAD_A: c_uint = 8;
pub const RETRO_DEVICE_ID_JOYPAD_X: c_uint = 9;
pub const RETRO_DEVICE_ID_JOYPAD_L: c_uint = 10;
pub const RETRO_DEVICE_ID_JOYPAD_R: c_uint = 11;
pub const RETRO_DEVICE_ID_JOYPAD_L2: c_uint = 12;
pub const RETRO_DEVICE_ID_JOYPAD_R2: c_uint = 13;
pub const RETRO_DEVICE_ID_JOYPAD_L3: c_uint = 14;
pub const RETRO_DEVICE_ID_JOYPAD_R3: c_uint = 15;
pub const RETRO_DEVICE_ID_JOYPAD_MASK: c_uint = 256;

// -----------------------------------------------------------------------------
// Pixel formats (`enum retro_pixel_format`)
// -----------------------------------------------------------------------------
pub type retro_pixel_format = c_int;
pub const RETRO_PIXEL_FORMAT_0RGB1555: retro_pixel_format = 0;
pub const RETRO_PIXEL_FORMAT_XRGB8888: retro_pixel_format = 1;
pub const RETRO_PIXEL_FORMAT_RGB565: retro_pixel_format = 2;

// -----------------------------------------------------------------------------
// Memory regions
// -----------------------------------------------------------------------------
pub const RETRO_MEMORY_MASK: c_uint = 0xFF;
pub const RETRO_MEMORY_SAVE_RAM: c_uint = 0;
pub const RETRO_MEMORY_RTC: c_uint = 1;
pub const RETRO_MEMORY_SYSTEM_RAM: c_uint = 2;
pub const RETRO_MEMORY_VIDEO_RAM: c_uint = 3;

// -----------------------------------------------------------------------------
// Memory descriptor flags (`RETRO_MEMDESC_*`)
// -----------------------------------------------------------------------------
pub const RETRO_MEMDESC_CONST: u64 = 1 << 0;
pub const RETRO_MEMDESC_BIGENDIAN: u64 = 1 << 1;
pub const RETRO_MEMDESC_SYSTEM_RAM: u64 = 1 << 2;
pub const RETRO_MEMDESC_SAVE_RAM: u64 = 1 << 3;
pub const RETRO_MEMDESC_VIDEO_RAM: u64 = 1 << 4;
pub const RETRO_MEMDESC_ALIGN_2: u64 = 1 << 16;
pub const RETRO_MEMDESC_ALIGN_4: u64 = 2 << 16;
pub const RETRO_MEMDESC_ALIGN_8: u64 = 3 << 16;
pub const RETRO_MEMDESC_MINSIZE_2: u64 = 1 << 24;
pub const RETRO_MEMDESC_MINSIZE_4: u64 = 2 << 24;
pub const RETRO_MEMDESC_MINSIZE_8: u64 = 3 << 24;

// -----------------------------------------------------------------------------
// Rumble (`enum retro_rumble_effect`)
// -----------------------------------------------------------------------------
pub type retro_rumble_effect = c_int;
pub const RETRO_RUMBLE_STRONG: retro_rumble_effect = 0;
pub const RETRO_RUMBLE_WEAK: retro_rumble_effect = 1;

// -----------------------------------------------------------------------------
// Logging (`enum retro_log_level`)
// -----------------------------------------------------------------------------
pub type retro_log_level = c_int;
pub const RETRO_LOG_DEBUG: retro_log_level = 0;
pub const RETRO_LOG_INFO: retro_log_level = 1;
pub const RETRO_LOG_WARN: retro_log_level = 2;
pub const RETRO_LOG_ERROR: retro_log_level = 3;

// -----------------------------------------------------------------------------
// Environment commands (`RETRO_ENVIRONMENT_*`)
// -----------------------------------------------------------------------------
pub const RETRO_ENVIRONMENT_EXPERIMENTAL: c_uint = 0x10000;
pub const RETRO_ENVIRONMENT_PRIVATE: c_uint = 0x20000;

pub const RETRO_ENVIRONMENT_SET_ROTATION: c_uint = 1;
pub const RETRO_ENVIRONMENT_GET_OVERSCAN: c_uint = 2;
pub const RETRO_ENVIRONMENT_GET_CAN_DUPE: c_uint = 3;
pub const RETRO_ENVIRONMENT_SET_MESSAGE: c_uint = 6;
pub const RETRO_ENVIRONMENT_SHUTDOWN: c_uint = 7;
pub const RETRO_ENVIRONMENT_SET_PERFORMANCE_LEVEL: c_uint = 8;
pub const RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY: c_uint = 9;
pub const RETRO_ENVIRONMENT_SET_PIXEL_FORMAT: c_uint = 10;
pub const RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS: c_uint = 11;
pub const RETRO_ENVIRONMENT_SET_KEYBOARD_CALLBACK: c_uint = 12;
pub const RETRO_ENVIRONMENT_SET_DISK_CONTROL_INTERFACE: c_uint = 13;
pub const RETRO_ENVIRONMENT_SET_HW_RENDER: c_uint = 14;
pub const RETRO_ENVIRONMENT_GET_VARIABLE: c_uint = 15;
pub const RETRO_ENVIRONMENT_SET_VARIABLES: c_uint = 16;
pub const RETRO_ENVIRONMENT_GET_VARIABLE_UPDATE: c_uint = 17;
pub const RETRO_ENVIRONMENT_SET_SUPPORT_NO_GAME: c_uint = 18;
pub const RETRO_ENVIRONMENT_GET_LIBRETRO_PATH: c_uint = 19;
pub const RETRO_ENVIRONMENT_SET_FRAME_TIME_CALLBACK: c_uint = 21;
pub const RETRO_ENVIRONMENT_SET_AUDIO_CALLBACK: c_uint = 22;
pub const RETRO_ENVIRONMENT_GET_RUMBLE_INTERFACE: c_uint = 23;
pub const RETRO_ENVIRONMENT_GET_INPUT_DEVICE_CAPABILITIES: c_uint = 24;
pub const RETRO_ENVIRONMENT_GET_SENSOR_INTERFACE: c_uint = 25 | RETRO_ENVIRONMENT_EXPERIMENTAL;
pub const RETRO_ENVIRONMENT_GET_CAMERA_INTERFACE: c_uint = 26 | RETRO_ENVIRONMENT_EXPERIMENTAL;
pub const RETRO_ENVIRONMENT_GET_LOG_INTERFACE: c_uint = 27;
pub const RETRO_ENVIRONMENT_GET_PERF_INTERFACE: c_uint = 28;
pub const RETRO_ENVIRONMENT_GET_LOCATION_INTERFACE: c_uint = 29;
pub const RETRO_ENVIRONMENT_GET_CONTENT_DIRECTORY: c_uint = 30;
pub const RETRO_ENVIRONMENT_GET_SAVE_DIRECTORY: c_uint = 31;
pub const RETRO_ENVIRONMENT_SET_SYSTEM_AV_INFO: c_uint = 32;
pub const RETRO_ENVIRONMENT_SET_PROC_ADDRESS_CALLBACK: c_uint = 33;
pub const RETRO_ENVIRONMENT_SET_SUBSYSTEM_INFO: c_uint = 34;
pub const RETRO_ENVIRONMENT_SET_CONTROLLER_INFO: c_uint = 35;
pub const RETRO_ENVIRONMENT_SET_MEMORY_MAPS: c_uint = 36 | RETRO_ENVIRONMENT_EXPERIMENTAL;
pub const RETRO_ENVIRONMENT_SET_GEOMETRY: c_uint = 37;
pub const RETRO_ENVIRONMENT_GET_USERNAME: c_uint = 38;
pub const RETRO_ENVIRONMENT_GET_LANGUAGE: c_uint = 39;
pub const RETRO_ENVIRONMENT_SET_SUPPORT_ACHIEVEMENTS: c_uint = 42 | RETRO_ENVIRONMENT_EXPERIMENTAL;
pub const RETRO_ENVIRONMENT_GET_INPUT_BITMASKS: c_uint = 51;

// -----------------------------------------------------------------------------
// Callback typedefs
// -----------------------------------------------------------------------------
pub type retro_environment_t = Option<unsafe extern "C" fn(cmd: c_uint, data: *mut c_void) -> bool>;
pub type retro_video_refresh_t =
    Option<unsafe extern "C" fn(data: *const c_void, width: c_uint, height: c_uint, pitch: usize)>;
pub type retro_audio_sample_t = Option<unsafe extern "C" fn(left: i16, right: i16)>;
pub type retro_audio_sample_batch_t =
    Option<unsafe extern "C" fn(data: *const i16, frames: usize) -> usize>;
pub type retro_input_poll_t = Option<unsafe extern "C" fn()>;
pub type retro_input_state_t =
    Option<unsafe extern "C" fn(port: c_uint, device: c_uint, index: c_uint, id: c_uint) -> i16>;

pub type retro_set_rumble_state_t =
    Option<unsafe extern "C" fn(port: c_uint, effect: retro_rumble_effect, strength: u16) -> bool>;
pub type retro_log_printf_t =
    Option<unsafe extern "C" fn(level: retro_log_level, fmt: *const c_char, ...)>;

// -----------------------------------------------------------------------------
// Structures
// -----------------------------------------------------------------------------
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct retro_system_info {
    pub library_name: *const c_char,
    pub library_version: *const c_char,
    pub valid_extensions: *const c_char,
    pub need_fullpath: bool,
    pub block_extract: bool,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct retro_game_geometry {
    pub base_width: c_uint,
    pub base_height: c_uint,
    pub max_width: c_uint,
    pub max_height: c_uint,
    pub aspect_ratio: f32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct retro_system_timing {
    pub fps: f64,
    pub sample_rate: f64,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct retro_system_av_info {
    pub geometry: retro_game_geometry,
    pub timing: retro_system_timing,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct retro_game_info {
    pub path: *const c_char,
    pub data: *const c_void,
    pub size: usize,
    pub meta: *const c_char,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct retro_variable {
    pub key: *const c_char,
    pub value: *const c_char,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct retro_input_descriptor {
    pub port: c_uint,
    pub device: c_uint,
    pub index: c_uint,
    pub id: c_uint,
    pub description: *const c_char,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct retro_rumble_interface {
    pub set_rumble_state: retro_set_rumble_state_t,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct retro_log_callback {
    pub log: retro_log_printf_t,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct retro_memory_descriptor {
    pub flags: u64,
    pub ptr: *mut c_void,
    pub offset: usize,
    pub start: usize,
    pub select: usize,
    pub disconnect: usize,
    pub len: usize,
    pub addrspace: *const c_char,
}

// Safety: memory descriptors are only mutated under the global core mutex and never
// shared across threads concurrently without synchronization.
unsafe impl Send for retro_memory_descriptor {}
unsafe impl Sync for retro_memory_descriptor {}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct retro_memory_map {
    pub descriptors: *const retro_memory_descriptor,
    pub num_descriptors: c_uint,
}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct retro_controller_description {
    pub desc: *const c_char,
    pub id: c_uint,
}

unsafe impl Sync for retro_controller_description {}

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct retro_controller_info {
    pub types: *const retro_controller_description,
    pub num_types: c_uint,
}

unsafe impl Sync for retro_controller_info {}
unsafe impl Sync for retro_variable {}
unsafe impl Sync for retro_input_descriptor {}
