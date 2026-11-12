use std::fs;
use std::path::Path;

use crate::app::RustboyApp;

impl RustboyApp {
    pub(crate) fn apply_ips_patch_if_present(rom_path: &Path, rom: &mut Vec<u8>) -> bool {
        let ips_path = rom_path.with_extension("ips");
        let Ok(patch) = fs::read(&ips_path) else {
            return false;
        };
        if patch.len() < 8 || &patch[0..5] != b"PATCH" {
            return false;
        }
        let mut pos = 5;
        while pos + 3 <= patch.len() {
            if &patch[pos..pos + 3] == b"EOF" {
                if pos + 6 <= patch.len() {
                    let trunc_len = ((patch[pos + 3] as usize) << 16)
                        | ((patch[pos + 4] as usize) << 8)
                        | (patch[pos + 5] as usize);
                    rom.truncate(trunc_len);
                }
                return true;
            }
            if pos + 5 > patch.len() {
                break;
            }
            let offset = ((patch[pos] as usize) << 16)
                | ((patch[pos + 1] as usize) << 8)
                | (patch[pos + 2] as usize);
            let size = ((patch[pos + 3] as usize) << 8) | (patch[pos + 4] as usize);
            pos += 5;
            if size == 0 {
                // RLE record
                if pos + 3 > patch.len() {
                    break;
                }
                let rle_len = ((patch[pos] as usize) << 8) | (patch[pos + 1] as usize);
                let val = patch[pos + 2];
                pos += 3;
                if offset + rle_len > rom.len() {
                    rom.resize(offset + rle_len, 0xFF);
                }
                rom[offset..offset + rle_len].fill(val);
            } else {
                if pos + size > patch.len() {
                    break;
                }
                if offset + size > rom.len() {
                    rom.resize(offset + size, 0xFF);
                }
                rom[offset..offset + size].copy_from_slice(&patch[pos..pos + size]);
                pos += size;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ips_soft_patching() {
        let temp_dir = std::env::temp_dir().join("rba_test_ips");
        let _ = fs::create_dir_all(&temp_dir);
        let rom_path = temp_dir.join("test_game.gba");
        let ips_path = temp_dir.join("test_game.ips");

        let mut original_rom = vec![0x00; 64];

        // Construct IPS patch: "PATCH", offset 0x000004 (3 bytes), size 0x0003 (2 bytes), data [0xAA, 0xBB, 0xCC],
        // followed by RLE: offset 0x000010 (3 bytes), size 0x0000 (2 bytes), rle_len 0x0004 (2 bytes), val 0x77,
        // then "EOF"
        let mut patch = Vec::new();
        patch.extend_from_slice(b"PATCH");
        // Standard chunk: offset 4, len 3
        patch.extend_from_slice(&[0x00, 0x00, 0x04]);
        patch.extend_from_slice(&[0x00, 0x03]);
        patch.extend_from_slice(&[0xAA, 0xBB, 0xCC]);
        // RLE chunk: offset 16, len 0 -> rle_len 4, val 0x77
        patch.extend_from_slice(&[0x00, 0x00, 0x10]);
        patch.extend_from_slice(&[0x00, 0x00]);
        patch.extend_from_slice(&[0x00, 0x04, 0x77]);
        // Terminator
        patch.extend_from_slice(b"EOF");

        fs::write(&ips_path, &patch).unwrap();

        let applied = RustboyApp::apply_ips_patch_if_present(&rom_path, &mut original_rom);
        assert!(applied);
        assert_eq!(&original_rom[4..7], &[0xAA, 0xBB, 0xCC]);
        assert_eq!(&original_rom[16..20], &[0x77, 0x77, 0x77, 0x77]);

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
