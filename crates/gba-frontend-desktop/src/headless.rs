use std::fs;
use std::path::PathBuf;

use gba_core::{GbaCore, HostFrontend, LCD_HEIGHT, LCD_WIDTH};
use gba_debugger::format_registers;

pub struct CliConfig {
    pub rom_path: Option<PathBuf>,
    pub bios_path: Option<PathBuf>,
    pub sav_path: Option<PathBuf>,
    pub headless: bool,
    pub frames: usize,
    pub dump_regs: bool,
    pub screenshot_ppm: Option<PathBuf>,
    pub fb_crc32: bool,
    pub dump_ascii_grid: bool,
}

impl CliConfig {
    pub fn parse() -> Result<Self, String> {
        let mut cfg = Self {
            rom_path: None,
            bios_path: None,
            sav_path: None,
            headless: false,
            frames: 120,
            dump_regs: false,
            screenshot_ppm: None,
            fb_crc32: false,
            dump_ascii_grid: false,
        };

        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--headless" => cfg.headless = true,
                "--dump-regs" => cfg.dump_regs = true,
                "--fb-crc32" => cfg.fb_crc32 = true,
                "--ascii" => cfg.dump_ascii_grid = true,
                "--frames" => {
                    let v = args.next().ok_or("missing value for --frames")?;
                    cfg.frames = v.parse().map_err(|_| "invalid --frames number")?;
                }
                "--bios" => {
                    let v = args.next().ok_or("missing value for --bios")?;
                    cfg.bios_path = Some(PathBuf::from(v));
                }
                "--sav" => {
                    let v = args.next().ok_or("missing value for --sav")?;
                    cfg.sav_path = Some(PathBuf::from(v));
                }
                "--screenshot" => {
                    let v = args.next().ok_or("missing value for --screenshot")?;
                    cfg.screenshot_ppm = Some(PathBuf::from(v));
                }
                "--help" | "-h" => {
                    println!(
                        "Usage: gba-frontend-desktop [ROM.gba] [OPTIONS]\n\
                         Options:\n\
                         \t--headless            Run without GUI window\n\
                         \t--frames <N>          Number of frames in headless mode (default 120)\n\
                         \t--bios <path>         Path to 16KB GBA BIOS\n\
                         \t--sav <path>          Path to .sav battery file\n\
                         \t--dump-regs           Print CPU/HW registers after execution\n\
                         \t--fb-crc32            Print framebuffer FNV/CRC checksum and color summary\n\
                         \t--ascii               Print 30x20 luminance block preview of framebuffer\n\
                         \t--screenshot <file>   Write 240x160 RGB PPM screenshot"
                    );
                    std::process::exit(0);
                }
                _ if !arg.starts_with('-') && cfg.rom_path.is_none() => {
                    cfg.rom_path = Some(PathBuf::from(arg));
                }
                _ => return Err(format!("unknown argument: {arg}")),
            }
        }
        Ok(cfg)
    }
}

struct NullFrontend {
    last_fb: [u16; LCD_WIDTH * LCD_HEIGHT],
    frames_presented: usize,
    total_audio_samples: usize,
}

impl NullFrontend {
    fn new() -> Self {
        Self {
            last_fb: [0x7FFF; LCD_WIDTH * LCD_HEIGHT],
            frames_presented: 0,
            total_audio_samples: 0,
        }
    }
}

impl HostFrontend for NullFrontend {
    fn present_frame(&mut self, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
        self.frames_presented += 1;
        self.last_fb.copy_from_slice(fb);
    }

    fn push_audio_samples(&mut self, samples: &[i16]) {
        self.total_audio_samples += samples.len();
    }

    fn poll_keypad(&mut self) -> u16 {
        0x03FF
    }
}

pub fn run_headless(cfg: &CliConfig) -> Result<(), String> {
    let rom_path = cfg
        .rom_path
        .as_ref()
        .ok_or("headless mode requires a ROM path")?;
    let rom = fs::read(rom_path).map_err(|e| format!("failed to read ROM: {e}"))?;
    let bios = match &cfg.bios_path {
        Some(p) => Some(fs::read(p).map_err(|e| format!("failed to read BIOS: {e}"))?),
        None => None,
    };

    let mut core = GbaCore::new(bios.as_deref(), rom);
    if let Some(sav) = &cfg.sav_path {
        if sav.exists() {
            if let Ok(bytes) = fs::read(sav) {
                core.bus.cart.load_save_data(&bytes);
            }
        }
    }

    let mut frontend = NullFrontend::new();
    for _ in 0..cfg.frames {
        core.run_frame(&mut frontend);
    }

    if let Some(sav) = &cfg.sav_path {
        if core.bus.cart.save_dirty {
            let _ = fs::write(sav, core.bus.cart.export_save_data());
        }
    }

    println!(
        "Executed {} frames ({} audio samples, save_type={:?}, DISPCNT={:#06x})",
        frontend.frames_presented,
        frontend.total_audio_samples,
        core.bus.cart.save_type,
        core.bus.ppu.dispcnt
    );

    if cfg.dump_regs {
        print!("{}", format_registers(&core));
    }

    if cfg.fb_crc32 {
        let mut crc = 0xFFFF_FFFFu32;
        let mut non_white = 0usize;
        let mut green_dominant = 0usize;
        let mut red_dominant = 0usize;
        for &px in &frontend.last_fb {
            if px != 0x7FFF {
                non_white += 1;
            }
            let r = px & 0x1F;
            let g = (px >> 5) & 0x1F;
            let b = (px >> 10) & 0x1F;
            if g > r + 4 && g > b + 4 {
                green_dominant += 1;
            }
            if r > g + 4 && r > b + 4 {
                red_dominant += 1;
            }
            crc ^= px as u32;
            for _ in 0..16 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        crc = !crc;
        println!(
            "FB CRC32={crc:#010x} non_white={non_white} green_px={green_dominant} red_px={red_dominant}"
        );
    }

    if cfg.dump_ascii_grid {
        print_ascii_preview(&frontend.last_fb);
    }

    if let Some(ppm_path) = &cfg.screenshot_ppm {
        write_ppm(ppm_path, &frontend.last_fb)?;
        println!("Wrote screenshot to {}", ppm_path.display());
    }

    Ok(())
}

pub fn write_ppm(path: &PathBuf, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) -> Result<(), String> {
    let mut bytes = Vec::with_capacity(32 + LCD_WIDTH * LCD_HEIGHT * 3);
    bytes.extend_from_slice(format!("P6\n{} {}\n255\n", LCD_WIDTH, LCD_HEIGHT).as_bytes());
    for &px in fb {
        let r = (((px & 0x1F) * 255) / 31) as u8;
        let g = ((((px >> 5) & 0x1F) * 255) / 31) as u8;
        let b = ((((px >> 10) & 0x1F) * 255) / 31) as u8;
        bytes.push(r);
        bytes.push(g);
        bytes.push(b);
    }
    fs::write(path, &bytes).map_err(|e| format!("failed to write PPM: {e}"))
}

fn print_ascii_preview(fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
    // Downsample 240x160 -> 80x40 using blocks of 3x4 pixels for crisp terminal inspection.
    for by in 0..40 {
        let mut line = String::with_capacity(80);
        for bx in 0..80 {
            let mut sum_luma = 0u32;
            let mut min_luma = 255u32;
            for dy in 0..4 {
                for dx in 0..3 {
                    let px = fb[(by * 4 + dy) * LCD_WIDTH + (bx * 3 + dx)];
                    let r = ((px & 0x1F) * 8) as u32;
                    let g = (((px >> 5) & 0x1F) * 8) as u32;
                    let b = (((px >> 10) & 0x1F) * 8) as u32;
                    let luma = (r * 3 + g * 6 + b) / 10;
                    sum_luma += luma;
                    min_luma = min_luma.min(luma);
                }
            }
            let avg = sum_luma / 12;
            let val = if min_luma < 80 && avg > 150 { min_luma } else { avg };
            let ch = match val {
                0..=40 => '#',
                41..=100 => '+',
                101..=170 => ':',
                171..=230 => '.',
                _ => ' ',
            };
            line.push(ch);
        }
        println!("|{line}|");
    }
}

