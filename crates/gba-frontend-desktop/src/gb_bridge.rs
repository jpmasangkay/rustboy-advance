//! Bridge integrating `gb-core` (Game Boy / Game Boy Color) into the desktop frontend.

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use gb_core::{
    CartridgeHeader, GbCore, GbHostFrontend, GbModel, LCD_HEIGHT as GB_HEIGHT,
    LCD_WIDTH as GB_WIDTH,
};
use gba_core::{LCD_HEIGHT as GBA_HEIGHT, LCD_WIDTH as GBA_WIDTH};

use crate::app::{CoreSink, RustboyApp};
use crate::config::DmgPaletteChoice;
use crate::game_settings::CartridgeHardwareDefaults;
use crate::rom_loader::{gb_cart_type_label, gb_short_game_code, PatchFormat, RomPlatform};

/// Horizontal centering offset (in pixels) of the 160x144 GB screen inside the 240x160 buffer.
pub(crate) const GB_VIEW_OFFSET_X: usize = (GBA_WIDTH - GB_WIDTH) / 2;
/// Vertical centering offset (in pixels) of the 160x144 GB screen inside the 240x160 buffer.
pub(crate) const GB_VIEW_OFFSET_Y: usize = (GBA_HEIGHT - GB_HEIGHT) / 2;

const MAX_OFFLINE_RTC_DAYS_SECS: u64 = 512 * 86_400;

/// Active Game Boy / Game Boy Color emulation session.
pub(crate) struct GbSession {
    pub(crate) core: GbCore,
    pub(crate) platform: RomPlatform,
    pub(crate) cart_type_label: String,
    pub(crate) short_code: String,
}

impl GbSession {
    /// Creates a new GB/GBC session from raw ROM bytes and applies the user's DMG palette.
    pub(crate) fn new(rom_bytes: Vec<u8>, palette: DmgPaletteChoice) -> Self {
        let hdr_len = rom_bytes.len().min(0x150);
        let mut hdr_copy = [0u8; 0x150];
        hdr_copy[..hdr_len].copy_from_slice(&rom_bytes[..hdr_len]);

        let hdr = CartridgeHeader::parse(&hdr_copy[..hdr_len]);
        let cart_type_label = gb_cart_type_label(hdr.cart_type).to_string();

        let mut core = GbCore::new(rom_bytes, None, GbModel::Auto);
        core.set_dmg_palette(palette.to_core());

        let is_cgb = core.is_cgb();
        let platform = if is_cgb {
            RomPlatform::Gbc
        } else {
            RomPlatform::Gb
        };
        let short_code = gb_short_game_code(&hdr_copy[..hdr_len], is_cgb);

        Self {
            core,
            platform,
            cart_type_label,
            short_code,
        }
    }

    /// Returns `true` when this cartridge carries an MBC3 Real-Time Clock.
    pub(crate) fn has_rtc(&self) -> bool {
        matches!(
            self.core.bus.cart.mbc,
            gb_core::cartridge::MbcType::Mbc3 { has_rtc: true }
        )
    }

    /// Loads `.sav` battery SRAM + 48-byte MBC3 RTC trailer, advancing offline elapsed seconds.
    pub(crate) fn load_sav_with_rtc(&mut self, data: &[u8]) {
        self.core.load_battery_ram(data);
        if self.has_rtc() {
            let now = now_unix_secs();
            let loaded_ts = self.core.bus.cart.rtc.timestamp;
            if loaded_ts > 1_000_000_000 && now > loaded_ts {
                let elapsed = (now - loaded_ts).min(MAX_OFFLINE_RTC_DAYS_SECS);
                self.core.advance_rtc_seconds(elapsed);
            }
            self.core.bus.cart.rtc.timestamp = now;
        }
    }

    /// Exports battery SRAM (and refreshes the MBC3 RTC unix timestamp before writing).
    pub(crate) fn export_sav_with_rtc(&mut self) -> Vec<u8> {
        if self.has_rtc() {
            self.core.bus.cart.rtc.timestamp = now_unix_secs();
        }
        self.core.battery_ram()
    }
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Centers a 160x144 BGR555 Game Boy framebuffer inside a 240x160 buffer.
///
/// Keeping the desktop post-processor at 240x160 preserves ghosting, scanlines,
/// and 240x160 state thumbnails while `hud.rs` crops the active 160x144 region via UVs.
pub(crate) fn blit_gb_to_gba_fb(
    gb_fb: &[u16; GB_WIDTH * GB_HEIGHT],
    out: &mut [u16; GBA_WIDTH * GBA_HEIGHT],
) {
    out.fill(0);
    for y in 0..GB_HEIGHT {
        let src_start = y * GB_WIDTH;
        let dst_start = (y + GB_VIEW_OFFSET_Y) * GBA_WIDTH + GB_VIEW_OFFSET_X;
        out[dst_start..dst_start + GB_WIDTH]
            .copy_from_slice(&gb_fb[src_start..src_start + GB_WIDTH]);
    }
}

/// Extracts the centered 160x144 RGBA image out of a 240x160 RGBA post-processed frame.
pub(crate) fn crop_gb_rgba_from_240x160(rgba_240x160: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(GB_WIDTH * GB_HEIGHT * 4);
    for y in 0..GB_HEIGHT {
        let row_start = ((y + GB_VIEW_OFFSET_Y) * GBA_WIDTH + GB_VIEW_OFFSET_X) * 4;
        let row_end = row_start + GB_WIDTH * 4;
        if row_end <= rgba_240x160.len() {
            out.extend_from_slice(&rgba_240x160[row_start..row_end]);
        } else {
            out.resize(out.len() + GB_WIDTH * 4, 0);
        }
    }
    out
}

struct GbHostAdapter<'a> {
    sink: &'a mut CoreSink,
}

impl GbHostFrontend for GbHostAdapter<'_> {
    fn present_frame(&mut self, framebuffer_bgr555: &[u16; GB_WIDTH * GB_HEIGHT]) {
        self.sink
            .prev_framebuffer
            .copy_from_slice(&self.sink.framebuffer);
        blit_gb_to_gba_fb(framebuffer_bgr555, &mut self.sink.framebuffer);
        self.sink.has_prev_frame = true;
        self.sink.frame_seq = self.sink.frame_seq.wrapping_add(1);
        self.sink.frame_dirty = true;
    }

    fn push_audio_samples(&mut self, samples_interleaved_i16: &[i16]) {
        if !self.sink.suppress_audio {
            self.sink.audio.push_interleaved(samples_interleaved_i16);
        }
        if self.sink.capture_audio {
            self.sink
                .captured_audio
                .extend_from_slice(samples_interleaved_i16);
        }
    }

    fn poll_keypad(&mut self) -> u16 {
        self.sink.keypad
    }
}

impl RustboyApp {
    /// Returns `true` when either a GBA or GB/GBC ROM is loaded.
    pub(crate) fn has_active_game(&self) -> bool {
        self.core.is_some() || self.gb.is_some()
    }

    /// Returns `true` when a Game Boy / Game Boy Color game is active.
    pub(crate) fn is_gb_active(&self) -> bool {
        self.gb.is_some()
    }

    /// Human-readable active system badge (`GBA`, `GB`, `GBC`, or `NO ROM`).
    pub(crate) fn active_platform_badge(&self) -> &'static str {
        if let Some(gb) = &self.gb {
            gb.platform.badge()
        } else if self.core.is_some() {
            "GBA"
        } else {
            "NO ROM"
        }
    }

    /// Serializes a state snapshot from whichever core (GBA or GB/GBC) is active.
    pub(crate) fn save_active_core_state(&self) -> Result<Vec<u8>, String> {
        if let Some(core) = &self.core {
            core.save_state().map_err(|e| format!("{e:?}"))
        } else if let Some(gb) = &self.gb {
            gb.core.save_state().map_err(|e| format!("{e:?}"))
        } else {
            Err("No active ROM loaded".into())
        }
    }

    /// Serializes an uncompressed raw state snapshot (no LZ4, no header) from whichever core is active.
    /// Ideal for in-memory rewind buffers where XOR deltas between consecutive raw states compress
    /// drastically better and avoid duplicate compression overhead.
    pub(crate) fn save_active_core_state_raw(&self) -> Result<Vec<u8>, String> {
        if let Some(core) = &self.core {
            core.save_state_raw().map_err(|e| format!("{e:?}"))
        } else if let Some(gb) = &self.gb {
            gb.core.save_state().map_err(|e| format!("{e:?}"))
        } else {
            Err("No active ROM loaded".into())
        }
    }

    /// Loads a serialized core state snapshot into the active core and refreshes the framebuffer.
    pub(crate) fn load_active_core_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        if let Some(core) = &mut self.core {
            core.load_state(bytes).map_err(|e| format!("{e:?}"))?;
            core.bus.ppu.dot_accurate_mode = self.config.dot_accurate_ppu;
            core.bus.ppu.layer_enable_mask = self.config.layer_mask;
            core.bus.apu.channel_enable_mask = self.config.audio_channel_mask;
            core.bus.apu.master_volume_pct = self.config.master_volume;
            self.sink
                .framebuffer
                .copy_from_slice(&*core.bus.ppu.framebuffer);
            self.sink.frame_dirty = true;
            Ok(())
        } else if let Some(gb) = &mut self.gb {
            gb.core.load_state(bytes).map_err(|e| format!("{e:?}"))?;
            gb.core.set_dmg_palette(self.config.dmg_palette.to_core());
            blit_gb_to_gba_fb(gb.core.framebuffer(), &mut self.sink.framebuffer);
            self.sink.frame_dirty = true;
            Ok(())
        } else {
            Err("No active ROM loaded".into())
        }
    }

    /// Restores a raw state snapshot into the active core and refreshes PPU/APU state and framebuffer.
    pub(crate) fn load_active_core_state_raw(&mut self, bytes: &[u8]) -> Result<(), String> {
        if let Some(core) = &mut self.core {
            core.load_state_raw(bytes).map_err(|e| format!("{e:?}"))?;
            core.bus.ppu.dot_accurate_mode = self.config.dot_accurate_ppu;
            core.bus.ppu.layer_enable_mask = self.config.layer_mask;
            core.bus.apu.channel_enable_mask = self.config.audio_channel_mask;
            core.bus.apu.master_volume_pct = self.config.master_volume;
            self.sink
                .framebuffer
                .copy_from_slice(&*core.bus.ppu.framebuffer);
            self.sink.frame_dirty = true;
            Ok(())
        } else if let Some(gb) = &mut self.gb {
            gb.core.load_state(bytes).map_err(|e| format!("{e:?}"))?;
            gb.core.set_dmg_palette(self.config.dmg_palette.to_core());
            blit_gb_to_gba_fb(gb.core.framebuffer(), &mut self.sink.framebuffer);
            self.sink.frame_dirty = true;
            Ok(())
        } else {
            Err("No active ROM loaded".into())
        }
    }

    /// Applies the chosen DMG color palette to the active GB core (if monochrome DMG) and config.
    pub(crate) fn apply_dmg_palette(&mut self, choice: DmgPaletteChoice) {
        self.config.dmg_palette = choice;
        self.config.save();
        if let Some(gb) = &mut self.gb {
            gb.core.set_dmg_palette(choice.to_core());
            blit_gb_to_gba_fb(gb.core.framebuffer(), &mut self.sink.framebuffer);
            self.sink.frame_dirty = true;
        }
    }

    /// Steps one Game Boy / Game Boy Color frame into `self.sink`.
    pub(crate) fn step_gb_frame(&mut self, keypad: u16, suppress_audio: bool) {
        let Some(gb) = &mut self.gb else { return };
        self.sink.keypad = keypad;
        self.sink.suppress_audio = suppress_audio;
        let mut adapter = GbHostAdapter {
            sink: &mut self.sink,
        };
        gb.core.run_frame(&mut adapter);
        self.sink.suppress_audio = false;
    }

    /// Flushes GB battery SRAM (and MBC3 RTC timestamp if `force_rtc_update`) to `.sav`.
    pub(crate) fn flush_gb_battery_save(&mut self, force_rtc_update: bool) {
        let (Some(gb), Some(sav_path)) = (&mut self.gb, &self.sav_path) else {
            return;
        };
        if !gb.core.has_battery() {
            return;
        }
        let rtc_needs_flush = force_rtc_update && gb.has_rtc() && sav_path.exists();
        if !gb.core.battery_dirty() && !rtc_needs_flush {
            return;
        }
        let data = gb.export_sav_with_rtc();
        if !data.is_empty() {
            gb.core.clear_battery_dirty();
            let path = sav_path.clone();
            std::thread::spawn(move || {
                let _ = fs::write(path, data);
            });
        }
    }

    /// Imports an external `.sav` file into the active GB/GBC cartridge.
    pub(crate) fn import_gb_battery_save(&mut self) {
        if self.gb.is_none() {
            return;
        }
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Import GB/GBC Battery Save (.sav)")
            .add_filter("Battery Save", &["sav", "srm"])
            .pick_file()
        {
            match fs::read(&path) {
                Ok(bytes) => {
                    if let Some(gb) = &mut self.gb {
                        gb.load_sav_with_rtc(&bytes);
                    }
                    self.flush_gb_battery_save(true);
                    self.notify("Imported GB/GBC Battery Save");
                }
                Err(e) => self.notify_error(&format!("Import failed: {e}")),
            }
        }
    }

    /// Exports the active GB/GBC battery SRAM (+ 48-byte MBC3 RTC trailer) to a `.sav` file.
    pub(crate) fn export_gb_battery_save(&mut self) {
        let Some(gb) = &mut self.gb else { return };
        if !gb.core.has_battery() {
            self.notify("Current GB/GBC cartridge has no battery-backed RAM");
            return;
        }
        let data = gb.export_sav_with_rtc();
        if data.is_empty() {
            self.notify("Current GB/GBC cartridge reports no save memory");
            return;
        }
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Export GB/GBC Battery Save (.sav)")
            .add_filter("Battery Save", &["sav"])
            .set_file_name(format!("{}.sav", self.rom_title))
            .save_file()
        {
            match fs::write(&path, data) {
                Ok(()) => self.notify("Exported GB/GBC Battery Save"),
                Err(e) => self.notify_error(&format!("Export failed: {e}")),
            }
        }
    }

    /// Opens an already-loaded (and optionally patched) GB/GBC ROM and resets session UI state.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open_gb_rom_loaded(
        &mut self,
        rom_path: &Path,
        sav_path: &Path,
        rom_bytes: Vec<u8>,
        archive_badge: Option<&'static str>,
        archive_inner: Option<String>,
        patch_applied: Option<PatchFormat>,
        ctx: Option<&egui::Context>,
    ) {
        self.link.disconnect_all();
        self.core = None;

        let rom_crc32 = crc32fast::hash(&rom_bytes);
        let mut session = GbSession::new(rom_bytes, self.config.dmg_palette);
        if let Ok(sav) = fs::read(sav_path) {
            session.load_sav_with_rtc(&sav);
        }

        let header_title = session.core.title().trim().to_string();
        let title = if header_title.is_empty() {
            rom_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Game Boy")
                .to_string()
        } else {
            header_title
        };
        let code = session.short_code.clone();
        let platform_badge = session.platform.badge();
        let cart_label = session.cart_type_label.clone();

        self.cart_hw_defaults = CartridgeHardwareDefaults::default();
        self.active_cheats.clear();
        blit_gb_to_gba_fb(session.core.framebuffer(), &mut self.sink.framebuffer);
        self.gb = Some(session);

        self.finalize_rom_session(
            rom_path,
            sav_path,
            title.clone(),
            code.clone(),
            rom_crc32,
            ctx,
        );

        let patch_tag = match patch_applied {
            Some(fmt) => format!(" ({} Patched)", fmt.label()),
            None => String::new(),
        };
        let archive_tag = match (archive_badge, archive_inner.as_deref()) {
            (Some(kind), Some(inner)) => format!(" [{kind}: {inner}]"),
            (Some(kind), None) => format!(" [{kind}]"),
            _ => String::new(),
        };
        self.notify(&format!(
            "Playing {title} [{platform_badge} • {cart_label}]{archive_tag}{patch_tag}"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_synthetic_gb_rom(title: &str, cgb_flag: u8, cart_type: u8) -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        // Simple infinite loop at 0x0100: nop; jr -2 (0x00, 0x18, 0xFE)
        rom[0x0100] = 0x00;
        rom[0x0101] = 0x18;
        rom[0x0102] = 0xFE;

        let tbytes = title.as_bytes();
        let copy_len = tbytes.len().min(15);
        rom[0x0134..0x0134 + copy_len].copy_from_slice(&tbytes[..copy_len]);
        rom[0x0143] = cgb_flag;
        rom[0x0147] = cart_type;
        rom[0x0148] = 0x00; // 32 KiB
        rom[0x0149] = 0x02; // 8 KiB RAM

        let mut chk = 0u8;
        for &b in &rom[0x0134..=0x014C] {
            chk = chk.wrapping_sub(b).wrapping_sub(1);
        }
        rom[0x014D] = chk;
        rom[0x014E] = 0x12;
        rom[0x014F] = 0x34;
        rom
    }

    #[test]
    fn test_gb_session_steps_frame_and_roundtrips_save_state() {
        let rom = build_synthetic_gb_rom("TESTDMG", 0x00, 0x03);
        let mut session = GbSession::new(rom, DmgPaletteChoice::ClassicGreen);
        assert_eq!(session.platform, RomPlatform::Gb);
        assert_eq!(session.cart_type_label, "MBC1+RAM+BATT");
        assert_eq!(session.short_code.len(), 4);

        struct DummyFe {
            fb: [u16; GBA_WIDTH * GBA_HEIGHT],
            frames: usize,
        }
        impl GbHostFrontend for DummyFe {
            fn present_frame(&mut self, fb: &[u16; GB_WIDTH * GB_HEIGHT]) {
                blit_gb_to_gba_fb(fb, &mut self.fb);
                self.frames += 1;
            }
            fn push_audio_samples(&mut self, _samples: &[i16]) {}
            fn poll_keypad(&mut self) -> u16 {
                0x03FF
            }
        }

        let mut fe = DummyFe {
            fb: [0x7FFF; GBA_WIDTH * GBA_HEIGHT],
            frames: 0,
        };
        session.core.run_frame(&mut fe);
        session.core.run_frame(&mut fe);
        assert_eq!(fe.frames, 2);
        // Border outside the centered 160x144 region must be cleared to 0.
        assert_eq!(fe.fb[0], 0);
        assert_eq!(fe.fb[(GBA_HEIGHT - 1) * GBA_WIDTH + (GBA_WIDTH - 1)], 0);

        let state = session.core.save_state().expect("gb save_state");
        session.core.load_state(&state).expect("gb load_state");
    }

    #[test]
    fn test_gb_rgba_crop_extracts_exact_160x144_region() {
        let mut rgba = vec![0u8; GBA_WIDTH * GBA_HEIGHT * 4];
        let tl = (GB_VIEW_OFFSET_Y * GBA_WIDTH + GB_VIEW_OFFSET_X) * 4;
        rgba[tl..tl + 4].copy_from_slice(&[11, 22, 33, 255]);
        let br = ((GB_VIEW_OFFSET_Y + GB_HEIGHT - 1) * GBA_WIDTH
            + (GB_VIEW_OFFSET_X + GB_WIDTH - 1))
            * 4;
        rgba[br..br + 4].copy_from_slice(&[44, 55, 66, 255]);

        let cropped = crop_gb_rgba_from_240x160(&rgba);
        assert_eq!(cropped.len(), GB_WIDTH * GB_HEIGHT * 4);
        assert_eq!(&cropped[0..4], &[11, 22, 33, 255]);
        let end = (GB_WIDTH * GB_HEIGHT - 1) * 4;
        assert_eq!(&cropped[end..end + 4], &[44, 55, 66, 255]);
    }
}
