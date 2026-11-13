use gba_core::GbaCore;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheatFormat {
    AutoRaw,
    CodeBreaker,
    GameSharkV1,
    ActionReplayV3,
}

impl CheatFormat {
    pub const ALL: [CheatFormat; 4] = [
        Self::AutoRaw,
        Self::CodeBreaker,
        Self::GameSharkV1,
        Self::ActionReplayV3,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::AutoRaw => "Auto / Raw",
            Self::CodeBreaker => "CodeBreaker",
            Self::GameSharkV1 => "GameShark / AR v1",
            Self::ActionReplayV3 => "Action Replay v3",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheatEntry {
    pub name: String,
    pub code: String,
    pub format: CheatFormat,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheatAction {
    Write8 { addr: u32, val: u8 },
    Write16 { addr: u32, val: u16 },
    Write32 { addr: u32, val: u32 },
    Add16 { addr: u32, delta: u16 },
    IfEqual16 { addr: u32, val: u16 },
    IfNotEqual16 { addr: u32, val: u16 },
    NoOp,
}

const AR_V1_SEEDS: [u32; 4] = [0x09F4_FBBD, 0x9681_884A, 0x3520_27E9, 0xF3DE_E5A7];
const AR_V3_SEEDS: [u32; 4] = [0x7AA9_648F, 0x7FAE_6994, 0xC0EF_AAD5, 0x4271_2C57];

fn tea_decrypt(mut v0: u32, mut v1: u32, k: &[u32; 4]) -> (u32, u32) {
    let mut sum = 0xC6EF_3720u32;
    let delta = 0x9E37_79B9u32;
    for _ in 0..32 {
        v1 = v1.wrapping_sub(
            ((v0 << 4).wrapping_add(k[2]))
                ^ (v0.wrapping_add(sum))
                ^ ((v0 >> 5).wrapping_add(k[3])),
        );
        v0 = v0.wrapping_sub(
            ((v1 << 4).wrapping_add(k[0]))
                ^ (v1.wrapping_add(sum))
                ^ ((v1 >> 5).wrapping_add(k[1])),
        );
        sum = sum.wrapping_sub(delta);
    }
    (v0, v1)
}

#[inline]
fn cb_map_addr(addr_28: u32) -> u32 {
    // Decode CodeBreaker RAM/ROM address bits (upper nibble determines EWRAM/IWRAM/ROM target).
    let low = addr_28 & 0x00FF_FFFF;
    match (addr_28 >> 24) & 0x0F {
        0x00 => 0x0300_0000 | (low & 0x7FFF),
        0x02 => 0x0200_0000 | (low & 0x3_FFFF),
        0x03 => 0x0300_0000 | (low & 0x7FFF),
        other => (other << 24) | low,
    }
}

fn parse_raw_pair_32_16(w1: u32, w2: u16) -> CheatAction {
    let op = (w1 >> 28) & 0xF;
    let addr = cb_map_addr(w1 & 0x0FFF_FFFF);
    match op {
        0x2 => CheatAction::Add16 { addr, delta: w2 },
        0x3 => CheatAction::Write8 {
            addr,
            val: (w2 & 0xFF) as u8,
        },
        0x6 => CheatAction::Write16 { addr, val: w2 },
        0x7 => CheatAction::IfEqual16 { addr, val: w2 },
        0x8 => CheatAction::Write16 { addr, val: w2 },
        0xA => CheatAction::IfNotEqual16 { addr, val: w2 },
        0xD => {
            if (w1 & 0x0FFF_FFFF) == 0x0000_0020 {
                // D0000020 xxxx: keypad check against active-high button mask
                CheatAction::IfEqual16 {
                    addr: 0x0400_0130,
                    val: (!w2) & 0x03FF,
                }
            } else {
                CheatAction::IfEqual16 { addr, val: w2 }
            }
        }
        _ => CheatAction::NoOp,
    }
}

fn parse_ar_decrypted(op_addr: u32, val: u32, is_v3: bool) -> CheatAction {
    if op_addr == 0xDEAD_FACE {
        return CheatAction::NoOp;
    }
    if is_v3 {
        let cmd = (op_addr >> 24) & 0xFF;
        let region_bit = (op_addr >> 25) & 1;
        let low = op_addr & 0x00FF_FFFF;
        let base = if region_bit != 0 {
            0x0300_0000 | (low & 0x7FFF)
        } else {
            0x0200_0000 | (low & 0x3_FFFF)
        };
        match cmd & 0xFD {
            0x00 => CheatAction::Write8 {
                addr: base,
                val: (val & 0xFF) as u8,
            },
            0x02 => CheatAction::Write16 {
                addr: base,
                val: (val & 0xFFFF) as u16,
            },
            0x04 => CheatAction::Write32 { addr: base, val },
            0x08 => CheatAction::IfEqual16 {
                addr: base,
                val: (val & 0xFFFF) as u16,
            },
            0x0A => CheatAction::IfNotEqual16 {
                addr: base,
                val: (val & 0xFFFF) as u16,
            },
            _ => CheatAction::NoOp,
        }
    } else {
        // Raw or AR v1/v2 28-bit addressing
        let cmd = (op_addr >> 28) & 0xF;
        let addr = op_addr & 0x0FFF_FFFF;
        match cmd {
            0x0 => CheatAction::Write8 {
                addr,
                val: (val & 0xFF) as u8,
            },
            0x1 => CheatAction::Write16 {
                addr,
                val: (val & 0xFFFF) as u16,
            },
            0x2 => CheatAction::Write32 { addr, val },
            0x8 => CheatAction::Write16 {
                addr: 0x0200_0000 | (addr & 0x3_FFFF),
                val: (val & 0xFFFF) as u16,
            },
            0xD => CheatAction::IfEqual16 {
                addr,
                val: (val & 0xFFFF) as u16,
            },
            _ => {
                // Direct raw 32-bit RAM/ROM address (`02xxxxxx` / `03xxxxxx` / `08xxxxxx`)
                if matches!((op_addr >> 24) & 0xFF, 0x02..=0x0D) {
                    if val <= 0xFF {
                        CheatAction::Write8 {
                            addr: op_addr,
                            val: val as u8,
                        }
                    } else if val <= 0xFFFF {
                        CheatAction::Write16 {
                            addr: op_addr,
                            val: val as u16,
                        }
                    } else {
                        CheatAction::Write32 { addr: op_addr, val }
                    }
                } else {
                    CheatAction::NoOp
                }
            }
        }
    }
}

pub fn decode_cheat_lines(code: &str, format: CheatFormat) -> Result<Vec<CheatAction>, String> {
    let mut actions = Vec::new();
    for (line_idx, raw_line) in code.lines().enumerate() {
        let cleaned: String = raw_line
            .chars()
            .filter(|c| c.is_ascii_hexdigit() || c.is_whitespace())
            .collect();
        let tokens: Vec<&str> = cleaned.split_whitespace().collect();
        if tokens.is_empty() {
            continue;
        }
        // Join into pairs; users sometimes paste codes with spaces or without spaces (e.g. 16 hex chars).
        let pairs: Vec<(&str, &str)> = if tokens.len() >= 2 && tokens.len().is_multiple_of(2) {
            tokens
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| (c[0], c[1]))
                .collect()
        } else if tokens.len() == 1 && tokens[0].len() == 16 {
            vec![(&tokens[0][..8], &tokens[0][8..16])]
        } else if tokens.len() == 1 && tokens[0].len() == 12 {
            vec![(&tokens[0][..8], &tokens[0][8..12])]
        } else {
            return Err(format!(
                "Line {}: expected `XXXXXXXX YYYY` or `XXXXXXXX YYYYYYYY` hex pairs",
                line_idx + 1
            ));
        };

        for (lhs, rhs) in pairs {
            let w1 = u32::from_str_radix(lhs, 16)
                .map_err(|_| format!("Line {}: invalid hex `{lhs}`", line_idx + 1))?;
            let w2 = u32::from_str_radix(rhs, 16)
                .map_err(|_| format!("Line {}: invalid hex `{rhs}`", line_idx + 1))?;

            let act = match format {
                CheatFormat::CodeBreaker => parse_raw_pair_32_16(w1, w2 as u16),
                CheatFormat::GameSharkV1 => {
                    let (a, b) = tea_decrypt(w1, w2, &AR_V1_SEEDS);
                    parse_ar_decrypted(a, b, false)
                }
                CheatFormat::ActionReplayV3 => {
                    let (a, b) = tea_decrypt(w1, w2, &AR_V3_SEEDS);
                    parse_ar_decrypted(a, b, true)
                }
                CheatFormat::AutoRaw => {
                    if rhs.len() <= 4 {
                        parse_raw_pair_32_16(w1, w2 as u16)
                    } else if matches!((w1 >> 24) & 0xFF, 0x02..=0x0D) {
                        parse_ar_decrypted(w1, w2, false)
                    } else {
                        // Treat unprefixed 8+8 AR code cleanly as raw AR command or v3 decrypted fallback
                        let raw = parse_ar_decrypted(w1, w2, false);
                        if raw == CheatAction::NoOp {
                            let (a, b) = tea_decrypt(w1, w2, &AR_V3_SEEDS);
                            parse_ar_decrypted(a, b, true)
                        } else {
                            raw
                        }
                    }
                }
            };
            actions.push(act);
        }
    }
    Ok(actions)
}

pub fn apply_cheats_to_core(core: &mut GbaCore, cheats: &[CheatEntry]) {
    for entry in cheats {
        if !entry.enabled {
            continue;
        }
        let Ok(actions) = decode_cheat_lines(&entry.code, entry.format) else {
            continue;
        };
        let mut skip_next = false;
        for act in actions {
            if skip_next {
                skip_next = false;
                continue;
            }
            match act {
                CheatAction::Write8 { addr, val } => core.bus.apply_cheat_write_8(addr, val),
                CheatAction::Write16 { addr, val } => core.bus.apply_cheat_write_16(addr, val),
                CheatAction::Write32 { addr, val } => core.bus.apply_cheat_write_32(addr, val),
                CheatAction::Add16 { addr, delta } => {
                    let cur = core.bus.debug_read_16(addr);
                    core.bus.apply_cheat_write_16(addr, cur.wrapping_add(delta));
                }
                CheatAction::IfEqual16 { addr, val } => {
                    if core.bus.debug_read_16(addr) != val {
                        skip_next = true;
                    }
                }
                CheatAction::IfNotEqual16 { addr, val } => {
                    if core.bus.debug_read_16(addr) == val {
                        skip_next = true;
                    }
                }
                CheatAction::NoOp => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codebreaker_and_raw_parsing_and_application() {
        let mut core = GbaCore::new(None, vec![0; 0x200]);
        let entries = vec![
            CheatEntry {
                name: "EWRAM 8bit".to_string(),
                code: "32000100 007F".to_string(),
                format: CheatFormat::CodeBreaker,
                enabled: true,
            },
            CheatEntry {
                name: "IWRAM 16bit".to_string(),
                code: "83000200 ABCD".to_string(),
                format: CheatFormat::CodeBreaker,
                enabled: true,
            },
            CheatEntry {
                name: "Conditional false skip".to_string(),
                code: "72000100 0099\n82000104 1234".to_string(),
                format: CheatFormat::CodeBreaker,
                enabled: true,
            },
            CheatEntry {
                name: "Conditional true pass".to_string(),
                code: "72000100 007F\n82000108 5678".to_string(),
                format: CheatFormat::CodeBreaker,
                enabled: true,
            },
        ];
        apply_cheats_to_core(&mut core, &entries);
        assert_eq!(core.bus.debug_read_8(0x0200_0100), 0x7F);
        assert_eq!(core.bus.debug_read_16(0x0300_0200), 0xABCD);
        assert_eq!(core.bus.debug_read_16(0x0200_0104), 0x0000);
        assert_eq!(core.bus.debug_read_16(0x0200_0108), 0x5678);
    }
}
