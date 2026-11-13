use core::ffi::{CStr, c_char, c_uint, c_void};
use core::ptr;
use std::sync::Mutex;

use gba_core::{KEY_A, KEY_UP, LCD_HEIGHT, LCD_WIDTH, SAMPLE_RATE_HZ};
use gba_libretro::ffi::{
    RETRO_API_VERSION, RETRO_DEVICE_ID_JOYPAD_A, RETRO_DEVICE_ID_JOYPAD_UP, RETRO_DEVICE_JOYPAD,
    RETRO_DEVICE_NONE, RETRO_ENVIRONMENT_GET_LOG_INTERFACE, RETRO_ENVIRONMENT_GET_RUMBLE_INTERFACE,
    RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY, RETRO_ENVIRONMENT_GET_VARIABLE,
    RETRO_ENVIRONMENT_GET_VARIABLE_UPDATE, RETRO_ENVIRONMENT_SET_CONTROLLER_INFO,
    RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS, RETRO_ENVIRONMENT_SET_MEMORY_MAPS,
    RETRO_ENVIRONMENT_SET_PIXEL_FORMAT, RETRO_ENVIRONMENT_SET_SUPPORT_ACHIEVEMENTS,
    RETRO_ENVIRONMENT_SET_SUPPORT_NO_GAME, RETRO_ENVIRONMENT_SET_VARIABLES, RETRO_MEMORY_SAVE_RAM,
    RETRO_MEMORY_SYSTEM_RAM, RETRO_PIXEL_FORMAT_0RGB1555, RETRO_PIXEL_FORMAT_RGB565,
    RETRO_REGION_NTSC, RETRO_RUMBLE_STRONG, retro_game_info, retro_log_callback, retro_log_level,
    retro_memory_map, retro_pixel_format, retro_rumble_effect, retro_rumble_interface,
    retro_system_av_info, retro_system_info, retro_variable,
};
use gba_libretro::{
    retro_api_version, retro_cheat_reset, retro_cheat_set, retro_deinit, retro_get_memory_data,
    retro_get_memory_size, retro_get_region, retro_get_system_av_info, retro_get_system_info,
    retro_init, retro_load_game, retro_load_game_special, retro_reset, retro_run, retro_serialize,
    retro_serialize_size, retro_set_audio_sample, retro_set_audio_sample_batch,
    retro_set_controller_port_device, retro_set_environment, retro_set_input_poll,
    retro_set_input_state, retro_set_video_refresh, retro_unload_game, retro_unserialize,
};

static TEST_LOCK: Mutex<()> = Mutex::new(());
static MOCK: Mutex<MockFrontend> = Mutex::new(MockFrontend::new());

struct MockFrontend {
    allow_rgb565: bool,
    vars_set: bool,
    input_descriptors_set: bool,
    achievements_supported: bool,
    system_dir: Option<std::ffi::CString>,
    vars_updated: bool,
    opt_color_correction: Option<std::ffi::CString>,
    opt_solar_level: Option<std::ffi::CString>,
    opt_save_type: Option<std::ffi::CString>,
    pixel_format: Option<retro_pixel_format>,
    memory_maps: Vec<(usize, usize, u64)>,
    video_frames: usize,
    last_width: c_uint,
    last_height: c_uint,
    last_pitch: usize,
    last_frame_pixels: Vec<u16>,
    audio_frames_received: usize,
    input_poll_calls: usize,
    pressed_a: bool,
    pressed_up: bool,
    rumble_events: Vec<(c_uint, retro_rumble_effect, u16)>,
    log_messages: usize,
}

impl MockFrontend {
    const fn new() -> Self {
        Self {
            allow_rgb565: true,
            vars_set: false,
            input_descriptors_set: false,
            achievements_supported: false,
            system_dir: None,
            vars_updated: false,
            opt_color_correction: None,
            opt_solar_level: None,
            opt_save_type: None,
            pixel_format: None,
            memory_maps: Vec::new(),
            video_frames: 0,
            last_width: 0,
            last_height: 0,
            last_pitch: 0,
            last_frame_pixels: Vec::new(),
            audio_frames_received: 0,
            input_poll_calls: 0,
            pressed_a: false,
            pressed_up: false,
            rumble_events: Vec::new(),
            log_messages: 0,
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }
}

fn lock_mock() -> std::sync::MutexGuard<'static, MockFrontend> {
    MOCK.lock().unwrap_or_else(|p| p.into_inner())
}

unsafe extern "C" fn mock_log(_level: retro_log_level, _fmt: *const c_char, _: ...) {
    lock_mock().log_messages += 1;
}

unsafe extern "C" fn mock_rumble(port: c_uint, effect: retro_rumble_effect, strength: u16) -> bool {
    lock_mock().rumble_events.push((port, effect, strength));
    true
}

unsafe extern "C" fn mock_environ(cmd: c_uint, data: *mut c_void) -> bool {
    let mut mock = lock_mock();
    match cmd {
        RETRO_ENVIRONMENT_SET_SUPPORT_NO_GAME => !data.is_null(),
        RETRO_ENVIRONMENT_SET_VARIABLES => {
            mock.vars_set = !data.is_null();
            mock.vars_set
        }
        RETRO_ENVIRONMENT_SET_CONTROLLER_INFO => !data.is_null(),
        RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS => {
            mock.input_descriptors_set = !data.is_null();
            mock.input_descriptors_set
        }
        RETRO_ENVIRONMENT_SET_PIXEL_FORMAT => {
            if data.is_null() {
                return false;
            }
            let fmt = unsafe { *data.cast::<retro_pixel_format>() };
            if fmt == RETRO_PIXEL_FORMAT_RGB565 && !mock.allow_rgb565 {
                return false;
            }
            if fmt == RETRO_PIXEL_FORMAT_RGB565 || fmt == RETRO_PIXEL_FORMAT_0RGB1555 {
                mock.pixel_format = Some(fmt);
                true
            } else {
                false
            }
        }
        RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY => {
            if data.is_null() {
                return false;
            }
            if let Some(ref c_dir) = mock.system_dir {
                unsafe {
                    *data.cast::<*const c_char>() = c_dir.as_ptr();
                }
                true
            } else {
                false
            }
        }
        RETRO_ENVIRONMENT_GET_LOG_INTERFACE => {
            if data.is_null() {
                return false;
            }
            unsafe {
                (*data.cast::<retro_log_callback>()).log = Some(mock_log);
            }
            true
        }
        RETRO_ENVIRONMENT_GET_RUMBLE_INTERFACE => {
            if data.is_null() {
                return false;
            }
            unsafe {
                (*data.cast::<retro_rumble_interface>()).set_rumble_state = Some(mock_rumble);
            }
            true
        }
        RETRO_ENVIRONMENT_GET_VARIABLE_UPDATE => {
            if data.is_null() {
                return false;
            }
            unsafe {
                *data.cast::<bool>() = mock.vars_updated;
            }
            mock.vars_updated = false;
            true
        }
        RETRO_ENVIRONMENT_GET_VARIABLE => {
            if data.is_null() {
                return false;
            }
            let var = unsafe { &mut *data.cast::<retro_variable>() };
            if var.key.is_null() {
                return false;
            }
            let key = unsafe { CStr::from_ptr(var.key) }.to_str().unwrap_or("");
            let val_ptr = match key {
                "rustboy_color_correction" => mock
                    .opt_color_correction
                    .as_ref()
                    .map_or(ptr::null(), |s| s.as_ptr()),
                "rustboy_solar_sensor_level" => mock
                    .opt_solar_level
                    .as_ref()
                    .map_or(ptr::null(), |s| s.as_ptr()),
                "rustboy_save_type" => mock
                    .opt_save_type
                    .as_ref()
                    .map_or(ptr::null(), |s| s.as_ptr()),
                _ => ptr::null(),
            };
            if val_ptr.is_null() {
                false
            } else {
                var.value = val_ptr;
                true
            }
        }
        RETRO_ENVIRONMENT_SET_MEMORY_MAPS => {
            if data.is_null() {
                return false;
            }
            let map = unsafe { &*data.cast::<retro_memory_map>() };
            if map.descriptors.is_null() {
                return false;
            }
            mock.memory_maps.clear();
            let slice = unsafe {
                core::slice::from_raw_parts(map.descriptors, map.num_descriptors as usize)
            };
            for desc in slice {
                mock.memory_maps.push((desc.start, desc.len, desc.flags));
            }
            true
        }
        RETRO_ENVIRONMENT_SET_SUPPORT_ACHIEVEMENTS => {
            if data.is_null() {
                return false;
            }
            mock.achievements_supported = unsafe { *data.cast::<bool>() };
            true
        }
        _ => false,
    }
}

unsafe extern "C" fn mock_video_refresh(
    data: *const c_void,
    width: c_uint,
    height: c_uint,
    pitch: usize,
) {
    let mut mock = lock_mock();
    mock.video_frames += 1;
    mock.last_width = width;
    mock.last_height = height;
    mock.last_pitch = pitch;
    if !data.is_null() {
        let len = (width as usize) * (height as usize);
        let slice = unsafe { core::slice::from_raw_parts(data.cast::<u16>(), len) };
        mock.last_frame_pixels = slice.to_vec();
    }
}

unsafe extern "C" fn mock_audio_sample(_left: i16, _right: i16) {
    lock_mock().audio_frames_received += 1;
}

unsafe extern "C" fn mock_audio_sample_batch(_data: *const i16, frames: usize) -> usize {
    lock_mock().audio_frames_received += frames;
    frames
}

unsafe extern "C" fn mock_input_poll() {
    lock_mock().input_poll_calls += 1;
}

unsafe extern "C" fn mock_input_state(
    port: c_uint,
    device: c_uint,
    _index: c_uint,
    id: c_uint,
) -> i16 {
    if port != 0 || device != RETRO_DEVICE_JOYPAD {
        return 0;
    }
    let mock = lock_mock();
    match id {
        RETRO_DEVICE_ID_JOYPAD_A if mock.pressed_a => 1,
        RETRO_DEVICE_ID_JOYPAD_UP if mock.pressed_up => 1,
        _ => 0,
    }
}

struct TestGuard;

impl TestGuard {
    fn acquire() -> (std::sync::MutexGuard<'static, ()>, Self) {
        let guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        lock_mock().reset();
        retro_deinit();
        retro_set_environment(Some(mock_environ));
        retro_set_video_refresh(Some(mock_video_refresh));
        retro_set_audio_sample(Some(mock_audio_sample));
        retro_set_audio_sample_batch(Some(mock_audio_sample_batch));
        retro_set_input_poll(Some(mock_input_poll));
        retro_set_input_state(Some(mock_input_state));
        retro_init();
        (guard, Self)
    }
}

impl Drop for TestGuard {
    fn drop(&mut self) {
        retro_unload_game();
        retro_deinit();
    }
}

fn rom_from_words(words: &[u32]) -> Vec<u8> {
    let mut rom = Vec::with_capacity(words.len() * 4);
    for &w in words {
        rom.extend_from_slice(&w.to_le_bytes());
    }
    rom
}

/// Synthetic GBA ROM that sets Mode 3 + BG2, writes BGR555 red (0x001F) to pixel (0,0),
/// and halts in `B .` (`0xEAFFFFFE`).
fn mode3_red_pixel_rom() -> Vec<u8> {
    rom_from_words(&[
        0xE3A0_0301, // MOV R0, #0x04000000
        0xE3A0_1B01, // MOV R1, #0x0400
        0xE381_1003, // ORR R1, R1, #3 -> 0x0403 (Mode 3, BG2)
        0xE1C0_10B0, // STRH R1, [R0]
        0xE3A0_2406, // MOV R2, #0x06000000
        0xE3A0_301F, // MOV R3, #0x1F (BGR555 pure red)
        0xE1C2_30B0, // STRH R3, [R2]
        0xEAFF_FFFE, // B .
    ])
}

#[test]
fn system_and_av_info_and_null_safety() {
    let (_guard, _teardown) = TestGuard::acquire();

    assert_eq!(retro_api_version(), RETRO_API_VERSION);
    assert_eq!(retro_get_region(), RETRO_REGION_NTSC);

    let mut sys = retro_system_info {
        library_name: ptr::null(),
        library_version: ptr::null(),
        valid_extensions: ptr::null(),
        need_fullpath: true,
        block_extract: true,
    };
    unsafe { retro_get_system_info(&raw mut sys) };
    assert!(!sys.library_name.is_null());
    assert!(!sys.library_version.is_null());
    assert!(!sys.valid_extensions.is_null());
    assert_eq!(
        unsafe { CStr::from_ptr(sys.library_name) }
            .to_str()
            .unwrap(),
        "Rustboy Advance"
    );
    assert_eq!(
        unsafe { CStr::from_ptr(sys.valid_extensions) }
            .to_str()
            .unwrap(),
        "gba|agb|gb|gbc|bin"
    );
    assert!(!sys.need_fullpath);
    assert!(!sys.block_extract);

    let mut av = retro_system_av_info {
        geometry: gba_libretro::ffi::retro_game_geometry {
            base_width: 0,
            base_height: 0,
            max_width: 0,
            max_height: 0,
            aspect_ratio: 0.0,
        },
        timing: gba_libretro::ffi::retro_system_timing {
            fps: 0.0,
            sample_rate: 0.0,
        },
    };
    unsafe { retro_get_system_av_info(&raw mut av) };
    assert_eq!(av.geometry.base_width, LCD_WIDTH as c_uint);
    assert_eq!(av.geometry.base_height, LCD_HEIGHT as c_uint);
    assert_eq!(av.geometry.max_width, LCD_WIDTH as c_uint);
    assert_eq!(av.geometry.max_height, LCD_HEIGHT as c_uint);
    assert!((av.geometry.aspect_ratio - 1.5).abs() < 1e-6);
    assert!((av.timing.fps - 59.7275).abs() < 1e-6);
    assert_eq!(av.timing.sample_rate, SAMPLE_RATE_HZ as f64);

    // Null pointer & unloaded-state guards should never panic or crash.
    unsafe {
        retro_get_system_info(ptr::null_mut());
        retro_get_system_av_info(ptr::null_mut());
        assert!(!retro_load_game(ptr::null()));
        assert!(!retro_load_game_special(0, ptr::null(), 0));
        assert_eq!(retro_serialize_size(), 0);
        assert!(!retro_serialize(ptr::null_mut(), 64));
        assert!(!retro_unserialize(ptr::null(), 64));
        retro_cheat_reset();
        retro_cheat_set(0, false, ptr::null());
    }
    assert!(retro_get_memory_data(RETRO_MEMORY_SYSTEM_RAM).is_null());
    assert_eq!(retro_get_memory_size(RETRO_MEMORY_SYSTEM_RAM), 0);
}

#[test]
fn run_frames_rgb565_audio_memory_and_serialize_roundtrip() {
    let (_guard, _teardown) = TestGuard::acquire();

    let rom = mode3_red_pixel_rom();
    let game = retro_game_info {
        path: ptr::null(),
        data: rom.as_ptr().cast(),
        size: rom.len(),
        meta: ptr::null(),
    };
    assert!(unsafe { retro_load_game(&raw const game) });

    {
        let mock = lock_mock();
        assert!(mock.vars_set);
        assert!(mock.input_descriptors_set);
        assert!(mock.achievements_supported);
        assert_eq!(mock.pixel_format, Some(RETRO_PIXEL_FORMAT_RGB565));
        assert!(
            mock.memory_maps
                .iter()
                .any(|&(start, len, _)| start == 0x0200_0000 && len == 0x40000)
        );
        assert!(
            mock.memory_maps
                .iter()
                .any(|&(start, len, _)| start == 0x0300_0000 && len == 0x8000)
        );
    }

    for _ in 0..3 {
        retro_run();
    }

    {
        let mock = lock_mock();
        assert_eq!(mock.video_frames, 3);
        assert_eq!(mock.last_width, LCD_WIDTH as c_uint);
        assert_eq!(mock.last_height, LCD_HEIGHT as c_uint);
        assert_eq!(mock.last_pitch, LCD_WIDTH * 2);
        assert_eq!(mock.last_frame_pixels.len(), LCD_WIDTH * LCD_HEIGHT);
        // BGR555 0x001F (red) -> RGB565 0xF800.
        assert_eq!(mock.last_frame_pixels[0], 0xF800);
        assert!(mock.audio_frames_received > 0);
        assert_eq!(mock.input_poll_calls, 3);
    }

    let sys_ptr = retro_get_memory_data(RETRO_MEMORY_SYSTEM_RAM);
    let sys_size = retro_get_memory_size(RETRO_MEMORY_SYSTEM_RAM);
    assert!(!sys_ptr.is_null());
    assert_eq!(sys_size, 0x40000);

    let save_ptr = retro_get_memory_data(RETRO_MEMORY_SAVE_RAM);
    let save_size = retro_get_memory_size(RETRO_MEMORY_SAVE_RAM);
    assert!(!save_ptr.is_null());
    assert!(save_size > 0);

    // Serialize roundtrip verifies EWRAM state restoration and pointer stability.
    let state_size = retro_serialize_size();
    assert!(state_size >= 600_000);
    let mut state_buf = vec![0u8; state_size];
    assert!(unsafe { retro_serialize(state_buf.as_mut_ptr().cast(), state_buf.len()) });
    assert_eq!(&state_buf[0..4], b"RBLS");

    unsafe {
        *sys_ptr.cast::<u8>().add(0x123) = 0xA5;
    }
    retro_run();
    assert_eq!(unsafe { *sys_ptr.cast::<u8>().add(0x123) }, 0xA5);

    assert!(unsafe { retro_unserialize(state_buf.as_ptr().cast(), state_buf.len()) });
    assert_eq!(retro_get_memory_data(RETRO_MEMORY_SYSTEM_RAM), sys_ptr);
    assert_eq!(unsafe { *sys_ptr.cast::<u8>().add(0x123) }, 0x00);

    // Corrupted magic must fail gracefully without clobbering memory.
    state_buf[0] = b'X';
    assert!(!unsafe { retro_unserialize(state_buf.as_ptr().cast(), state_buf.len()) });

    // Reset keeps memory pointer valid and runs cleanly.
    retro_reset();
    assert_eq!(retro_get_memory_data(RETRO_MEMORY_SYSTEM_RAM), sys_ptr);
    retro_run();
}

#[test]
fn pixel_format_falls_back_to_0rgb1555() {
    let (_guard, _teardown) = TestGuard::acquire();
    lock_mock().allow_rgb565 = false;

    let rom = mode3_red_pixel_rom();
    let game = retro_game_info {
        path: ptr::null(),
        data: rom.as_ptr().cast(),
        size: rom.len(),
        meta: ptr::null(),
    };
    assert!(unsafe { retro_load_game(&raw const game) });
    assert_eq!(lock_mock().pixel_format, Some(RETRO_PIXEL_FORMAT_0RGB1555));

    for _ in 0..2 {
        retro_run();
    }
    let mock = lock_mock();
    assert_eq!(mock.last_pitch, LCD_WIDTH * 2);
    // BGR555 0x001F (red) -> 0RGB1555 0x7C00.
    assert_eq!(mock.last_frame_pixels[0], 0x7C00);
}

#[test]
fn core_options_apply_color_correction_and_save_override() {
    let (_guard, _teardown) = TestGuard::acquire();
    {
        let mut mock = lock_mock();
        mock.opt_color_correction = Some(std::ffi::CString::new("GBA").unwrap());
        mock.opt_solar_level = Some(std::ffi::CString::new("80%").unwrap());
        mock.opt_save_type = Some(std::ffi::CString::new("SRAM 32KB").unwrap());
    }

    let rom = mode3_red_pixel_rom();
    let game = retro_game_info {
        path: ptr::null(),
        data: rom.as_ptr().cast(),
        size: rom.len(),
        meta: ptr::null(),
    };
    assert!(unsafe { retro_load_game(&raw const game) });
    assert_eq!(retro_get_memory_size(RETRO_MEMORY_SAVE_RAM), 0x8000);

    for _ in 0..2 {
        retro_run();
    }
    // GBA color correction modifies pure BGR555 red (0x001F) away from 0xF800.
    assert_ne!(lock_mock().last_frame_pixels[0], 0xF800);

    // Update core option dynamically to None save type.
    {
        let mut mock = lock_mock();
        mock.opt_save_type = Some(std::ffi::CString::new("None").unwrap());
        mock.vars_updated = true;
    }
    retro_run();
    assert_eq!(retro_get_memory_size(RETRO_MEMORY_SAVE_RAM), 0);
    assert!(retro_get_memory_data(RETRO_MEMORY_SAVE_RAM).is_null());
}

#[test]
fn keypad_polling_updates_keyinput_and_respects_device_none() {
    let (_guard, _teardown) = TestGuard::acquire();

    // Loop: read KEYINPUT (0x0400_0130) and store halfword at EWRAM 0x0200_0000.
    let rom = rom_from_words(&[
        0xE3A0_0301, // MOV R0, #0x04000000
        0xE280_0C01, // ADD R0, R0, #0x100
        0xE280_0030, // ADD R0, R0, #0x30  -> 0x0400_0130 (KEYINPUT)
        0xE3A0_1402, // MOV R1, #0x02000000 (EWRAM)
        0xE1D0_20B0, // LDRH R2, [R0]
        0xE1C1_20B0, // STRH R2, [R1]
        0xEAFF_FFFC, // B -16 (loop back to LDRH)
    ]);
    let game = retro_game_info {
        path: ptr::null(),
        data: rom.as_ptr().cast(),
        size: rom.len(),
        meta: ptr::null(),
    };
    assert!(unsafe { retro_load_game(&raw const game) });

    {
        let mut mock = lock_mock();
        mock.pressed_a = true;
        mock.pressed_up = true;
    }
    retro_run();
    let ewram = retro_get_memory_data(RETRO_MEMORY_SYSTEM_RAM) as *const u16;
    assert!(!ewram.is_null());
    let key_val = unsafe { *ewram } & 0x03FF;
    assert_eq!(key_val, !(KEY_A | KEY_UP) & 0x03FF);

    retro_set_controller_port_device(0, RETRO_DEVICE_NONE);
    retro_run();
    let released = unsafe { *ewram } & 0x03FF;
    assert_eq!(released, 0x03FF);
}

#[test]
fn bios_loaded_from_system_directory_and_hle_fallback() {
    let (_guard, _teardown) = TestGuard::acquire();

    let uniq = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sys_dir = std::env::temp_dir().join(format!("rba_libretro_bios_test_{uniq}"));
    std::fs::create_dir_all(&sys_dir).unwrap();

    // Write a 16 KiB synthetic BIOS whose reset vector jumps directly to 0x0800_0000.
    let mut bios = vec![0u8; 0x4000];
    bios[0..4].copy_from_slice(&0xE3A0_0408u32.to_le_bytes()); // MOV R0, #0x08000000
    bios[4..8].copy_from_slice(&0xE12F_FF10u32.to_le_bytes()); // BX R0
    let bios_path = sys_dir.join("gba_bios.bin");
    std::fs::write(&bios_path, &bios).unwrap();

    lock_mock().system_dir = Some(std::ffi::CString::new(sys_dir.to_str().unwrap()).unwrap());

    let rom = mode3_red_pixel_rom();
    let game = retro_game_info {
        path: ptr::null(),
        data: rom.as_ptr().cast(),
        size: rom.len(),
        meta: ptr::null(),
    };
    assert!(unsafe { retro_load_game(&raw const game) });
    for _ in 0..2 {
        retro_run();
    }
    assert_eq!(lock_mock().last_frame_pixels[0], 0xF800);

    let _ = std::fs::remove_file(&bios_path);
    let _ = std::fs::remove_dir(&sys_dir);
}

#[test]
fn cart_rumble_triggers_libretro_rumble_callback() {
    let (_guard, _teardown) = TestGuard::acquire();

    // ROM large enough for header at 0xAC..0xB0 with game code "RZWJ" (rumble enabled).
    let mut rom = vec![0u8; 0x100];
    rom[0xAC..0xB0].copy_from_slice(b"RZWJ");
    // Entry at 0x0800_0000: branch over header to 0x0800_00C0 -> offset = (0xC0 - 8)/4 = 0x2E
    rom[0..4].copy_from_slice(&0xEA00_002Eu32.to_le_bytes());

    let program: [u32; 7] = [
        0xE3A0_0408, // MOV R0, #0x08000000
        0xE280_00C6, // ADD R0, R0, #0xC6 (GPIO dir)
        0xE3A0_1008, // MOV R1, #8
        0xE1C0_10B0, // STRH R1, [R0]
        0xE240_0002, // SUB R0, R0, #2 (0x0800_00C4 GPIO data)
        0xE1C0_10B0, // STRH R1, [R0] (rumble bit 3 = 1)
        0xEAFF_FFFE, // B .
    ];
    for (idx, &w) in program.iter().enumerate() {
        let off = 0xC0 + idx * 4;
        rom[off..off + 4].copy_from_slice(&w.to_le_bytes());
    }

    let game = retro_game_info {
        path: ptr::null(),
        data: rom.as_ptr().cast(),
        size: rom.len(),
        meta: ptr::null(),
    };
    assert!(unsafe { retro_load_game(&raw const game) });
    retro_run();

    {
        let mock = lock_mock();
        assert!(
            mock.rumble_events
                .iter()
                .any(|&(port, eff, str_val)| port == 0
                    && eff == RETRO_RUMBLE_STRONG
                    && str_val == u16::MAX)
        );
    }

    retro_unload_game();
    let mock = lock_mock();
    assert!(
        mock.rumble_events
            .iter()
            .any(|&(port, eff, str_val)| port == 0 && eff == RETRO_RUMBLE_STRONG && str_val == 0)
    );
}

#[test]
fn load_game_from_path_and_audio_sample_fallback() {
    let (_guard, _teardown) = TestGuard::acquire();

    // Disable batch callback so per-sample fallback is exercised.
    retro_set_audio_sample_batch(None);

    let uniq = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let rom_path = std::env::temp_dir().join(format!("rba_test_rom_{uniq}.gba"));
    let rom = mode3_red_pixel_rom();
    std::fs::write(&rom_path, &rom).unwrap();

    let c_path = std::ffi::CString::new(rom_path.to_str().unwrap()).unwrap();
    let game = retro_game_info {
        path: c_path.as_ptr(),
        data: ptr::null(),
        size: 0,
        meta: ptr::null(),
    };
    assert!(unsafe { retro_load_game(&raw const game) });

    retro_run();
    {
        let mock = lock_mock();
        assert_eq!(mock.video_frames, 1);
        assert_eq!(mock.last_frame_pixels[0], 0xF800);
        assert!(mock.audio_frames_received > 0);
    }

    let _ = std::fs::remove_file(&rom_path);
}

#[test]
fn save_ram_sizes_follow_detected_rom_signatures() {
    let (_guard, _teardown) = TestGuard::acquire();

    let cases: &[(&[u8], usize)] = &[
        (b"SRAM_V112", 0x8000),
        (b"FLASH512_V130", 0x10000),
        (b"FLASH1M_V103", 0x20000),
        (b"EEPROM_V124", 0x2000),
    ];

    for (sig, expected_size) in cases {
        let mut rom = vec![0u8; 0x100];
        rom[0..4].copy_from_slice(&0xEAFF_FFFEu32.to_le_bytes());
        rom[0xC0..0xC0 + sig.len()].copy_from_slice(sig);

        let game = retro_game_info {
            path: ptr::null(),
            data: rom.as_ptr().cast(),
            size: rom.len(),
            meta: ptr::null(),
        };
        assert!(unsafe { retro_load_game(&raw const game) });
        assert!(!retro_get_memory_data(RETRO_MEMORY_SAVE_RAM).is_null());
        assert_eq!(retro_get_memory_size(RETRO_MEMORY_SAVE_RAM), *expected_size);
        retro_unload_game();
    }
}

#[test]
fn serialize_rejects_short_buffers_and_gba_sp_color_correction_works() {
    let (_guard, _teardown) = TestGuard::acquire();
    lock_mock().opt_color_correction = Some(std::ffi::CString::new("GBA SP").unwrap());

    let rom = mode3_red_pixel_rom();
    let game = retro_game_info {
        path: ptr::null(),
        data: rom.as_ptr().cast(),
        size: rom.len(),
        meta: ptr::null(),
    };
    assert!(unsafe { retro_load_game(&raw const game) });
    for _ in 0..2 {
        retro_run();
    }
    // GBA SP curve maps 0x001F differently from raw 0xF800.
    assert_ne!(lock_mock().last_frame_pixels[0], 0xF800);

    let mut tiny = [0u8; 64];
    assert!(!unsafe { retro_serialize(tiny.as_mut_ptr().cast(), tiny.len()) });
    assert!(!unsafe { retro_unserialize(tiny.as_ptr().cast(), tiny.len()) });
}

fn synthetic_gb_rom() -> Vec<u8> {
    let mut rom = vec![0u8; 0x8000];
    // Entry point 0x0100: NOP; JR -2 (infinite loop at 0x0150)
    rom[0x0100] = 0x00;
    rom[0x0101] = 0xC3; // JP 0x0150
    rom[0x0102] = 0x50;
    rom[0x0103] = 0x01;
    // Nintendo logo prefix at 0x0104..0x010C
    rom[0x0104..0x010C].copy_from_slice(&[0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B]);
    rom[0x0134..0x013C].copy_from_slice(b"GBTEST01");
    rom[0x0143] = 0x00; // DMG
    rom[0x0147] = 0x03; // MBC1+RAM+BATT
    rom[0x0148] = 0x00; // 32 KiB ROM
    rom[0x0149] = 0x02; // 8 KiB RAM
    let mut chk = 0u8;
    for &b in &rom[0x0134..=0x014C] {
        chk = chk.wrapping_sub(b).wrapping_sub(1);
    }
    rom[0x014D] = chk;
    // Program at 0x0150: LD A, 0x80; LDH (0x26), A; JR -2 (enable APU + loop)
    rom[0x0150] = 0x3E; // LD A, 0x80
    rom[0x0151] = 0x80;
    rom[0x0152] = 0xE0; // LDH (0xFF26), A
    rom[0x0153] = 0x26;
    rom[0x0154] = 0x18; // JR -2
    rom[0x0155] = 0xFE;
    rom
}

#[test]
fn dual_core_dispatches_gb_rom_and_serializes_cleanly() {
    let (_guard, _teardown) = TestGuard::acquire();

    let rom = synthetic_gb_rom();
    let game = retro_game_info {
        path: ptr::null(),
        data: rom.as_ptr().cast(),
        size: rom.len(),
        meta: ptr::null(),
    };
    assert!(unsafe { retro_load_game(&raw const game) });

    let mut av = retro_system_av_info {
        geometry: gba_libretro::ffi::retro_game_geometry {
            base_width: 0,
            base_height: 0,
            max_width: 0,
            max_height: 0,
            aspect_ratio: 0.0,
        },
        timing: gba_libretro::ffi::retro_system_timing {
            fps: 0.0,
            sample_rate: 0.0,
        },
    };
    unsafe { retro_get_system_av_info(&raw mut av) };
    assert_eq!(av.geometry.base_width, gb_core::LCD_WIDTH as c_uint);
    assert_eq!(av.geometry.base_height, gb_core::LCD_HEIGHT as c_uint);
    assert!((av.geometry.aspect_ratio - (10.0 / 9.0)).abs() < 1e-5);

    for _ in 0..3 {
        retro_run();
    }

    {
        let mock = lock_mock();
        assert_eq!(mock.video_frames, 3);
        assert_eq!(mock.last_width, gb_core::LCD_WIDTH as c_uint);
        assert_eq!(mock.last_height, gb_core::LCD_HEIGHT as c_uint);
        assert_eq!(mock.last_pitch, gb_core::LCD_WIDTH * 2);
        assert_eq!(
            mock.last_frame_pixels.len(),
            gb_core::LCD_WIDTH * gb_core::LCD_HEIGHT
        );
        assert!(mock.audio_frames_received > 0);
    }

    let wram_ptr = retro_get_memory_data(RETRO_MEMORY_SYSTEM_RAM);
    let wram_size = retro_get_memory_size(RETRO_MEMORY_SYSTEM_RAM);
    assert!(!wram_ptr.is_null());
    assert_eq!(wram_size, 0x8000);

    let sram_ptr = retro_get_memory_data(RETRO_MEMORY_SAVE_RAM);
    let sram_size = retro_get_memory_size(RETRO_MEMORY_SAVE_RAM);
    assert!(!sram_ptr.is_null());
    assert_eq!(sram_size, 0x2000);

    let state_size = retro_serialize_size();
    let mut state_buf = vec![0u8; state_size];
    assert!(unsafe { retro_serialize(state_buf.as_mut_ptr().cast(), state_buf.len()) });

    unsafe {
        *wram_ptr.cast::<u8>().add(0x42) = 0x5A;
    }
    assert!(unsafe { retro_unserialize(state_buf.as_ptr().cast(), state_buf.len()) });
    assert_eq!(retro_get_memory_data(RETRO_MEMORY_SYSTEM_RAM), wram_ptr);
    assert_eq!(unsafe { *wram_ptr.cast::<u8>().add(0x42) }, 0x00);

    retro_reset();
    assert_eq!(retro_get_memory_data(RETRO_MEMORY_SYSTEM_RAM), wram_ptr);
    retro_run();
}
