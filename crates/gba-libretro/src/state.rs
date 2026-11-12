//! Global thread-safe state management and `gba-core` lifecycle integration.

use core::ffi::{CStr, c_char, c_uint, c_void};
use core::ptr;
use std::path::Path;
use std::sync::Mutex;

use gba_core::cartridge::SaveType;
use gba_core::{
    GbaCore, HostFrontend, KEY_A, KEY_B, KEY_DOWN, KEY_L, KEY_LEFT, KEY_R, KEY_RIGHT, KEY_SELECT,
    KEY_START, KEY_UP, LCD_HEIGHT, LCD_WIDTH, encode_keyinput,
};

use crate::ffi::{
    RETRO_DEVICE_ID_JOYPAD_A, RETRO_DEVICE_ID_JOYPAD_B, RETRO_DEVICE_ID_JOYPAD_DOWN,
    RETRO_DEVICE_ID_JOYPAD_L, RETRO_DEVICE_ID_JOYPAD_LEFT, RETRO_DEVICE_ID_JOYPAD_R,
    RETRO_DEVICE_ID_JOYPAD_RIGHT, RETRO_DEVICE_ID_JOYPAD_SELECT, RETRO_DEVICE_ID_JOYPAD_START,
    RETRO_DEVICE_ID_JOYPAD_UP, RETRO_DEVICE_JOYPAD, RETRO_DEVICE_NONE,
    RETRO_ENVIRONMENT_GET_LOG_INTERFACE, RETRO_ENVIRONMENT_GET_RUMBLE_INTERFACE,
    RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY, RETRO_ENVIRONMENT_GET_VARIABLE_UPDATE,
    RETRO_ENVIRONMENT_SET_CONTROLLER_INFO, RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS,
    RETRO_ENVIRONMENT_SET_MEMORY_MAPS, RETRO_ENVIRONMENT_SET_PIXEL_FORMAT,
    RETRO_ENVIRONMENT_SET_SUPPORT_ACHIEVEMENTS, RETRO_ENVIRONMENT_SET_SUPPORT_NO_GAME,
    RETRO_ENVIRONMENT_SET_VARIABLES, RETRO_LOG_INFO, RETRO_LOG_WARN, RETRO_MEMDESC_CONST,
    RETRO_MEMDESC_SAVE_RAM, RETRO_MEMDESC_SYSTEM_RAM, RETRO_MEMORY_MASK, RETRO_MEMORY_SAVE_RAM,
    RETRO_MEMORY_SYSTEM_RAM, RETRO_PIXEL_FORMAT_0RGB1555, RETRO_PIXEL_FORMAT_RGB565,
    RETRO_RUMBLE_STRONG, RETRO_RUMBLE_WEAK, retro_audio_sample_batch_t, retro_audio_sample_t,
    retro_environment_t, retro_game_info, retro_input_poll_t, retro_input_state_t,
    retro_log_callback, retro_log_level, retro_log_printf_t, retro_memory_descriptor,
    retro_memory_map, retro_rumble_interface, retro_set_rumble_state_t, retro_video_refresh_t,
};
use crate::options::{CONTROLLER_INFO, CORE_VARIABLES, CoreOptions, INPUT_DESCRIPTORS, read_core_options};
use crate::video::{OutputPixelFormat, convert_framebuffer};

const STATE_MAGIC: [u8; 4] = *b"RBLS";
const STATE_VERSION: u32 = 1;
const STATE_HEADER_SIZE: usize = 16;
const STATE_MARGIN_BYTES: usize = 256 * 1024;
const STATE_MIN_BOUND_BYTES: usize = 1024 * 1024;

const BIOS_SIZE: usize = 0x4000;
const EWRAM_SIZE: usize = 0x40000;
const IWRAM_SIZE: usize = 0x8000;
const SRAM_MAX_SIZE: usize = 0x20000;

static STATE: Mutex<LibretroState> = Mutex::new(LibretroState::new());

fn lock_state() -> std::sync::MutexGuard<'static, LibretroState> {
    match STATE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[derive(Copy, Clone, Default)]
pub struct LibretroCallbacks {
    pub environ: retro_environment_t,
    pub video_refresh: retro_video_refresh_t,
    pub audio_sample: retro_audio_sample_t,
    pub audio_sample_batch: retro_audio_sample_batch_t,
    pub input_poll: retro_input_poll_t,
    pub input_state: retro_input_state_t,
    pub rumble: retro_set_rumble_state_t,
    pub log: retro_log_printf_t,
}

struct LoadedCore {
    core: GbaCore,
    pixel_format: OutputPixelFormat,
    options: CoreOptions,
    frame_buf: Option<Box<[u16; LCD_WIDTH * LCD_HEIGHT]>>,
    audio_buf: Vec<i16>,
    spare_audio_buf: Vec<i16>,
    serialize_bound: usize,
    last_rumble_active: bool,
    memory_descriptors: Box<[retro_memory_descriptor; 4]>,
}

struct LibretroState {
    callbacks: LibretroCallbacks,
    port0_device: c_uint,
    loaded: Option<LoadedCore>,
}

impl LibretroState {
    const fn new() -> Self {
        Self {
            callbacks: LibretroCallbacks {
                environ: None,
                video_refresh: None,
                audio_sample: None,
                audio_sample_batch: None,
                input_poll: None,
                input_state: None,
                rumble: None,
                log: None,
            },
            port0_device: RETRO_DEVICE_JOYPAD,
            loaded: None,
        }
    }
}

fn alloc_boxed_u8_zeroed<const N: usize>() -> Box<[u8; N]> {
    vec![0u8; N]
        .into_boxed_slice()
        .try_into()
        .expect("exact-size heap byte array")
}

fn alloc_boxed_u16_zeroed<const N: usize>() -> Box<[u16; N]> {
    vec![0u16; N]
        .into_boxed_slice()
        .try_into()
        .expect("exact-size heap u16 array")
}

fn frontend_log(log_cb: retro_log_printf_t, level: retro_log_level, msg: &str) {
    let Some(cb) = log_cb else {
        return;
    };
    if let Ok(c_msg) = std::ffi::CString::new(msg) {
        unsafe {
            cb(level, c"[rustboy-advance] %s\n".as_ptr(), c_msg.as_ptr());
        }
    }
}

fn save_ram_size_for_cart(core: &GbaCore) -> usize {
    if core.bus.cart.save_type_override == Some(SaveType::None) {
        return 0;
    }
    match core.bus.cart.effective_save_type() {
        SaveType::Eeprom512B => 512,
        SaveType::Eeprom8K => 0x2000,
        SaveType::Sram32K => 0x8000,
        SaveType::Flash64K => 0x10000,
        SaveType::Flash128K => 0x20000,
        // Expose full backing buffer when save type is auto-detected as None so
        // homebrew/synthetic ROMs or fallback detection can still persist SRAM.
        SaveType::None => SRAM_MAX_SIZE,
    }
}

fn build_memory_descriptors(core: &mut GbaCore) -> [retro_memory_descriptor; 4] {
    let save_len = save_ram_size_for_cart(core);
    let empty_desc = retro_memory_descriptor {
        flags: 0,
        ptr: ptr::null_mut(),
        offset: 0,
        start: 0,
        select: 0,
        disconnect: 0,
        len: 0,
        addrspace: ptr::null(),
    };

    let ewram_desc = retro_memory_descriptor {
        flags: RETRO_MEMDESC_SYSTEM_RAM,
        ptr: core.bus.ewram.as_mut_ptr().cast(),
        offset: 0,
        start: 0x0200_0000,
        select: 0xFF00_0000,
        disconnect: 0x00FC_0000,
        len: EWRAM_SIZE,
        addrspace: ptr::null(),
    };

    let iwram_desc = retro_memory_descriptor {
        flags: RETRO_MEMDESC_SYSTEM_RAM,
        ptr: core.bus.iwram.as_mut_ptr().cast(),
        offset: 0,
        start: 0x0300_0000,
        select: 0xFF00_0000,
        disconnect: 0x00FF_8000,
        len: IWRAM_SIZE,
        addrspace: ptr::null(),
    };

    let rom_desc = retro_memory_descriptor {
        flags: RETRO_MEMDESC_CONST,
        ptr: core.bus.cart.rom.as_mut_ptr().cast(),
        offset: 0,
        start: 0x0800_0000,
        select: 0xFE00_0000,
        disconnect: 0,
        len: core.bus.cart.rom.len(),
        addrspace: ptr::null(),
    };

    let sram_desc = if save_len > 0 {
        retro_memory_descriptor {
            flags: RETRO_MEMDESC_SAVE_RAM,
            ptr: core.bus.cart.sram.as_mut_ptr().cast(),
            offset: 0,
            start: 0x0E00_0000,
            select: 0xFF00_0000,
            disconnect: 0,
            len: save_len,
            addrspace: ptr::null(),
        }
    } else {
        empty_desc
    };

    [ewram_desc, iwram_desc, rom_desc, sram_desc]
}

/// Runs `op(core)` while keeping the heap allocations of `ewram`, `iwram`, and
/// `cart.sram` at their exact original addresses so frontend memory pointers remain valid.
fn preserve_memory_buffers<E>(
    core: &mut GbaCore,
    copy_sram_in: bool,
    op: impl FnOnce(&mut GbaCore) -> Result<(), E>,
) -> Result<(), E> {
    let mut orig_ewram = alloc_boxed_u8_zeroed::<EWRAM_SIZE>();
    let mut orig_iwram = alloc_boxed_u8_zeroed::<IWRAM_SIZE>();
    let mut orig_sram = alloc_boxed_u8_zeroed::<SRAM_MAX_SIZE>();

    core::mem::swap(&mut core.bus.ewram, &mut orig_ewram);
    core::mem::swap(&mut core.bus.iwram, &mut orig_iwram);
    core::mem::swap(&mut core.bus.cart.sram, &mut orig_sram);

    if copy_sram_in {
        core.bus.cart.sram.copy_from_slice(&*orig_sram);
    }

    let result = op(core);

    if result.is_ok() {
        orig_ewram.copy_from_slice(&*core.bus.ewram);
        orig_iwram.copy_from_slice(&*core.bus.iwram);
        orig_sram.copy_from_slice(&*core.bus.cart.sram);
    }

    core::mem::swap(&mut core.bus.ewram, &mut orig_ewram);
    core::mem::swap(&mut core.bus.iwram, &mut orig_iwram);
    core::mem::swap(&mut core.bus.cart.sram, &mut orig_sram);

    core.bus.rebuild_fastmem();
    result
}

fn apply_options_to_core(core: &mut GbaCore, opts: CoreOptions) {
    core.bus.cart.solar_lux = opts.solar_lux;
    core.bus.cart.set_save_type_override(opts.save_type_override);
}

fn read_bios_from_system_dir(
    environ_cb: retro_environment_t,
    log_cb: retro_log_printf_t,
) -> Option<Vec<u8>> {
    let env = environ_cb?;
    let mut sys_dir_ptr: *const c_char = ptr::null();
    let ok = unsafe {
        env(
            RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY,
            (&raw mut sys_dir_ptr).cast(),
        )
    };
    if !ok || sys_dir_ptr.is_null() {
        return None;
    }

    let c_dir = unsafe { CStr::from_ptr(sys_dir_ptr) };
    let dir_str = c_dir.to_str().ok()?;
    if dir_str.is_empty() {
        return None;
    }

    let bios_path = Path::new(dir_str).join("gba_bios.bin");
    match std::fs::read(&bios_path) {
        Ok(bytes) if bytes.len() == BIOS_SIZE => {
            frontend_log(
                log_cb,
                RETRO_LOG_INFO,
                &format!("Loaded GBA BIOS from {}", bios_path.display()),
            );
            Some(bytes)
        }
        Ok(bytes) => {
            frontend_log(
                log_cb,
                RETRO_LOG_WARN,
                &format!(
                    "Ignoring {} (expected {} bytes, got {}); using HLE BIOS",
                    bios_path.display(),
                    BIOS_SIZE,
                    bytes.len()
                ),
            );
            None
        }
        Err(_) => {
            frontend_log(
                log_cb,
                RETRO_LOG_INFO,
                "gba_bios.bin not found in system directory; using built-in HLE BIOS",
            );
            None
        }
    }
}

fn negotiate_pixel_format(environ_cb: retro_environment_t) -> OutputPixelFormat {
    let Some(env) = environ_cb else {
        return OutputPixelFormat::Xrgb1555;
    };

    let mut fmt_565 = RETRO_PIXEL_FORMAT_RGB565;
    if unsafe { env(RETRO_ENVIRONMENT_SET_PIXEL_FORMAT, (&raw mut fmt_565).cast()) } {
        return OutputPixelFormat::Rgb565;
    }

    let mut fmt_1555 = RETRO_PIXEL_FORMAT_0RGB1555;
    let _ = unsafe { env(RETRO_ENVIRONMENT_SET_PIXEL_FORMAT, (&raw mut fmt_1555).cast()) };
    OutputPixelFormat::Xrgb1555
}

fn query_rumble_interface(environ_cb: retro_environment_t) -> retro_set_rumble_state_t {
    let env = environ_cb?;
    let mut iface = retro_rumble_interface {
        set_rumble_state: None,
    };
    let ok = unsafe {
        env(
            RETRO_ENVIRONMENT_GET_RUMBLE_INTERFACE,
            (&raw mut iface).cast(),
        )
    };
    if ok { iface.set_rumble_state } else { None }
}

fn stop_rumble(rumble_cb: retro_set_rumble_state_t, last_active: &mut bool) {
    if *last_active {
        if let Some(cb) = rumble_cb {
            unsafe {
                cb(0, RETRO_RUMBLE_STRONG, 0);
                cb(0, RETRO_RUMBLE_WEAK, 0);
            }
        }
        *last_active = false;
    }
}

struct FrameHost<'a> {
    frame_buf: &'a mut [u16; LCD_WIDTH * LCD_HEIGHT],
    audio_buf: &'a mut Vec<i16>,
    keyinput: u16,
    pixel_format: OutputPixelFormat,
    color_correction: crate::video::ColorCorrection,
    frame_presented: bool,
}

impl HostFrontend for FrameHost<'_> {
    fn present_frame(&mut self, fb_bgr555: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
        convert_framebuffer(
            fb_bgr555,
            self.frame_buf,
            self.pixel_format,
            self.color_correction,
        );
        self.frame_presented = true;
    }

    fn push_audio_samples(&mut self, samples: &[i16]) {
        self.audio_buf.extend_from_slice(samples);
    }

    fn poll_keypad(&mut self) -> u16 {
        self.keyinput
    }
}

fn poll_port0_keypad(
    input_poll: retro_input_poll_t,
    input_state: retro_input_state_t,
    port0_device: c_uint,
) -> u16 {
    if let Some(poll) = input_poll {
        unsafe { poll() };
    }
    if port0_device == RETRO_DEVICE_NONE {
        return encode_keyinput(0);
    }
    let Some(state_cb) = input_state else {
        return encode_keyinput(0);
    };

    const BUTTON_MAP: [(c_uint, u16); 10] = [
        (RETRO_DEVICE_ID_JOYPAD_A, KEY_A),
        (RETRO_DEVICE_ID_JOYPAD_B, KEY_B),
        (RETRO_DEVICE_ID_JOYPAD_SELECT, KEY_SELECT),
        (RETRO_DEVICE_ID_JOYPAD_START, KEY_START),
        (RETRO_DEVICE_ID_JOYPAD_RIGHT, KEY_RIGHT),
        (RETRO_DEVICE_ID_JOYPAD_LEFT, KEY_LEFT),
        (RETRO_DEVICE_ID_JOYPAD_UP, KEY_UP),
        (RETRO_DEVICE_ID_JOYPAD_DOWN, KEY_DOWN),
        (RETRO_DEVICE_ID_JOYPAD_R, KEY_R),
        (RETRO_DEVICE_ID_JOYPAD_L, KEY_L),
    ];

    let mut pressed = 0u16;
    for &(retro_id, gba_mask) in &BUTTON_MAP {
        let val = unsafe { state_cb(0, RETRO_DEVICE_JOYPAD, 0, retro_id) };
        if val != 0 {
            pressed |= gba_mask;
        }
    }
    encode_keyinput(pressed)
}

// -----------------------------------------------------------------------------
// Public lifecycle handlers invoked by `extern "C"` FFI exports
// -----------------------------------------------------------------------------

pub fn set_environment(cb: retro_environment_t) {
    {
        let mut state = lock_state();
        state.callbacks.environ = cb;
    }
    let Some(env) = cb else {
        return;
    };
    unsafe {
        let mut support_no_game = false;
        env(
            RETRO_ENVIRONMENT_SET_SUPPORT_NO_GAME,
            (&raw mut support_no_game).cast(),
        );
        env(
            RETRO_ENVIRONMENT_SET_VARIABLES,
            CORE_VARIABLES.as_ptr().cast_mut().cast(),
        );
        env(
            RETRO_ENVIRONMENT_SET_CONTROLLER_INFO,
            CONTROLLER_INFO.as_ptr().cast_mut().cast(),
        );
    }
}

pub fn set_video_refresh(cb: retro_video_refresh_t) {
    lock_state().callbacks.video_refresh = cb;
}

pub fn set_audio_sample(cb: retro_audio_sample_t) {
    lock_state().callbacks.audio_sample = cb;
}

pub fn set_audio_sample_batch(cb: retro_audio_sample_batch_t) {
    lock_state().callbacks.audio_sample_batch = cb;
}

pub fn set_input_poll(cb: retro_input_poll_t) {
    lock_state().callbacks.input_poll = cb;
}

pub fn set_input_state(cb: retro_input_state_t) {
    lock_state().callbacks.input_state = cb;
}

pub fn set_controller_port_device(port: c_uint, device: c_uint) {
    if port == 0 {
        lock_state().port0_device = device;
    }
}

pub fn init() {
    let environ_cb = lock_state().callbacks.environ;
    let mut log_cb: retro_log_printf_t = None;
    if let Some(env) = environ_cb {
        let mut log_iface = retro_log_callback { log: None };
        if unsafe {
            env(
                RETRO_ENVIRONMENT_GET_LOG_INTERFACE,
                (&raw mut log_iface).cast(),
            )
        } {
            log_cb = log_iface.log;
        }
    }
    lock_state().callbacks.log = log_cb;
}

pub fn deinit() {
    unload_game();
    let mut state = lock_state();
    *state = LibretroState::new();
}

/// # Safety
/// `game` must either be null or point to a valid `retro_game_info`.
pub unsafe fn load_game(game: *const retro_game_info) -> bool {
    if game.is_null() {
        return false;
    }

    let info = unsafe { &*game };
    let rom_bytes = if !info.data.is_null() && info.size > 0 {
        unsafe { core::slice::from_raw_parts(info.data.cast::<u8>(), info.size) }.to_vec()
    } else if !info.path.is_null() {
        let Ok(path_str) = unsafe { CStr::from_ptr(info.path) }.to_str() else {
            return false;
        };
        match std::fs::read(path_str) {
            Ok(bytes) if !bytes.is_empty() => bytes,
            _ => return false,
        }
    } else {
        return false;
    };

    let (environ_cb, log_cb) = {
        let state = lock_state();
        (state.callbacks.environ, state.callbacks.log)
    };

    let bios_bytes = read_bios_from_system_dir(environ_cb, log_cb);
    let mut core = GbaCore::new(bios_bytes.as_deref(), rom_bytes);

    let pixel_format = negotiate_pixel_format(environ_cb);
    let rumble_cb = query_rumble_interface(environ_cb);
    let options = read_core_options(environ_cb);
    apply_options_to_core(&mut core, options);

    if let Some(env) = environ_cb {
        unsafe {
            env(
                RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS,
                INPUT_DESCRIPTORS.as_ptr().cast_mut().cast(),
            );
            let mut achievements = true;
            env(
                RETRO_ENVIRONMENT_SET_SUPPORT_ACHIEVEMENTS,
                (&raw mut achievements).cast(),
            );
        }
    }

    let measured_state = core.save_state().map(|b| b.len()).unwrap_or(0);
    let serialize_bound = (measured_state + STATE_HEADER_SIZE + STATE_MARGIN_BYTES)
        .max(STATE_MIN_BOUND_BYTES);

    let descriptors = Box::new(build_memory_descriptors(&mut core));

    let loaded = LoadedCore {
        core,
        pixel_format,
        options,
        frame_buf: Some(alloc_boxed_u16_zeroed::<{ LCD_WIDTH * LCD_HEIGHT }>()),
        audio_buf: Vec::with_capacity(2048),
        spare_audio_buf: Vec::with_capacity(2048),
        serialize_bound,
        last_rumble_active: false,
        memory_descriptors: descriptors,
    };

    let mut map = retro_memory_map {
        descriptors: loaded.memory_descriptors.as_ptr(),
        num_descriptors: if loaded.memory_descriptors[3].len > 0 {
            4
        } else {
            3
        },
    };
    let map_ptr = (&raw mut map).cast();

    {
        let mut state = lock_state();
        state.callbacks.rumble = rumble_cb;
        state.loaded = Some(loaded);
    }

    if let Some(env) = environ_cb {
        unsafe {
            env(RETRO_ENVIRONMENT_SET_MEMORY_MAPS, map_ptr);
        }
    }

    true
}

pub fn unload_game() {
    let (rumble_cb, mut prev_loaded) = {
        let mut state = lock_state();
        (state.callbacks.rumble, state.loaded.take())
    };
    if let Some(ref mut loaded) = prev_loaded {
        stop_rumble(rumble_cb, &mut loaded.last_rumble_active);
    }
}

pub fn reset() {
    let (environ_cb, rumble_cb, mut last_rumble) = {
        let state = lock_state();
        (
            state.callbacks.environ,
            state.callbacks.rumble,
            state
                .loaded
                .as_ref()
                .is_some_and(|l| l.last_rumble_active),
        )
    };

    stop_rumble(rumble_cb, &mut last_rumble);

    let options = read_core_options(environ_cb);
    let mut state = lock_state();
    if let Some(ref mut loaded) = state.loaded {
        loaded.last_rumble_active = false;
        loaded.options = options;
        let _ = preserve_memory_buffers(&mut loaded.core, true, |c| {
            c.reset();
            Ok::<(), ()>(())
        });
        apply_options_to_core(&mut loaded.core, options);
    }
}

pub fn run() {
    let (callbacks, port0_device, has_loaded) = {
        let state = lock_state();
        (state.callbacks, state.port0_device, state.loaded.is_some())
    };
    if !has_loaded {
        return;
    }

    // Check if core options were updated before running the frame.
    let mut vars_updated = false;
    if let Some(env) = callbacks.environ {
        let ok = unsafe {
            env(
                RETRO_ENVIRONMENT_GET_VARIABLE_UPDATE,
                (&raw mut vars_updated).cast(),
            )
        };
        if !ok {
            vars_updated = false;
        }
    }
    let updated_options = if vars_updated {
        Some(read_core_options(callbacks.environ))
    } else {
        None
    };

    // Poll keypad before acquiring the core mutex so frontend input callbacks never deadlock.
    let keyinput = poll_port0_keypad(callbacks.input_poll, callbacks.input_state, port0_device);

    let (
        out_frame,
        out_audio,
        pixel_format,
        rumble_change,
        refresh_map,
    ) = {
        let mut state = lock_state();
        let Some(ref mut loaded) = state.loaded else {
            return;
        };

        let mut refresh_map = false;
        if let Some(opts) = updated_options {
            let save_changed = opts.save_type_override != loaded.options.save_type_override;
            loaded.options = opts;
            apply_options_to_core(&mut loaded.core, opts);
            refresh_map = save_changed;
        }

        let mut frame_buf = loaded
            .frame_buf
            .take()
            .unwrap_or_else(alloc_boxed_u16_zeroed::<{ LCD_WIDTH * LCD_HEIGHT }>);
        loaded.audio_buf.clear();

        let mut host = FrameHost {
            frame_buf: &mut frame_buf,
            audio_buf: &mut loaded.audio_buf,
            keyinput,
            pixel_format: loaded.pixel_format,
            color_correction: loaded.options.color_correction,
            frame_presented: false,
        };
        loaded.core.run_frame(&mut host);
        if !host.frame_presented {
            convert_framebuffer(
                &loaded.core.bus.ppu.framebuffer,
                &mut frame_buf,
                loaded.pixel_format,
                loaded.options.color_correction,
            );
        }

        let mut out_audio = core::mem::take(&mut loaded.spare_audio_buf);
        out_audio.clear();
        core::mem::swap(&mut loaded.audio_buf, &mut out_audio);

        let rumble_now =
            loaded.core.bus.cart.rumble_enabled && loaded.core.bus.cart.rumble_active;
        let rumble_change = if rumble_now != loaded.last_rumble_active {
            loaded.last_rumble_active = rumble_now;
            Some(rumble_now)
        } else {
            None
        };

        let mut map_descriptors = None;
        if refresh_map {
            *loaded.memory_descriptors = build_memory_descriptors(&mut loaded.core);
            map_descriptors = Some((
                loaded.memory_descriptors.as_ptr(),
                if loaded.memory_descriptors[3].len > 0 {
                    4
                } else {
                    3
                },
            ));
        }

        (
            frame_buf,
            out_audio,
            loaded.pixel_format,
            rumble_change,
            map_descriptors,
        )
    };

    if let Some((descs_ptr, num_descs)) = refresh_map
        && let Some(env) = callbacks.environ
    {
        let mut map = retro_memory_map {
            descriptors: descs_ptr,
            num_descriptors: num_descs,
        };
        unsafe {
            env(RETRO_ENVIRONMENT_SET_MEMORY_MAPS, (&raw mut map).cast());
        }
    }

    if let Some(video_cb) = callbacks.video_refresh {
        unsafe {
            video_cb(
                out_frame.as_ptr().cast(),
                LCD_WIDTH as c_uint,
                LCD_HEIGHT as c_uint,
                pixel_format.pitch_bytes(),
            );
        }
    }

    if !out_audio.is_empty() {
        let frames = out_audio.len() / 2;
        if frames > 0 {
            if let Some(batch_cb) = callbacks.audio_sample_batch {
                unsafe {
                    batch_cb(out_audio.as_ptr(), frames);
                }
            } else if let Some(sample_cb) = callbacks.audio_sample {
                let (pairs, _) = out_audio.as_chunks::<2>();
                for pair in pairs {
                    unsafe {
                        sample_cb(pair[0], pair[1]);
                    }
                }
            }
        }
    }

    if let Some(active) = rumble_change
        && let Some(rumble_cb) = callbacks.rumble
    {
        let strength = if active { u16::MAX } else { 0 };
        unsafe {
            rumble_cb(0, RETRO_RUMBLE_STRONG, strength);
            rumble_cb(0, RETRO_RUMBLE_WEAK, strength);
        }
    }

    // Return frame and audio buffers back to LoadedCore to reuse allocations next frame.
    let mut state = lock_state();
    if let Some(ref mut loaded) = state.loaded {
        loaded.frame_buf = Some(out_frame);
        let mut recycled = out_audio;
        recycled.clear();
        loaded.spare_audio_buf = recycled;
    }
}

pub fn serialize_size() -> usize {
    let state = lock_state();
    state
        .loaded
        .as_ref()
        .map_or(0, |loaded| loaded.serialize_bound)
}

/// # Safety
/// `data` must point to at least `size` writable bytes when non-null.
pub unsafe fn serialize(data: *mut c_void, size: usize) -> bool {
    if data.is_null() || size < STATE_HEADER_SIZE {
        return false;
    }

    let mut state = lock_state();
    let Some(ref mut loaded) = state.loaded else {
        return false;
    };

    let Ok(payload) = loaded.core.save_state() else {
        return false;
    };
    let needed = STATE_HEADER_SIZE + payload.len();
    if size < needed {
        return false;
    }

    let dst = unsafe { core::slice::from_raw_parts_mut(data.cast::<u8>(), size) };
    dst[0..4].copy_from_slice(&STATE_MAGIC);
    dst[4..8].copy_from_slice(&STATE_VERSION.to_le_bytes());
    dst[8..16].copy_from_slice(&(payload.len() as u64).to_le_bytes());
    dst[STATE_HEADER_SIZE..needed].copy_from_slice(&payload);
    dst[needed..size].fill(0);

    true
}

/// # Safety
/// `data` must point to at least `size` readable bytes when non-null.
pub unsafe fn unserialize(data: *const c_void, size: usize) -> bool {
    if data.is_null() || size < STATE_HEADER_SIZE {
        return false;
    }

    let src = unsafe { core::slice::from_raw_parts(data.cast::<u8>(), size) };
    if src[0..4] != STATE_MAGIC {
        return false;
    }
    let version = u32::from_le_bytes([src[4], src[5], src[6], src[7]]);
    if version != STATE_VERSION {
        return false;
    }
    let payload_len = u64::from_le_bytes([
        src[8], src[9], src[10], src[11], src[12], src[13], src[14], src[15],
    ]);
    let Ok(payload_len_usize) = usize::try_from(payload_len) else {
        return false;
    };
    let Some(end) = STATE_HEADER_SIZE.checked_add(payload_len_usize) else {
        return false;
    };
    if end > size {
        return false;
    }
    let payload = &src[STATE_HEADER_SIZE..end];

    let mut state = lock_state();
    let Some(ref mut loaded) = state.loaded else {
        return false;
    };

    let ok = preserve_memory_buffers(&mut loaded.core, false, |c| c.load_state(payload)).is_ok();
    if ok {
        apply_options_to_core(&mut loaded.core, loaded.options);
    }
    ok
}

pub fn get_memory_data(id: c_uint) -> *mut c_void {
    let mut state = lock_state();
    let Some(ref mut loaded) = state.loaded else {
        return ptr::null_mut();
    };
    match id & RETRO_MEMORY_MASK {
        RETRO_MEMORY_SYSTEM_RAM => loaded.core.bus.ewram.as_mut_ptr().cast(),
        RETRO_MEMORY_SAVE_RAM => {
            if save_ram_size_for_cart(&loaded.core) == 0 {
                ptr::null_mut()
            } else {
                loaded.core.bus.cart.sram.as_mut_ptr().cast()
            }
        }
        _ => ptr::null_mut(),
    }
}

pub fn get_memory_size(id: c_uint) -> usize {
    let state = lock_state();
    let Some(ref loaded) = state.loaded else {
        return 0;
    };
    match id & RETRO_MEMORY_MASK {
        RETRO_MEMORY_SYSTEM_RAM => EWRAM_SIZE,
        RETRO_MEMORY_SAVE_RAM => save_ram_size_for_cart(&loaded.core),
        _ => 0,
    }
}
