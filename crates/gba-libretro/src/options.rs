//! Core options (`RETRO_ENVIRONMENT_SET_VARIABLES`), input descriptors, and controller info.

use core::ffi::{CStr, c_char};
use core::ptr;

use gba_core::cartridge::SaveType;

use crate::ffi::{
    RETRO_DEVICE_ID_JOYPAD_A, RETRO_DEVICE_ID_JOYPAD_B, RETRO_DEVICE_ID_JOYPAD_DOWN,
    RETRO_DEVICE_ID_JOYPAD_L, RETRO_DEVICE_ID_JOYPAD_LEFT, RETRO_DEVICE_ID_JOYPAD_R,
    RETRO_DEVICE_ID_JOYPAD_RIGHT, RETRO_DEVICE_ID_JOYPAD_SELECT, RETRO_DEVICE_ID_JOYPAD_START,
    RETRO_DEVICE_ID_JOYPAD_UP, RETRO_DEVICE_JOYPAD, RETRO_ENVIRONMENT_GET_VARIABLE,
    retro_controller_description, retro_controller_info, retro_environment_t,
    retro_input_descriptor, retro_variable,
};
use crate::video::ColorCorrection;

pub const OPT_COLOR_CORRECTION_KEY: &CStr = c"rustboy_color_correction";
pub const OPT_SOLAR_SENSOR_KEY: &CStr = c"rustboy_solar_sensor_level";
pub const OPT_SAVE_TYPE_KEY: &CStr = c"rustboy_save_type";

/// Legacy core options array passed via `RETRO_ENVIRONMENT_SET_VARIABLES`.
pub static CORE_VARIABLES: [retro_variable; 4] = [
    retro_variable {
        key: OPT_COLOR_CORRECTION_KEY.as_ptr(),
        value: c"Color Correction; Off|GBA|GBA SP".as_ptr(),
    },
    retro_variable {
        key: OPT_SOLAR_SENSOR_KEY.as_ptr(),
        value: c"Solar Sensor Level (Boktai); 50%|0%|10%|20%|30%|40%|60%|70%|80%|90%|100%".as_ptr(),
    },
    retro_variable {
        key: OPT_SAVE_TYPE_KEY.as_ptr(),
        value:
            c"Save Type Override; Auto|SRAM 32KB|Flash 64KB|Flash 128KB|EEPROM 512B|EEPROM 8KB|None"
                .as_ptr(),
    },
    retro_variable {
        key: ptr::null(),
        value: ptr::null(),
    },
];

/// Port 0 RetroPad -> GBA hardware button labels.
pub static INPUT_DESCRIPTORS: [retro_input_descriptor; 11] = [
    retro_input_descriptor {
        port: 0,
        device: RETRO_DEVICE_JOYPAD,
        index: 0,
        id: RETRO_DEVICE_ID_JOYPAD_A,
        description: c"A".as_ptr(),
    },
    retro_input_descriptor {
        port: 0,
        device: RETRO_DEVICE_JOYPAD,
        index: 0,
        id: RETRO_DEVICE_ID_JOYPAD_B,
        description: c"B".as_ptr(),
    },
    retro_input_descriptor {
        port: 0,
        device: RETRO_DEVICE_JOYPAD,
        index: 0,
        id: RETRO_DEVICE_ID_JOYPAD_SELECT,
        description: c"Select".as_ptr(),
    },
    retro_input_descriptor {
        port: 0,
        device: RETRO_DEVICE_JOYPAD,
        index: 0,
        id: RETRO_DEVICE_ID_JOYPAD_START,
        description: c"Start".as_ptr(),
    },
    retro_input_descriptor {
        port: 0,
        device: RETRO_DEVICE_JOYPAD,
        index: 0,
        id: RETRO_DEVICE_ID_JOYPAD_UP,
        description: c"D-Pad Up".as_ptr(),
    },
    retro_input_descriptor {
        port: 0,
        device: RETRO_DEVICE_JOYPAD,
        index: 0,
        id: RETRO_DEVICE_ID_JOYPAD_DOWN,
        description: c"D-Pad Down".as_ptr(),
    },
    retro_input_descriptor {
        port: 0,
        device: RETRO_DEVICE_JOYPAD,
        index: 0,
        id: RETRO_DEVICE_ID_JOYPAD_LEFT,
        description: c"D-Pad Left".as_ptr(),
    },
    retro_input_descriptor {
        port: 0,
        device: RETRO_DEVICE_JOYPAD,
        index: 0,
        id: RETRO_DEVICE_ID_JOYPAD_RIGHT,
        description: c"D-Pad Right".as_ptr(),
    },
    retro_input_descriptor {
        port: 0,
        device: RETRO_DEVICE_JOYPAD,
        index: 0,
        id: RETRO_DEVICE_ID_JOYPAD_L,
        description: c"L".as_ptr(),
    },
    retro_input_descriptor {
        port: 0,
        device: RETRO_DEVICE_JOYPAD,
        index: 0,
        id: RETRO_DEVICE_ID_JOYPAD_R,
        description: c"R".as_ptr(),
    },
    retro_input_descriptor {
        port: 0,
        device: 0,
        index: 0,
        id: 0,
        description: ptr::null(),
    },
];

static PORT0_CONTROLLER_TYPES: [retro_controller_description; 1] = [retro_controller_description {
    desc: c"GBA Joypad".as_ptr(),
    id: RETRO_DEVICE_JOYPAD,
}];

pub static CONTROLLER_INFO: [retro_controller_info; 2] = [
    retro_controller_info {
        types: PORT0_CONTROLLER_TYPES.as_ptr(),
        num_types: 1,
    },
    retro_controller_info {
        types: ptr::null(),
        num_types: 0,
    },
];

/// Parsed runtime core option values.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CoreOptions {
    pub color_correction: ColorCorrection,
    pub solar_lux: u8,
    pub save_type_override: Option<SaveType>,
}

impl Default for CoreOptions {
    fn default() -> Self {
        Self {
            color_correction: ColorCorrection::Off,
            solar_lux: 128,
            save_type_override: None,
        }
    }
}

pub fn parse_solar_lux(raw: &str) -> u8 {
    let trimmed = raw.trim();
    if let Some(pct_str) = trimmed.strip_suffix('%') {
        if let Ok(pct) = pct_str.trim().parse::<u16>() {
            let clamped = pct.min(100);
            return ((clamped * 255 + 50) / 100) as u8;
        }
    } else if let Ok(lux) = trimmed.parse::<u8>() {
        return lux;
    }
    128
}

pub fn parse_save_type_override(raw: &str) -> Option<SaveType> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "sram 32kb" | "sram" => Some(SaveType::Sram32K),
        "flash 64kb" | "flash512" | "flash 512k" => Some(SaveType::Flash64K),
        "flash 128kb" | "flash1m" | "flash 1m" => Some(SaveType::Flash128K),
        "eeprom 512b" | "eeprom 4k" => Some(SaveType::Eeprom512B),
        "eeprom 8kb" | "eeprom 64k" | "eeprom" => Some(SaveType::Eeprom8K),
        "none" => Some(SaveType::None),
        _ => None,
    }
}

fn query_variable<'a>(
    environ_cb: retro_environment_t,
    key: &'static CStr,
    scratch: &'a mut String,
) -> Option<&'a str> {
    let env = environ_cb?;
    let mut var = retro_variable {
        key: key.as_ptr(),
        value: ptr::null(),
    };
    let ok = unsafe { env(RETRO_ENVIRONMENT_GET_VARIABLE, (&raw mut var).cast()) };
    if !ok || var.value.is_null() {
        return None;
    }
    let c_val = unsafe { CStr::from_ptr(var.value as *const c_char) };
    let str_val = c_val.to_str().ok()?;
    scratch.clear();
    scratch.push_str(str_val);
    Some(scratch.as_str())
}

/// Reads all core options from the frontend via `RETRO_ENVIRONMENT_GET_VARIABLE`.
pub fn read_core_options(environ_cb: retro_environment_t) -> CoreOptions {
    let mut opts = CoreOptions::default();
    let mut scratch = String::new();

    if let Some(val) = query_variable(environ_cb, OPT_COLOR_CORRECTION_KEY, &mut scratch) {
        opts.color_correction = ColorCorrection::from_option_str(val);
    }
    if let Some(val) = query_variable(environ_cb, OPT_SOLAR_SENSOR_KEY, &mut scratch) {
        opts.solar_lux = parse_solar_lux(val);
    }
    if let Some(val) = query_variable(environ_cb, OPT_SAVE_TYPE_KEY, &mut scratch) {
        opts.save_type_override = parse_save_type_override(val);
    }

    opts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_solar_and_save_type_options() {
        assert_eq!(parse_solar_lux("0%"), 0);
        assert_eq!(parse_solar_lux("50%"), 128);
        assert_eq!(parse_solar_lux("100%"), 255);
        assert_eq!(parse_solar_lux("200"), 200);

        assert_eq!(parse_save_type_override("Auto"), None);
        assert_eq!(
            parse_save_type_override("SRAM 32KB"),
            Some(SaveType::Sram32K)
        );
        assert_eq!(
            parse_save_type_override("Flash 64KB"),
            Some(SaveType::Flash64K)
        );
        assert_eq!(
            parse_save_type_override("Flash 128KB"),
            Some(SaveType::Flash128K)
        );
        assert_eq!(
            parse_save_type_override("EEPROM 512B"),
            Some(SaveType::Eeprom512B)
        );
        assert_eq!(
            parse_save_type_override("EEPROM 8KB"),
            Some(SaveType::Eeprom8K)
        );
        assert_eq!(parse_save_type_override("None"), Some(SaveType::None));
    }
}
