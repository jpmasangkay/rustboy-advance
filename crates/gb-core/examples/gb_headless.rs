use std::env;
use std::fs;
use std::path::PathBuf;
use std::process;

use gb_core::{DmgPalette, GbConfig, GbCore, GbHostFrontend, GbModel, LCD_HEIGHT, LCD_WIDTH};
use image::{ImageBuffer, Rgb};

struct HeadlessFrontend {
    last_fb: [u16; LCD_WIDTH * LCD_HEIGHT],
    audio_samples: usize,
}

impl HeadlessFrontend {
    fn new() -> Self {
        Self {
            last_fb: [0; LCD_WIDTH * LCD_HEIGHT],
            audio_samples: 0,
        }
    }
}

impl GbHostFrontend for HeadlessFrontend {
    fn present_frame(&mut self, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
        self.last_fb.copy_from_slice(fb);
    }

    fn push_audio_samples(&mut self, samples: &[i16]) {
        self.audio_samples += samples.len();
    }

    fn poll_keypad(&mut self) -> u16 {
        0x03FF
    }
}

#[inline]
fn bgr555_to_rgb8(c: u16) -> [u8; 3] {
    let r5 = (c & 0x1F) as u8;
    let g5 = ((c >> 5) & 0x1F) as u8;
    let b5 = ((c >> 10) & 0x1F) as u8;
    [
        (r5 << 3) | (r5 >> 2),
        (g5 << 3) | (g5 >> 2),
        (b5 << 3) | (b5 >> 2),
    ]
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "Usage: {} <rom.gb|rom.gbc> [frames=120] [out.png=gb_screenshot.png] [--dmg|--cgb] [--gba-palette]",
            args[0]
        );
        process::exit(1);
    }

    let rom_path = PathBuf::from(&args[1]);
    let mut frames: usize = 120;
    let mut out_path = PathBuf::from("gb_screenshot.png");
    let mut model = GbModel::Auto;
    let mut palette = DmgPalette::Greyscale;

    let mut pos = 0usize;
    for arg in &args[2..] {
        match arg.as_str() {
            "--dmg" => model = GbModel::Dmg,
            "--cgb" => model = GbModel::Cgb,
            "--gba-palette" => palette = DmgPalette::GbaColorized,
            _ => {
                if pos == 0
                    && let Ok(n) = arg.parse::<usize>()
                {
                    frames = n;
                    pos += 1;
                    continue;
                }
                out_path = PathBuf::from(arg);
                pos += 1;
            }
        }
    }

    let rom = fs::read(&rom_path).unwrap_or_else(|e| {
        eprintln!("Failed to read ROM {}: {e}", rom_path.display());
        process::exit(1);
    });

    let config = GbConfig {
        model,
        dmg_palette: palette,
        ..GbConfig::default()
    };
    let mut core = GbCore::new_with_config(rom, None, config);
    let mut fe = HeadlessFrontend::new();

    for _ in 0..frames {
        core.run_frame(&mut fe);
    }

    let mut img = ImageBuffer::<Rgb<u8>, Vec<u8>>::new(LCD_WIDTH as u32, LCD_HEIGHT as u32);
    for y in 0..LCD_HEIGHT {
        for x in 0..LCD_WIDTH {
            let rgb = bgr555_to_rgb8(fe.last_fb[y * LCD_WIDTH + x]);
            img.put_pixel(x as u32, y as u32, Rgb(rgb));
        }
    }

    if let Some(parent) = out_path.parent()
        && !parent.as_os_str().is_empty()
    {
        let _ = fs::create_dir_all(parent);
    }
    img.save(&out_path).unwrap_or_else(|e| {
        eprintln!("Failed to save PNG {}: {e}", out_path.display());
        process::exit(1);
    });

    println!(
        "Title: {:?} | Mode: {} | Frames: {} | Audio samples: {} | Screenshot: {}",
        core.title(),
        if core.is_cgb() { "CGB" } else { "DMG" },
        frames,
        fe.audio_samples,
        out_path.display()
    );
    let serial = core.serial_output_str();
    if !serial.is_empty() {
        println!("--- Serial Output ---\n{serial}");
    }
}
