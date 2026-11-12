use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// A decoded bitfield within a GBA MMIO register.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IoField {
    /// Human-readable field name (e.g., `"BG Mode"`, `"Forced Blank"`).
    pub name: &'static str,
    /// Bit position or inclusive range (e.g., `"0..=2"`, `"7"`).
    pub bit_range: &'static str,
    /// Extracted numeric value of the field.
    pub value: u32,
    /// Human-readable interpretation of `value`.
    pub meaning: String,
}

/// Complete decoded description of a GBA MMIO register.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IoRegDesc {
    /// Register address in the `0x0400_xxxx` range.
    pub addr: u32,
    /// Hardware register name (e.g., `"DISPCNT"`).
    pub name: &'static str,
    /// Functional subsystem (`"LCD"`, `"Sound"`, `"DMA"`, `"Timer"`, `"Serial"`, `"Keypad"`, `"System"`).
    pub category: &'static str,
    /// Raw value passed to [`decode_io_reg`].
    pub raw_value: u32,
    /// Concise single-line summary of the active register state.
    pub summary: String,
    /// Detailed per-bitfield breakdown.
    pub fields: Vec<IoField>,
}

const IRQ_FLAG_NAMES: [&str; 14] = [
    "VBlank",
    "HBlank",
    "VCount",
    "Timer0",
    "Timer1",
    "Timer2",
    "Timer3",
    "Serial",
    "DMA0",
    "DMA1",
    "DMA2",
    "DMA3",
    "Keypad",
    "GamePak",
];

const KEY_NAMES: [&str; 10] = [
    "A", "B", "Select", "Start", "Right", "Left", "Up", "Down", "R", "L",
];

#[inline]
fn bits(val: u32, lsb: u32, width: u32) -> u32 {
    let mask = if width >= 32 {
        u32::MAX
    } else {
        (1u32 << width) - 1
    };
    (val >> lsb) & mask
}

#[inline]
fn bit(val: u32, idx: u32) -> bool {
    ((val >> idx) & 1) != 0
}

fn on_off(b: bool) -> String {
    String::from(if b { "On" } else { "Off" })
}

fn flag_list(val: u32, names: &[&str]) -> String {
    let mut active = Vec::new();
    for (idx, name) in names.iter().enumerate() {
        if bit(val, idx as u32) {
            active.push(*name);
        }
    }
    if active.is_empty() {
        String::from("None")
    } else {
        active.join(", ")
    }
}

fn window_mask_desc(val: u32) -> String {
    let mut parts = Vec::new();
    for i in 0..4 {
        if bit(val, i) {
            parts.push(match i {
                0 => "BG0",
                1 => "BG1",
                2 => "BG2",
                _ => "BG3",
            });
        }
    }
    if bit(val, 4) {
        parts.push("OBJ");
    }
    if bit(val, 5) {
        parts.push("SFX");
    }
    if parts.is_empty() {
        String::from("None")
    } else {
        parts.join("+")
    }
}

/// Static list of well-known GBA MMIO registers (`(address, name, category)`).
pub fn known_io_regs() -> &'static [(u32, &'static str, &'static str)] {
    &[
        (0x0400_0000, "DISPCNT", "LCD"),
        (0x0400_0002, "GREENSWP", "LCD"),
        (0x0400_0004, "DISPSTAT", "LCD"),
        (0x0400_0006, "VCOUNT", "LCD"),
        (0x0400_0008, "BG0CNT", "LCD"),
        (0x0400_000A, "BG1CNT", "LCD"),
        (0x0400_000C, "BG2CNT", "LCD"),
        (0x0400_000E, "BG3CNT", "LCD"),
        (0x0400_0010, "BG0HOFS", "LCD"),
        (0x0400_0012, "BG0VOFS", "LCD"),
        (0x0400_0014, "BG1HOFS", "LCD"),
        (0x0400_0016, "BG1VOFS", "LCD"),
        (0x0400_0018, "BG2HOFS", "LCD"),
        (0x0400_001A, "BG2VOFS", "LCD"),
        (0x0400_001C, "BG3HOFS", "LCD"),
        (0x0400_001E, "BG3VOFS", "LCD"),
        (0x0400_0020, "BG2PA", "LCD"),
        (0x0400_0022, "BG2PB", "LCD"),
        (0x0400_0024, "BG2PC", "LCD"),
        (0x0400_0026, "BG2PD", "LCD"),
        (0x0400_0028, "BG2X_L", "LCD"),
        (0x0400_002A, "BG2X_H", "LCD"),
        (0x0400_002C, "BG2Y_L", "LCD"),
        (0x0400_002E, "BG2Y_H", "LCD"),
        (0x0400_0030, "BG3PA", "LCD"),
        (0x0400_0032, "BG3PB", "LCD"),
        (0x0400_0034, "BG3PC", "LCD"),
        (0x0400_0036, "BG3PD", "LCD"),
        (0x0400_0038, "BG3X_L", "LCD"),
        (0x0400_003A, "BG3X_H", "LCD"),
        (0x0400_003C, "BG3Y_L", "LCD"),
        (0x0400_003E, "BG3Y_H", "LCD"),
        (0x0400_0040, "WIN0H", "LCD"),
        (0x0400_0042, "WIN1H", "LCD"),
        (0x0400_0044, "WIN0V", "LCD"),
        (0x0400_0046, "WIN1V", "LCD"),
        (0x0400_0048, "WININ", "LCD"),
        (0x0400_004A, "WINOUT", "LCD"),
        (0x0400_004C, "MOSAIC", "LCD"),
        (0x0400_0050, "BLDCNT", "LCD"),
        (0x0400_0052, "BLDALPHA", "LCD"),
        (0x0400_0054, "BLDY", "LCD"),
        (0x0400_0060, "SOUND1CNT_L", "Sound"),
        (0x0400_0062, "SOUND1CNT_H", "Sound"),
        (0x0400_0064, "SOUND1CNT_X", "Sound"),
        (0x0400_0068, "SOUND2CNT_L", "Sound"),
        (0x0400_006C, "SOUND2CNT_H", "Sound"),
        (0x0400_0070, "SOUND3CNT_L", "Sound"),
        (0x0400_0072, "SOUND3CNT_H", "Sound"),
        (0x0400_0074, "SOUND3CNT_X", "Sound"),
        (0x0400_0078, "SOUND4CNT_L", "Sound"),
        (0x0400_007C, "SOUND4CNT_H", "Sound"),
        (0x0400_0080, "SOUNDCNT_L", "Sound"),
        (0x0400_0082, "SOUNDCNT_H", "Sound"),
        (0x0400_0084, "SOUNDCNT_X", "Sound"),
        (0x0400_0088, "SOUNDBIAS", "Sound"),
        (0x0400_00A0, "FIFO_A_L", "Sound"),
        (0x0400_00A4, "FIFO_B_L", "Sound"),
        (0x0400_00B0, "DMA0SAD_L", "DMA"),
        (0x0400_00B4, "DMA0DAD_L", "DMA"),
        (0x0400_00B8, "DMA0CNT_L", "DMA"),
        (0x0400_00BA, "DMA0CNT_H", "DMA"),
        (0x0400_00BC, "DMA1SAD_L", "DMA"),
        (0x0400_00C0, "DMA1DAD_L", "DMA"),
        (0x0400_00C4, "DMA1CNT_L", "DMA"),
        (0x0400_00C6, "DMA1CNT_H", "DMA"),
        (0x0400_00C8, "DMA2SAD_L", "DMA"),
        (0x0400_00CC, "DMA2DAD_L", "DMA"),
        (0x0400_00D0, "DMA2CNT_L", "DMA"),
        (0x0400_00D2, "DMA2CNT_H", "DMA"),
        (0x0400_00D4, "DMA3SAD_L", "DMA"),
        (0x0400_00D8, "DMA3DAD_L", "DMA"),
        (0x0400_00DC, "DMA3CNT_L", "DMA"),
        (0x0400_00DE, "DMA3CNT_H", "DMA"),
        (0x0400_0100, "TM0CNT_L", "Timer"),
        (0x0400_0102, "TM0CNT_H", "Timer"),
        (0x0400_0104, "TM1CNT_L", "Timer"),
        (0x0400_0106, "TM1CNT_H", "Timer"),
        (0x0400_0108, "TM2CNT_L", "Timer"),
        (0x0400_010A, "TM2CNT_H", "Timer"),
        (0x0400_010C, "TM3CNT_L", "Timer"),
        (0x0400_010E, "TM3CNT_H", "Timer"),
        (0x0400_0120, "SIODATA32_L", "Serial"),
        (0x0400_0122, "SIODATA32_H", "Serial"),
        (0x0400_0128, "SIOCNT", "Serial"),
        (0x0400_012A, "SIOMLT_SEND", "Serial"),
        (0x0400_0130, "KEYINPUT", "Keypad"),
        (0x0400_0132, "KEYCNT", "Keypad"),
        (0x0400_0134, "RCNT", "Serial"),
        (0x0400_0140, "JOYCNT", "Serial"),
        (0x0400_0158, "JOYSTAT", "Serial"),
        (0x0400_0200, "IE", "System"),
        (0x0400_0202, "IF", "System"),
        (0x0400_0204, "WAITCNT", "System"),
        (0x0400_0208, "IME", "System"),
        (0x0400_0300, "POSTFLG", "System"),
        (0x0400_0301, "HALTCNT", "System"),
    ]
}

fn decode_dispcnt(addr: u32, v: u32) -> IoRegDesc {
    let mode = bits(v, 0, 3);
    let mode_desc = match mode {
        0 => "Mode 0 (4 Text BGs)",
        1 => "Mode 1 (2 Text + 1 Affine BG)",
        2 => "Mode 2 (2 Affine BGs)",
        3 => "Mode 3 (240x160 15bpp Bitmap)",
        4 => "Mode 4 (240x160 8bpp Palette)",
        5 => "Mode 5 (160x128 15bpp Bitmap)",
        _ => "Prohibited Mode",
    };
    let mut enabled_layers = Vec::new();
    for (idx, lbl) in [(8, "BG0"), (9, "BG1"), (10, "BG2"), (11, "BG3"), (12, "OBJ")] {
        if bit(v, idx) {
            enabled_layers.push(lbl);
        }
    }
    let layers_str = if enabled_layers.is_empty() {
        String::from("None")
    } else {
        enabled_layers.join(",")
    };
    let fields = vec![
        IoField {
            name: "BG Mode",
            bit_range: "0..=2",
            value: mode,
            meaning: String::from(mode_desc),
        },
        IoField {
            name: "Display Frame",
            bit_range: "4",
            value: bits(v, 4, 1),
            meaning: format!("Frame {}", bits(v, 4, 1)),
        },
        IoField {
            name: "HBlank Free",
            bit_range: "5",
            value: bits(v, 5, 1),
            meaning: on_off(bit(v, 5)),
        },
        IoField {
            name: "OBJ Mapping",
            bit_range: "6",
            value: bits(v, 6, 1),
            meaning: String::from(if bit(v, 6) { "1D" } else { "2D" }),
        },
        IoField {
            name: "Forced Blank",
            bit_range: "7",
            value: bits(v, 7, 1),
            meaning: on_off(bit(v, 7)),
        },
        IoField {
            name: "Visible Layers",
            bit_range: "8..=12",
            value: bits(v, 8, 5),
            meaning: layers_str.clone(),
        },
        IoField {
            name: "Windows",
            bit_range: "13..=15",
            value: bits(v, 13, 3),
            meaning: format!(
                "WIN0={} WIN1={} OBJWIN={}",
                on_off(bit(v, 13)),
                on_off(bit(v, 14)),
                on_off(bit(v, 15))
            ),
        },
    ];
    let blank_tag = if bit(v, 7) { " [FORCED BLANK]" } else { "" };
    IoRegDesc {
        addr,
        name: "DISPCNT",
        category: "LCD",
        raw_value: v,
        summary: format!("Mode {mode}, Layers: {layers_str}{blank_tag}"),
        fields,
    }
}

fn decode_dispstat(addr: u32, v: u32) -> IoRegDesc {
    let lyc = bits(v, 8, 8);
    let fields = vec![
        IoField {
            name: "VBlank Flag",
            bit_range: "0",
            value: bits(v, 0, 1),
            meaning: on_off(bit(v, 0)),
        },
        IoField {
            name: "HBlank Flag",
            bit_range: "1",
            value: bits(v, 1, 1),
            meaning: on_off(bit(v, 1)),
        },
        IoField {
            name: "VCount Match",
            bit_range: "2",
            value: bits(v, 2, 1),
            meaning: on_off(bit(v, 2)),
        },
        IoField {
            name: "VBlank IRQ",
            bit_range: "3",
            value: bits(v, 3, 1),
            meaning: on_off(bit(v, 3)),
        },
        IoField {
            name: "HBlank IRQ",
            bit_range: "4",
            value: bits(v, 4, 1),
            meaning: on_off(bit(v, 4)),
        },
        IoField {
            name: "VCount IRQ",
            bit_range: "5",
            value: bits(v, 5, 1),
            meaning: on_off(bit(v, 5)),
        },
        IoField {
            name: "VCount Target (LYC)",
            bit_range: "8..=15",
            value: lyc,
            meaning: format!("Line {lyc}"),
        },
    ];
    IoRegDesc {
        addr,
        name: "DISPSTAT",
        category: "LCD",
        raw_value: v,
        summary: format!(
            "IRQ[VB={} HB={} VC={}] LYC={lyc}",
            bit(v, 3) as u8,
            bit(v, 4) as u8,
            bit(v, 5) as u8
        ),
        fields,
    }
}

fn decode_bgcnt(addr: u32, v: u32, bg_idx: usize) -> IoRegDesc {
    let name = match bg_idx {
        0 => "BG0CNT",
        1 => "BG1CNT",
        2 => "BG2CNT",
        _ => "BG3CNT",
    };
    let prio = bits(v, 0, 2);
    let cbb = bits(v, 2, 2);
    let sbb = bits(v, 8, 5);
    let bpp8 = bit(v, 7);
    let size = bits(v, 14, 2);
    let fields = vec![
        IoField {
            name: "Priority",
            bit_range: "0..=1",
            value: prio,
            meaning: format!("{prio} (0=highest)"),
        },
        IoField {
            name: "Char Base Block",
            bit_range: "2..=3",
            value: cbb,
            meaning: format!("0x{:08X}", 0x0600_0000 + cbb * 0x4000),
        },
        IoField {
            name: "Mosaic",
            bit_range: "6",
            value: bits(v, 6, 1),
            meaning: on_off(bit(v, 6)),
        },
        IoField {
            name: "Color Mode",
            bit_range: "7",
            value: bits(v, 7, 1),
            meaning: String::from(if bpp8 { "256/1 (8bpp)" } else { "16/16 (4bpp)" }),
        },
        IoField {
            name: "Screen Base Block",
            bit_range: "8..=12",
            value: sbb,
            meaning: format!("0x{:08X}", 0x0600_0000 + sbb * 0x800),
        },
        IoField {
            name: "Affine Wraparound",
            bit_range: "13",
            value: bits(v, 13, 1),
            meaning: on_off(bit(v, 13)),
        },
        IoField {
            name: "Screen Size",
            bit_range: "14..=15",
            value: size,
            meaning: format!("Size {size}"),
        },
    ];
    IoRegDesc {
        addr,
        name,
        category: "LCD",
        raw_value: v,
        summary: format!(
            "Prio={prio} CBB={cbb} SBB={sbb} {}",
            if bpp8 { "8bpp" } else { "4bpp" }
        ),
        fields,
    }
}

fn decode_bldcnt(addr: u32, v: u32) -> IoRegDesc {
    let mode = bits(v, 6, 2);
    let mode_name = match mode {
        0 => "Off",
        1 => "Alpha Blending",
        2 => "Brightness Increase",
        _ => "Brightness Decrease",
    };
    let t1 = window_mask_desc(bits(v, 0, 6));
    let t2 = window_mask_desc(bits(v, 8, 6));
    IoRegDesc {
        addr,
        name: "BLDCNT",
        category: "LCD",
        raw_value: v,
        summary: format!("{mode_name} ({t1} -> {t2})"),
        fields: vec![
            IoField {
                name: "1st Target",
                bit_range: "0..=5",
                value: bits(v, 0, 6),
                meaning: t1,
            },
            IoField {
                name: "Effect Mode",
                bit_range: "6..=7",
                value: mode,
                meaning: String::from(mode_name),
            },
            IoField {
                name: "2nd Target",
                bit_range: "8..=13",
                value: bits(v, 8, 6),
                meaning: t2,
            },
        ],
    }
}

fn decode_soundcnt_h(addr: u32, v: u32) -> IoRegDesc {
    let dmg_vol = match bits(v, 0, 2) {
        0 => "25%",
        1 => "50%",
        2 => "100%",
        _ => "Prohibited",
    };
    let fifo_a_vol = if bit(v, 2) { "100%" } else { "50%" };
    let fifo_b_vol = if bit(v, 3) { "100%" } else { "50%" };
    let fields = vec![
        IoField {
            name: "DMG Volume",
            bit_range: "0..=1",
            value: bits(v, 0, 2),
            meaning: String::from(dmg_vol),
        },
        IoField {
            name: "FIFO A Volume",
            bit_range: "2",
            value: bits(v, 2, 1),
            meaning: String::from(fifo_a_vol),
        },
        IoField {
            name: "FIFO B Volume",
            bit_range: "3",
            value: bits(v, 3, 1),
            meaning: String::from(fifo_b_vol),
        },
        IoField {
            name: "FIFO A Routing",
            bit_range: "8..=10",
            value: bits(v, 8, 3),
            meaning: format!(
                "R={} L={} Timer{}",
                on_off(bit(v, 8)),
                on_off(bit(v, 9)),
                bits(v, 10, 1)
            ),
        },
        IoField {
            name: "FIFO B Routing",
            bit_range: "12..=14",
            value: bits(v, 12, 3),
            meaning: format!(
                "R={} L={} Timer{}",
                on_off(bit(v, 12)),
                on_off(bit(v, 13)),
                bits(v, 14, 1)
            ),
        },
    ];
    IoRegDesc {
        addr,
        name: "SOUNDCNT_H",
        category: "Sound",
        raw_value: v,
        summary: format!("DMG={dmg_vol} FIFO_A={fifo_a_vol} FIFO_B={fifo_b_vol}"),
        fields,
    }
}

fn decode_dmacnt_h(addr: u32, v: u32, ch: usize) -> IoRegDesc {
    let name = match ch {
        0 => "DMA0CNT_H",
        1 => "DMA1CNT_H",
        2 => "DMA2CNT_H",
        _ => "DMA3CNT_H",
    };
    let addr_ctrl = |x: u32| match x {
        0 => "Increment",
        1 => "Decrement",
        2 => "Fixed",
        _ => "Inc/Reload",
    };
    let dst = bits(v, 5, 2);
    let src = bits(v, 7, 2);
    let timing = bits(v, 12, 2);
    let timing_name = match timing {
        0 => "Immediate",
        1 => "VBlank",
        2 => "HBlank",
        _ => {
            if ch == 1 || ch == 2 {
                "Sound FIFO"
            } else if ch == 3 {
                "Video Capture"
            } else {
                "Prohibited"
            }
        }
    };
    let fields = vec![
        IoField {
            name: "Dest Ctrl",
            bit_range: "5..=6",
            value: dst,
            meaning: String::from(addr_ctrl(dst)),
        },
        IoField {
            name: "Src Ctrl",
            bit_range: "7..=8",
            value: src,
            meaning: String::from(if src == 3 { "Prohibited" } else { addr_ctrl(src) }),
        },
        IoField {
            name: "Repeat",
            bit_range: "9",
            value: bits(v, 9, 1),
            meaning: on_off(bit(v, 9)),
        },
        IoField {
            name: "Chunk Size",
            bit_range: "10",
            value: bits(v, 10, 1),
            meaning: String::from(if bit(v, 10) { "32-bit" } else { "16-bit" }),
        },
        IoField {
            name: "Start Timing",
            bit_range: "12..=13",
            value: timing,
            meaning: String::from(timing_name),
        },
        IoField {
            name: "IRQ on End",
            bit_range: "14",
            value: bits(v, 14, 1),
            meaning: on_off(bit(v, 14)),
        },
        IoField {
            name: "Enable",
            bit_range: "15",
            value: bits(v, 15, 1),
            meaning: on_off(bit(v, 15)),
        },
    ];
    IoRegDesc {
        addr,
        name,
        category: "DMA",
        raw_value: v,
        summary: format!(
            "{} {} {timing_name}{}",
            if bit(v, 15) { "ENABLED" } else { "Off" },
            if bit(v, 10) { "32b" } else { "16b" },
            if bit(v, 9) { " Repeat" } else { "" }
        ),
        fields,
    }
}

fn decode_tmcnt_h(addr: u32, v: u32, ch: usize) -> IoRegDesc {
    let name = match ch {
        0 => "TM0CNT_H",
        1 => "TM1CNT_H",
        2 => "TM2CNT_H",
        _ => "TM3CNT_H",
    };
    let prescaler = bits(v, 0, 2);
    let div = match prescaler {
        0 => "F/1",
        1 => "F/64",
        2 => "F/256",
        _ => "F/1024",
    };
    let fields = vec![
        IoField {
            name: "Prescaler",
            bit_range: "0..=1",
            value: prescaler,
            meaning: String::from(div),
        },
        IoField {
            name: "Cascade (Count-Up)",
            bit_range: "2",
            value: bits(v, 2, 1),
            meaning: on_off(bit(v, 2)),
        },
        IoField {
            name: "IRQ Enable",
            bit_range: "6",
            value: bits(v, 6, 1),
            meaning: on_off(bit(v, 6)),
        },
        IoField {
            name: "Timer Enable",
            bit_range: "7",
            value: bits(v, 7, 1),
            meaning: on_off(bit(v, 7)),
        },
    ];
    IoRegDesc {
        addr,
        name,
        category: "Timer",
        raw_value: v,
        summary: format!(
            "{} {div}{}{}",
            if bit(v, 7) { "RUNNING" } else { "Stopped" },
            if bit(v, 2) { " Cascade" } else { "" },
            if bit(v, 6) { " IRQ" } else { "" }
        ),
        fields,
    }
}

fn decode_waitcnt(addr: u32, v: u32) -> IoRegDesc {
    let n_cycles = |x: u32| match x & 3 {
        0 => 4,
        1 => 3,
        2 => 2,
        _ => 8,
    };
    let sram = n_cycles(bits(v, 0, 2));
    let ws0_n = n_cycles(bits(v, 2, 2));
    let ws0_s = if bit(v, 4) { 1 } else { 2 };
    let ws1_n = n_cycles(bits(v, 5, 2));
    let ws1_s = if bit(v, 7) { 1 } else { 4 };
    let ws2_n = n_cycles(bits(v, 8, 2));
    let ws2_s = if bit(v, 10) { 1 } else { 8 };
    let phi = match bits(v, 11, 2) {
        0 => "Disabled",
        1 => "4.19 MHz",
        2 => "8.38 MHz",
        _ => "16.78 MHz",
    };
    let fields = vec![
        IoField {
            name: "SRAM Wait",
            bit_range: "0..=1",
            value: bits(v, 0, 2),
            meaning: format!("{sram} cycles"),
        },
        IoField {
            name: "WS0 (N/S)",
            bit_range: "2..=4",
            value: bits(v, 2, 3),
            meaning: format!("{ws0_n}/{ws0_s} cycles"),
        },
        IoField {
            name: "WS1 (N/S)",
            bit_range: "5..=7",
            value: bits(v, 5, 3),
            meaning: format!("{ws1_n}/{ws1_s} cycles"),
        },
        IoField {
            name: "WS2 (N/S)",
            bit_range: "8..=10",
            value: bits(v, 8, 3),
            meaning: format!("{ws2_n}/{ws2_s} cycles"),
        },
        IoField {
            name: "PHI Output",
            bit_range: "11..=12",
            value: bits(v, 11, 2),
            meaning: String::from(phi),
        },
        IoField {
            name: "Game Pak Prefetch",
            bit_range: "14",
            value: bits(v, 14, 1),
            meaning: on_off(bit(v, 14)),
        },
    ];
    IoRegDesc {
        addr,
        name: "WAITCNT",
        category: "System",
        raw_value: v,
        summary: format!(
            "SRAM={sram} WS0={ws0_n}/{ws0_s} Prefetch={}",
            on_off(bit(v, 14))
        ),
        fields,
    }
}

/// Decodes a GBA MMIO register address (`0x0400_0000..=0x0400_0304`, plus `0x0400_0800`)
/// and its current value into a human-readable descriptor with bitfields.
pub fn decode_io_reg(addr: u32, val: u32) -> Option<IoRegDesc> {
    match addr {
        0x0400_0000 => Some(decode_dispcnt(addr, val)),
        0x0400_0002 => Some(IoRegDesc {
            addr,
            name: "GREENSWP",
            category: "LCD",
            raw_value: val,
            summary: format!("Green Swap: {}", on_off(bit(val, 0))),
            fields: vec![IoField {
                name: "Green Swap",
                bit_range: "0",
                value: bits(val, 0, 1),
                meaning: on_off(bit(val, 0)),
            }],
        }),
        0x0400_0004 => Some(decode_dispstat(addr, val)),
        0x0400_0006 => {
            let line = bits(val, 0, 8);
            Some(IoRegDesc {
                addr,
                name: "VCOUNT",
                category: "LCD",
                raw_value: val,
                summary: format!(
                    "Scanline {line} ({})",
                    if line >= 160 { "VBlank" } else { "Visible" }
                ),
                fields: vec![IoField {
                    name: "Current Scanline",
                    bit_range: "0..=7",
                    value: line,
                    meaning: format!("{line} / 227"),
                }],
            })
        }
        0x0400_0008 => Some(decode_bgcnt(addr, val, 0)),
        0x0400_000A => Some(decode_bgcnt(addr, val, 1)),
        0x0400_000C => Some(decode_bgcnt(addr, val, 2)),
        0x0400_000E => Some(decode_bgcnt(addr, val, 3)),
        0x0400_0010..=0x0400_001E if (addr & 1) == 0 => {
            let idx = ((addr - 0x0400_0010) / 4) as usize;
            let is_v = ((addr - 0x0400_0010) & 2) != 0;
            let names = [
                ("BG0HOFS", "BG0VOFS"),
                ("BG1HOFS", "BG1VOFS"),
                ("BG2HOFS", "BG2VOFS"),
                ("BG3HOFS", "BG3VOFS"),
            ];
            let name = if is_v { names[idx].1 } else { names[idx].0 };
            let ofs = bits(val, 0, 9);
            Some(IoRegDesc {
                addr,
                name,
                category: "LCD",
                raw_value: val,
                summary: format!("Offset = {ofs}"),
                fields: vec![IoField {
                    name: if is_v { "Y Offset" } else { "X Offset" },
                    bit_range: "0..=8",
                    value: ofs,
                    meaning: format!("{ofs} px"),
                }],
            })
        }
        0x0400_0020..=0x0400_003E if (addr & 1) == 0 => {
            let bg = if addr < 0x0400_0030 { 2 } else { 3 };
            let off = (addr & 0xF) as usize;
            let names_2 = [
                "BG2PA", "BG2PB", "BG2PC", "BG2PD", "BG2X_L", "BG2X_H", "BG2Y_L", "BG2Y_H",
            ];
            let names_3 = [
                "BG3PA", "BG3PB", "BG3PC", "BG3PD", "BG3X_L", "BG3X_H", "BG3Y_L", "BG3Y_H",
            ];
            let name = if bg == 2 {
                names_2[off / 2]
            } else {
                names_3[off / 2]
            };
            Some(IoRegDesc {
                addr,
                name,
                category: "LCD",
                raw_value: val,
                summary: format!("0x{:04X} (signed {})", val as u16, val as i16),
                fields: vec![IoField {
                    name: "Fixed-Point Component",
                    bit_range: "0..=15",
                    value: val & 0xFFFF,
                    meaning: format!("0x{:04X}", val & 0xFFFF),
                }],
            })
        }
        0x0400_0040 | 0x0400_0042 => {
            let name = if addr == 0x0400_0040 { "WIN0H" } else { "WIN1H" };
            let x2 = bits(val, 0, 8);
            let x1 = bits(val, 8, 8);
            Some(IoRegDesc {
                addr,
                name,
                category: "LCD",
                raw_value: val,
                summary: format!("X: {x1}..{x2}"),
                fields: vec![
                    IoField {
                        name: "Left (X1)",
                        bit_range: "8..=15",
                        value: x1,
                        meaning: format!("{x1}"),
                    },
                    IoField {
                        name: "Right (X2)",
                        bit_range: "0..=7",
                        value: x2,
                        meaning: format!("{x2}"),
                    },
                ],
            })
        }
        0x0400_0044 | 0x0400_0046 => {
            let name = if addr == 0x0400_0044 { "WIN0V" } else { "WIN1V" };
            let y2 = bits(val, 0, 8);
            let y1 = bits(val, 8, 8);
            Some(IoRegDesc {
                addr,
                name,
                category: "LCD",
                raw_value: val,
                summary: format!("Y: {y1}..{y2}"),
                fields: vec![
                    IoField {
                        name: "Top (Y1)",
                        bit_range: "8..=15",
                        value: y1,
                        meaning: format!("{y1}"),
                    },
                    IoField {
                        name: "Bottom (Y2)",
                        bit_range: "0..=7",
                        value: y2,
                        meaning: format!("{y2}"),
                    },
                ],
            })
        }
        0x0400_0048 => {
            let w0 = window_mask_desc(bits(val, 0, 6));
            let w1 = window_mask_desc(bits(val, 8, 6));
            Some(IoRegDesc {
                addr,
                name: "WININ",
                category: "LCD",
                raw_value: val,
                summary: format!("WIN0=[{w0}] WIN1=[{w1}]"),
                fields: vec![
                    IoField {
                        name: "WIN0 Layers",
                        bit_range: "0..=5",
                        value: bits(val, 0, 6),
                        meaning: w0,
                    },
                    IoField {
                        name: "WIN1 Layers",
                        bit_range: "8..=13",
                        value: bits(val, 8, 6),
                        meaning: w1,
                    },
                ],
            })
        }
        0x0400_004A => {
            let out = window_mask_desc(bits(val, 0, 6));
            let objw = window_mask_desc(bits(val, 8, 6));
            Some(IoRegDesc {
                addr,
                name: "WINOUT",
                category: "LCD",
                raw_value: val,
                summary: format!("OUT=[{out}] OBJWIN=[{objw}]"),
                fields: vec![
                    IoField {
                        name: "Outside Layers",
                        bit_range: "0..=5",
                        value: bits(val, 0, 6),
                        meaning: out,
                    },
                    IoField {
                        name: "OBJWIN Layers",
                        bit_range: "8..=13",
                        value: bits(val, 8, 6),
                        meaning: objw,
                    },
                ],
            })
        }
        0x0400_004C => Some(IoRegDesc {
            addr,
            name: "MOSAIC",
            category: "LCD",
            raw_value: val,
            summary: format!(
                "BG={}x{} OBJ={}x{}",
                bits(val, 0, 4) + 1,
                bits(val, 4, 4) + 1,
                bits(val, 8, 4) + 1,
                bits(val, 12, 4) + 1
            ),
            fields: vec![
                IoField {
                    name: "BG H Size-1",
                    bit_range: "0..=3",
                    value: bits(val, 0, 4),
                    meaning: format!("{} px", bits(val, 0, 4) + 1),
                },
                IoField {
                    name: "BG V Size-1",
                    bit_range: "4..=7",
                    value: bits(val, 4, 4),
                    meaning: format!("{} px", bits(val, 4, 4) + 1),
                },
                IoField {
                    name: "OBJ H Size-1",
                    bit_range: "8..=11",
                    value: bits(val, 8, 4),
                    meaning: format!("{} px", bits(val, 8, 4) + 1),
                },
                IoField {
                    name: "OBJ V Size-1",
                    bit_range: "12..=15",
                    value: bits(val, 12, 4),
                    meaning: format!("{} px", bits(val, 12, 4) + 1),
                },
            ],
        }),
        0x0400_0050 => Some(decode_bldcnt(addr, val)),
        0x0400_0052 => {
            let eva = bits(val, 0, 5);
            let evb = bits(val, 8, 5);
            Some(IoRegDesc {
                addr,
                name: "BLDALPHA",
                category: "LCD",
                raw_value: val,
                summary: format!("EVA={eva}/16 EVB={evb}/16"),
                fields: vec![
                    IoField {
                        name: "EVA Coefficient",
                        bit_range: "0..=4",
                        value: eva,
                        meaning: format!("{eva}/16"),
                    },
                    IoField {
                        name: "EVB Coefficient",
                        bit_range: "8..=12",
                        value: evb,
                        meaning: format!("{evb}/16"),
                    },
                ],
            })
        }
        0x0400_0054 => {
            let evy = bits(val, 0, 5);
            Some(IoRegDesc {
                addr,
                name: "BLDY",
                category: "LCD",
                raw_value: val,
                summary: format!("EVY={evy}/16"),
                fields: vec![IoField {
                    name: "EVY Coefficient",
                    bit_range: "0..=4",
                    value: evy,
                    meaning: format!("{evy}/16"),
                }],
            })
        }
        0x0400_0060 => Some(IoRegDesc {
            addr,
            name: "SOUND1CNT_L",
            category: "Sound",
            raw_value: val,
            summary: format!(
                "SweepTime={} {} Shift={}",
                bits(val, 4, 3),
                if bit(val, 3) { "Down" } else { "Up" },
                bits(val, 0, 3)
            ),
            fields: vec![
                IoField {
                    name: "Sweep Shift",
                    bit_range: "0..=2",
                    value: bits(val, 0, 3),
                    meaning: format!("{}", bits(val, 0, 3)),
                },
                IoField {
                    name: "Sweep Dir",
                    bit_range: "3",
                    value: bits(val, 3, 1),
                    meaning: String::from(if bit(val, 3) { "Decrease" } else { "Increase" }),
                },
                IoField {
                    name: "Sweep Time",
                    bit_range: "4..=6",
                    value: bits(val, 4, 3),
                    meaning: format!("{} steps", bits(val, 4, 3)),
                },
            ],
        }),
        0x0400_0062 | 0x0400_0068 => Some(IoRegDesc {
            addr,
            name: if addr == 0x0400_0062 {
                "SOUND1CNT_H"
            } else {
                "SOUND2CNT_L"
            },
            category: "Sound",
            raw_value: val,
            summary: format!(
                "Duty={} EnvVol={}",
                match bits(val, 6, 2) {
                    0 => "12.5%",
                    1 => "25%",
                    2 => "50%",
                    _ => "75%",
                },
                bits(val, 12, 4)
            ),
            fields: vec![
                IoField {
                    name: "Sound Length",
                    bit_range: "0..=5",
                    value: bits(val, 0, 6),
                    meaning: format!("{}", bits(val, 0, 6)),
                },
                IoField {
                    name: "Wave Duty",
                    bit_range: "6..=7",
                    value: bits(val, 6, 2),
                    meaning: String::from(match bits(val, 6, 2) {
                        0 => "12.5%",
                        1 => "25%",
                        2 => "50%",
                        _ => "75%",
                    }),
                },
                IoField {
                    name: "Envelope Step",
                    bit_range: "8..=10",
                    value: bits(val, 8, 3),
                    meaning: format!("{}", bits(val, 8, 3)),
                },
                IoField {
                    name: "Envelope Init Vol",
                    bit_range: "12..=15",
                    value: bits(val, 12, 4),
                    meaning: format!("{}/15", bits(val, 12, 4)),
                },
            ],
        }),
        0x0400_0064 | 0x0400_006C | 0x0400_0074 => {
            let name = match addr {
                0x0400_0064 => "SOUND1CNT_X",
                0x0400_006C => "SOUND2CNT_H",
                _ => "SOUND3CNT_X",
            };
            Some(IoRegDesc {
                addr,
                name,
                category: "Sound",
                raw_value: val,
                summary: format!(
                    "Freq={} Timed={}",
                    bits(val, 0, 11),
                    on_off(bit(val, 14))
                ),
                fields: vec![
                    IoField {
                        name: "Rate / Freq",
                        bit_range: "0..=10",
                        value: bits(val, 0, 11),
                        meaning: format!("{}", bits(val, 0, 11)),
                    },
                    IoField {
                        name: "Timed Mode",
                        bit_range: "14",
                        value: bits(val, 14, 1),
                        meaning: on_off(bit(val, 14)),
                    },
                    IoField {
                        name: "Trigger/Init",
                        bit_range: "15",
                        value: bits(val, 15, 1),
                        meaning: on_off(bit(val, 15)),
                    },
                ],
            })
        }
        0x0400_0070 => Some(IoRegDesc {
            addr,
            name: "SOUND3CNT_L",
            category: "Sound",
            raw_value: val,
            summary: format!(
                "Ch3={} BankMode={}",
                on_off(bit(val, 7)),
                if bit(val, 5) { "64-step" } else { "32-step" }
            ),
            fields: vec![
                IoField {
                    name: "Dimension",
                    bit_range: "5",
                    value: bits(val, 5, 1),
                    meaning: String::from(if bit(val, 5) { "Two Banks (64)" } else { "One Bank (32)" }),
                },
                IoField {
                    name: "Bank Select",
                    bit_range: "6",
                    value: bits(val, 6, 1),
                    meaning: format!("Bank {}", bits(val, 6, 1)),
                },
                IoField {
                    name: "Playback Enable",
                    bit_range: "7",
                    value: bits(val, 7, 1),
                    meaning: on_off(bit(val, 7)),
                },
            ],
        }),
        0x0400_0072 => Some(IoRegDesc {
            addr,
            name: "SOUND3CNT_H",
            category: "Sound",
            raw_value: val,
            summary: format!("VolCode={}", bits(val, 13, 3)),
            fields: vec![
                IoField {
                    name: "Length",
                    bit_range: "0..=7",
                    value: bits(val, 0, 8),
                    meaning: format!("{}", bits(val, 0, 8)),
                },
                IoField {
                    name: "Volume",
                    bit_range: "13..=15",
                    value: bits(val, 13, 3),
                    meaning: format!("0x{:X}", bits(val, 13, 3)),
                },
            ],
        }),
        0x0400_0078 => Some(IoRegDesc {
            addr,
            name: "SOUND4CNT_L",
            category: "Sound",
            raw_value: val,
            summary: format!("Noise Len={} EnvVol={}", bits(val, 0, 6), bits(val, 12, 4)),
            fields: vec![
                IoField {
                    name: "Length",
                    bit_range: "0..=5",
                    value: bits(val, 0, 6),
                    meaning: format!("{}", bits(val, 0, 6)),
                },
                IoField {
                    name: "Env Vol",
                    bit_range: "12..=15",
                    value: bits(val, 12, 4),
                    meaning: format!("{}/15", bits(val, 12, 4)),
                },
            ],
        }),
        0x0400_007C => Some(IoRegDesc {
            addr,
            name: "SOUND4CNT_H",
            category: "Sound",
            raw_value: val,
            summary: format!("Shift={} Width={}", bits(val, 4, 4), if bit(val, 3) { "7-bit" } else { "15-bit" }),
            fields: vec![
                IoField {
                    name: "Dividing Ratio",
                    bit_range: "0..=2",
                    value: bits(val, 0, 3),
                    meaning: format!("{}", bits(val, 0, 3)),
                },
                IoField {
                    name: "Counter Width",
                    bit_range: "3",
                    value: bits(val, 3, 1),
                    meaning: String::from(if bit(val, 3) { "7-bit" } else { "15-bit" }),
                },
                IoField {
                    name: "Shift Clock Freq",
                    bit_range: "4..=7",
                    value: bits(val, 4, 4),
                    meaning: format!("{}", bits(val, 4, 4)),
                },
            ],
        }),
        0x0400_0080 => Some(IoRegDesc {
            addr,
            name: "SOUNDCNT_L",
            category: "Sound",
            raw_value: val,
            summary: format!(
                "DMG Vol R={}/L={} R_En=0x{:X} L_En=0x{:X}",
                bits(val, 0, 3),
                bits(val, 4, 3),
                bits(val, 8, 4),
                bits(val, 12, 4)
            ),
            fields: vec![
                IoField {
                    name: "DMG Right Vol",
                    bit_range: "0..=2",
                    value: bits(val, 0, 3),
                    meaning: format!("{}/7", bits(val, 0, 3)),
                },
                IoField {
                    name: "DMG Left Vol",
                    bit_range: "4..=6",
                    value: bits(val, 4, 3),
                    meaning: format!("{}/7", bits(val, 4, 3)),
                },
                IoField {
                    name: "Ch1-4 Right Enable",
                    bit_range: "8..=11",
                    value: bits(val, 8, 4),
                    meaning: format!("0b{:04b}", bits(val, 8, 4)),
                },
                IoField {
                    name: "Ch1-4 Left Enable",
                    bit_range: "12..=15",
                    value: bits(val, 12, 4),
                    meaning: format!("0b{:04b}", bits(val, 12, 4)),
                },
            ],
        }),
        0x0400_0082 => Some(decode_soundcnt_h(addr, val)),
        0x0400_0084 => Some(IoRegDesc {
            addr,
            name: "SOUNDCNT_X",
            category: "Sound",
            raw_value: val,
            summary: format!(
                "Master={} Active Ch=0b{:04b}",
                on_off(bit(val, 7)),
                bits(val, 0, 4)
            ),
            fields: vec![
                IoField {
                    name: "Active DMG Channels",
                    bit_range: "0..=3",
                    value: bits(val, 0, 4),
                    meaning: format!("0b{:04b}", bits(val, 0, 4)),
                },
                IoField {
                    name: "Master Sound Enable",
                    bit_range: "7",
                    value: bits(val, 7, 1),
                    meaning: on_off(bit(val, 7)),
                },
            ],
        }),
        0x0400_0088 => {
            let bias = bits(val, 1, 9);
            let rate = match bits(val, 14, 2) {
                0 => "32.768 kHz (9-bit)",
                1 => "65.536 kHz (8-bit)",
                2 => "131.072 kHz (7-bit)",
                _ => "262.144 kHz (6-bit)",
            };
            Some(IoRegDesc {
                addr,
                name: "SOUNDBIAS",
                category: "Sound",
                raw_value: val,
                summary: format!("Bias=0x{bias:03X} {rate}"),
                fields: vec![
                    IoField {
                        name: "Bias Level",
                        bit_range: "1..=9",
                        value: bias,
                        meaning: format!("0x{bias:03X}"),
                    },
                    IoField {
                        name: "Sampling Rate",
                        bit_range: "14..=15",
                        value: bits(val, 14, 2),
                        meaning: String::from(rate),
                    },
                ],
            })
        }
        0x0400_0090..=0x0400_009E if (addr & 1) == 0 => {
            let idx = (addr - 0x0400_0090) / 2;
            let name = match idx {
                0 => "WAVE_RAM0_L",
                1 => "WAVE_RAM0_H",
                2 => "WAVE_RAM1_L",
                3 => "WAVE_RAM1_H",
                4 => "WAVE_RAM2_L",
                5 => "WAVE_RAM2_H",
                6 => "WAVE_RAM3_L",
                _ => "WAVE_RAM3_H",
            };
            Some(IoRegDesc {
                addr,
                name,
                category: "Sound",
                raw_value: val,
                summary: format!("Wave samples 0x{:04X}", val & 0xFFFF),
                fields: vec![IoField {
                    name: "4-bit Samples",
                    bit_range: "0..=15",
                    value: val & 0xFFFF,
                    meaning: format!("0x{:04X}", val & 0xFFFF),
                }],
            })
        }
        0x0400_00A0 | 0x0400_00A2 | 0x0400_00A4 | 0x0400_00A6 => {
            let name = match addr {
                0x0400_00A0 => "FIFO_A_L",
                0x0400_00A2 => "FIFO_A_H",
                0x0400_00A4 => "FIFO_B_L",
                _ => "FIFO_B_H",
            };
            Some(IoRegDesc {
                addr,
                name,
                category: "Sound",
                raw_value: val,
                summary: format!("Direct Sound FIFO data 0x{val:08X}"),
                fields: vec![],
            })
        }
        0x0400_00B0..=0x0400_00DE if (addr & 1) == 0 => {
            let rel = addr - 0x0400_00B0;
            let ch = (rel / 12) as usize;
            let off = rel % 12;
            match off {
                0x0 | 0x2 => {
                    let names = [
                        ("DMA0SAD_L", "DMA0SAD_H"),
                        ("DMA1SAD_L", "DMA1SAD_H"),
                        ("DMA2SAD_L", "DMA2SAD_H"),
                        ("DMA3SAD_L", "DMA3SAD_H"),
                    ];
                    let name = if off == 0 { names[ch].0 } else { names[ch].1 };
                    Some(IoRegDesc {
                        addr,
                        name,
                        category: "DMA",
                        raw_value: val,
                        summary: format!("Source 0x{val:04X}"),
                        fields: vec![],
                    })
                }
                0x4 | 0x6 => {
                    let names = [
                        ("DMA0DAD_L", "DMA0DAD_H"),
                        ("DMA1DAD_L", "DMA1DAD_H"),
                        ("DMA2DAD_L", "DMA2DAD_H"),
                        ("DMA3DAD_L", "DMA3DAD_H"),
                    ];
                    let name = if off == 4 { names[ch].0 } else { names[ch].1 };
                    Some(IoRegDesc {
                        addr,
                        name,
                        category: "DMA",
                        raw_value: val,
                        summary: format!("Destination 0x{val:04X}"),
                        fields: vec![],
                    })
                }
                0x8 => {
                    let name = match ch {
                        0 => "DMA0CNT_L",
                        1 => "DMA1CNT_L",
                        2 => "DMA2CNT_L",
                        _ => "DMA3CNT_L",
                    };
                    Some(IoRegDesc {
                        addr,
                        name,
                        category: "DMA",
                        raw_value: val,
                        summary: format!("Word Count = {}", val & 0xFFFF),
                        fields: vec![IoField {
                            name: "Count",
                            bit_range: "0..=15",
                            value: val & 0xFFFF,
                            meaning: format!("{} units", val & 0xFFFF),
                        }],
                    })
                }
                0xA => Some(decode_dmacnt_h(addr, val, ch)),
                _ => None,
            }
        }
        0x0400_0100..=0x0400_010E if (addr & 1) == 0 => {
            let rel = addr - 0x0400_0100;
            let ch = (rel / 4) as usize;
            if (rel & 2) == 0 {
                let name = match ch {
                    0 => "TM0CNT_L",
                    1 => "TM1CNT_L",
                    2 => "TM2CNT_L",
                    _ => "TM3CNT_L",
                };
                Some(IoRegDesc {
                    addr,
                    name,
                    category: "Timer",
                    raw_value: val,
                    summary: format!("Counter/Reload = 0x{:04X} ({})", val & 0xFFFF, val & 0xFFFF),
                    fields: vec![IoField {
                        name: "Value",
                        bit_range: "0..=15",
                        value: val & 0xFFFF,
                        meaning: format!("0x{:04X}", val & 0xFFFF),
                    }],
                })
            } else {
                Some(decode_tmcnt_h(addr, val, ch))
            }
        }
        0x0400_0120..=0x0400_0126 if (addr & 1) == 0 => {
            let name = match addr {
                0x0400_0120 => "SIODATA32_L",
                0x0400_0122 => "SIODATA32_H",
                0x0400_0124 => "SIOMULTI2",
                _ => "SIOMULTI3",
            };
            Some(IoRegDesc {
                addr,
                name,
                category: "Serial",
                raw_value: val,
                summary: format!("0x{:04X}", val & 0xFFFF),
                fields: vec![],
            })
        }
        0x0400_0128 => {
            let mode = bits(val, 12, 2);
            let mode_name = match mode {
                0 | 1 => "Normal SIO",
                2 => "Multiplayer",
                _ => "UART",
            };
            Some(IoRegDesc {
                addr,
                name: "SIOCNT",
                category: "Serial",
                raw_value: val,
                summary: format!(
                    "{mode_name} Active={} IRQ={}",
                    on_off(bit(val, 7)),
                    on_off(bit(val, 14))
                ),
                fields: vec![
                    IoField {
                        name: "Start / Busy",
                        bit_range: "7",
                        value: bits(val, 7, 1),
                        meaning: on_off(bit(val, 7)),
                    },
                    IoField {
                        name: "Mode",
                        bit_range: "12..=13",
                        value: mode,
                        meaning: String::from(mode_name),
                    },
                    IoField {
                        name: "IRQ Enable",
                        bit_range: "14",
                        value: bits(val, 14, 1),
                        meaning: on_off(bit(val, 14)),
                    },
                ],
            })
        }
        0x0400_012A => Some(IoRegDesc {
            addr,
            name: "SIOMLT_SEND",
            category: "Serial",
            raw_value: val,
            summary: format!("Send Data = 0x{:04X}", val & 0xFFFF),
            fields: vec![],
        }),
        0x0400_0130 => {
            // Active-low bits: 0 means key pressed.
            let pressed_mask = (!val) & 0x03FF;
            let pressed = flag_list(pressed_mask, &KEY_NAMES);
            Some(IoRegDesc {
                addr,
                name: "KEYINPUT",
                category: "Keypad",
                raw_value: val,
                summary: format!("Pressed: {pressed}"),
                fields: vec![IoField {
                    name: "Pressed Buttons (active-low)",
                    bit_range: "0..=9",
                    value: bits(val, 0, 10),
                    meaning: pressed,
                }],
            })
        }
        0x0400_0132 => {
            let keys = flag_list(bits(val, 0, 10), &KEY_NAMES);
            Some(IoRegDesc {
                addr,
                name: "KEYCNT",
                category: "Keypad",
                raw_value: val,
                summary: format!(
                    "IRQ={} ({}) Keys=[{keys}]",
                    on_off(bit(val, 14)),
                    if bit(val, 15) { "AND" } else { "OR" }
                ),
                fields: vec![
                    IoField {
                        name: "Selected Keys",
                        bit_range: "0..=9",
                        value: bits(val, 0, 10),
                        meaning: keys,
                    },
                    IoField {
                        name: "Keypad IRQ Enable",
                        bit_range: "14",
                        value: bits(val, 14, 1),
                        meaning: on_off(bit(val, 14)),
                    },
                    IoField {
                        name: "IRQ Condition",
                        bit_range: "15",
                        value: bits(val, 15, 1),
                        meaning: String::from(if bit(val, 15) {
                            "Logical AND"
                        } else {
                            "Logical OR"
                        }),
                    },
                ],
            })
        }
        0x0400_0134 => Some(IoRegDesc {
            addr,
            name: "RCNT",
            category: "Serial",
            raw_value: val,
            summary: format!("Mode bits=0b{:02b} Raw=0x{:04X}", bits(val, 14, 2), val & 0xFFFF),
            fields: vec![IoField {
                name: "SIO/GPIO Mode",
                bit_range: "14..=15",
                value: bits(val, 14, 2),
                meaning: String::from(match bits(val, 14, 2) {
                    2 => "General Purpose I/O",
                    3 => "JOY BUS",
                    _ => "Normal/Multi/UART",
                }),
            }],
        }),
        0x0400_0140 | 0x0400_0150 | 0x0400_0152 | 0x0400_0154 | 0x0400_0156 | 0x0400_0158 => {
            let name = match addr {
                0x0400_0140 => "JOYCNT",
                0x0400_0150 => "JOY_RECV_L",
                0x0400_0152 => "JOY_RECV_H",
                0x0400_0154 => "JOY_TRANS_L",
                0x0400_0156 => "JOY_TRANS_H",
                _ => "JOYSTAT",
            };
            Some(IoRegDesc {
                addr,
                name,
                category: "Serial",
                raw_value: val,
                summary: format!("0x{:04X}", val & 0xFFFF),
                fields: vec![],
            })
        }
        0x0400_0200 => {
            let flags = flag_list(bits(val, 0, 14), &IRQ_FLAG_NAMES);
            Some(IoRegDesc {
                addr,
                name: "IE",
                category: "System",
                raw_value: val,
                summary: format!("Enabled: {flags}"),
                fields: vec![IoField {
                    name: "Interrupt Mask",
                    bit_range: "0..=13",
                    value: bits(val, 0, 14),
                    meaning: flags,
                }],
            })
        }
        0x0400_0202 => {
            let flags = flag_list(bits(val, 0, 14), &IRQ_FLAG_NAMES);
            Some(IoRegDesc {
                addr,
                name: "IF",
                category: "System",
                raw_value: val,
                summary: format!("Pending: {flags}"),
                fields: vec![IoField {
                    name: "IRQ Request Flags",
                    bit_range: "0..=13",
                    value: bits(val, 0, 14),
                    meaning: flags,
                }],
            })
        }
        0x0400_0204 => Some(decode_waitcnt(addr, val)),
        0x0400_0208 => Some(IoRegDesc {
            addr,
            name: "IME",
            category: "System",
            raw_value: val,
            summary: format!("Master IRQ Enable: {}", on_off(bit(val, 0))),
            fields: vec![IoField {
                name: "Master Enable",
                bit_range: "0",
                value: bits(val, 0, 1),
                meaning: on_off(bit(val, 0)),
            }],
        }),
        0x0400_0300 => Some(IoRegDesc {
            addr,
            name: "POSTFLG",
            category: "System",
            raw_value: val,
            summary: format!("Post-boot Flag = {}", bits(val, 0, 1)),
            fields: vec![IoField {
                name: "Boot Flag",
                bit_range: "0",
                value: bits(val, 0, 1),
                meaning: String::from(if bit(val, 0) {
                    "Post-Boot (1)"
                } else {
                    "Cold Boot (0)"
                }),
            }],
        }),
        0x0400_0301 => Some(IoRegDesc {
            addr,
            name: "HALTCNT",
            category: "System",
            raw_value: val,
            summary: String::from(if bit(val, 7) { "Stop Mode" } else { "Halt Mode" }),
            fields: vec![IoField {
                name: "Power-Down Mode",
                bit_range: "7",
                value: bits(val, 7, 1),
                meaning: String::from(if bit(val, 7) { "STOP" } else { "HALT" }),
            }],
        }),
        0x0400_0302..=0x0400_0304 if (addr & 1) == 0 => Some(IoRegDesc {
            addr,
            name: "PAUSE_CTRL",
            category: "System",
            raw_value: val,
            summary: format!("Undocumented System Control = 0x{:04X}", val & 0xFFFF),
            fields: vec![],
        }),
        0x0400_0800 => Some(IoRegDesc {
            addr,
            name: "MEMCNT",
            category: "System",
            raw_value: val,
            summary: format!(
                "WRAM Disable={} EWRAM Enable={}",
                bit(val, 0) as u8,
                bit(val, 5) as u8
            ),
            fields: vec![
                IoField {
                    name: "Disable 256K+32K WRAM",
                    bit_range: "0",
                    value: bits(val, 0, 1),
                    meaning: on_off(bit(val, 0)),
                },
                IoField {
                    name: "Enable EWRAM",
                    bit_range: "5",
                    value: bits(val, 5, 1),
                    meaning: on_off(bit(val, 5)),
                },
            ],
        }),
        _ => None,
    }
}
