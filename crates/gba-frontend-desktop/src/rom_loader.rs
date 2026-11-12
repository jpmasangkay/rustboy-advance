use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::Path;

use crate::app::RustboyApp;

const MAX_ROM_BYTES: usize = 128 * 1024 * 1024;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum PatchFormat {
    Bps,
    Ups,
    Ips,
}

impl PatchFormat {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Bps => "BPS",
            Self::Ups => "UPS",
            Self::Ips => "IPS",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum RomSourceKind {
    Raw,
    Zip,
    SevenZip,
}

impl RomSourceKind {
    pub(crate) const fn badge(self) -> Option<&'static str> {
        match self {
            Self::Raw => None,
            Self::Zip => Some("ZIP"),
            Self::SevenZip => Some("7Z"),
        }
    }
}

pub(crate) struct LoadedRomSource {
    pub(crate) bytes: Vec<u8>,
    pub(crate) inner_name: Option<String>,
    pub(crate) source_kind: RomSourceKind,
}

pub(crate) struct RomHeaderSummary {
    pub(crate) header: [u8; 0xC0],
    pub(crate) bytes_read: usize,
    pub(crate) rom_size_bytes: u64,
    pub(crate) source_kind: RomSourceKind,
}

pub(crate) fn has_rom_extension(name_or_path: &str) -> bool {
    let ext = Path::new(name_or_path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(ext.as_str(), "gba" | "agb" | "bin")
}

pub(crate) fn is_patch_extension(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(ext.as_str(), "bps" | "ups" | "ips")
}

pub(crate) fn is_supported_rom_or_archive_extension(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(ext.as_str(), "gba" | "agb" | "bin" | "zip" | "7z")
}

/// Reads a single bijective VLQ integer used by BPS and UPS patch formats.
fn decode_vlq(data: &[u8], pos: &mut usize) -> Result<u64, String> {
    let mut result: u64 = 0;
    let mut shift: u64 = 1;
    loop {
        let b = *data
            .get(*pos)
            .ok_or_else(|| "Unexpected end of patch while reading VLQ".to_string())?;
        *pos += 1;
        let term = u64::from(b & 0x7f)
            .checked_mul(shift)
            .ok_or_else(|| "Patch VLQ overflow".to_string())?;
        result = result
            .checked_add(term)
            .ok_or_else(|| "Patch VLQ sum overflow".to_string())?;
        if (b & 0x80) != 0 {
            return Ok(result);
        }
        shift = shift
            .checked_shl(7)
            .ok_or_else(|| "Patch VLQ shift overflow".to_string())?;
        result = result
            .checked_add(shift)
            .ok_or_else(|| "Patch VLQ bias overflow".to_string())?;
    }
}

pub(crate) fn apply_ips(rom: &mut Vec<u8>, patch: &[u8]) -> Result<(), String> {
    if patch.len() < 8 || &patch[0..5] != b"PATCH" {
        return Err("Invalid IPS patch header (missing PATCH signature)".into());
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
            return Ok(());
        }
        if pos + 5 > patch.len() {
            return Err("Truncated IPS record header".into());
        }
        let offset = ((patch[pos] as usize) << 16)
            | ((patch[pos + 1] as usize) << 8)
            | (patch[pos + 2] as usize);
        let size = ((patch[pos + 3] as usize) << 8) | (patch[pos + 4] as usize);
        pos += 5;

        if size == 0 {
            if pos + 3 > patch.len() {
                return Err("Truncated IPS RLE record".into());
            }
            let rle_len = ((patch[pos] as usize) << 8) | (patch[pos + 1] as usize);
            let val = patch[pos + 2];
            pos += 3;
            let end = offset
                .checked_add(rle_len)
                .filter(|&v| v <= MAX_ROM_BYTES)
                .ok_or_else(|| "IPS RLE record target exceeds size limit".to_string())?;
            if end > rom.len() {
                rom.resize(end, 0xFF);
            }
            rom[offset..end].fill(val);
        } else {
            if pos + size > patch.len() {
                return Err("Truncated IPS data chunk".into());
            }
            let end = offset
                .checked_add(size)
                .filter(|&v| v <= MAX_ROM_BYTES)
                .ok_or_else(|| "IPS record target exceeds size limit".to_string())?;
            if end > rom.len() {
                rom.resize(end, 0xFF);
            }
            rom[offset..end].copy_from_slice(&patch[pos..pos + size]);
            pos += size;
        }
    }
    Err("IPS patch missing EOF marker".into())
}

pub(crate) fn apply_bps(source: &[u8], patch: &[u8]) -> Result<Vec<u8>, String> {
    if patch.len() < 16 || &patch[0..4] != b"BPS1" {
        return Err("Invalid BPS patch header (missing BPS1 signature)".into());
    }

    let footer_start = patch.len() - 12;
    let source_crc = u32::from_le_bytes(patch[footer_start..footer_start + 4].try_into().unwrap());
    let target_crc =
        u32::from_le_bytes(patch[footer_start + 4..footer_start + 8].try_into().unwrap());
    let patch_crc = u32::from_le_bytes(patch[patch.len() - 4..].try_into().unwrap());

    let actual_patch_crc = crc32fast::hash(&patch[..patch.len() - 4]);
    if actual_patch_crc != patch_crc {
        return Err(format!(
            "BPS patch file CRC32 mismatch (expected {patch_crc:08X}, got {actual_patch_crc:08X})"
        ));
    }

    let actual_source_crc = crc32fast::hash(source);
    if actual_source_crc != source_crc {
        return Err(format!(
            "BPS source ROM CRC32 mismatch (patch expects {source_crc:08X}, ROM is {actual_source_crc:08X})"
        ));
    }

    let mut pos = 4;
    let source_size = decode_vlq(patch, &mut pos)?;
    if source_size != source.len() as u64 {
        return Err(format!(
            "BPS source ROM size mismatch (patch expects {source_size} bytes, ROM is {} bytes)",
            source.len()
        ));
    }
    let target_size_u64 = decode_vlq(patch, &mut pos)?;
    let target_size = usize::try_from(target_size_u64)
        .ok()
        .filter(|&v| v <= MAX_ROM_BYTES)
        .ok_or_else(|| "BPS target ROM size exceeds limit".to_string())?;

    let metadata_size = decode_vlq(patch, &mut pos)? as usize;
    pos = pos
        .checked_add(metadata_size)
        .filter(|&p| p <= footer_start)
        .ok_or_else(|| "BPS metadata length out of bounds".to_string())?;

    let mut output = Vec::with_capacity(target_size);
    let mut source_rel_offset: isize = 0;
    let mut target_rel_offset: isize = 0;

    while pos < footer_start {
        let data = decode_vlq(patch, &mut pos)?;
        let cmd = data & 3;
        let len = usize::try_from((data >> 2) + 1)
            .map_err(|_| "BPS action length overflow".to_string())?;

        if output.len().saturating_add(len) > target_size {
            return Err("BPS output exceeds declared target size".into());
        }

        match cmd {
            0 => {
                // SourceRead
                let start = output.len();
                let end = start + len;
                if end > source.len() {
                    return Err("BPS SourceRead past end of source ROM".into());
                }
                output.extend_from_slice(&source[start..end]);
            }
            1 => {
                // TargetRead
                if pos + len > footer_start {
                    return Err("BPS TargetRead truncated before footer".into());
                }
                output.extend_from_slice(&patch[pos..pos + len]);
                pos += len;
            }
            2 => {
                // SourceCopy
                let off = decode_vlq(patch, &mut pos)?;
                let delta = signed_vlq_offset(off)?;
                source_rel_offset = source_rel_offset
                    .checked_add(delta)
                    .ok_or_else(|| "BPS SourceCopy offset overflow".to_string())?;
                if source_rel_offset < 0
                    || (source_rel_offset as usize).saturating_add(len) > source.len()
                {
                    return Err("BPS SourceCopy offset out of bounds".into());
                }
                let start = source_rel_offset as usize;
                output.extend_from_slice(&source[start..start + len]);
                source_rel_offset += len as isize;
            }
            3 => {
                // TargetCopy (byte-by-byte to support overlapping RLE runs)
                let off = decode_vlq(patch, &mut pos)?;
                let delta = signed_vlq_offset(off)?;
                target_rel_offset = target_rel_offset
                    .checked_add(delta)
                    .ok_or_else(|| "BPS TargetCopy offset overflow".to_string())?;
                for _ in 0..len {
                    if target_rel_offset < 0 || (target_rel_offset as usize) >= output.len() {
                        return Err("BPS TargetCopy offset out of bounds".into());
                    }
                    let byte = output[target_rel_offset as usize];
                    output.push(byte);
                    target_rel_offset += 1;
                }
            }
            _ => unreachable!(),
        }
    }

    if output.len() != target_size {
        return Err(format!(
            "BPS produced {} bytes, expected {target_size} bytes",
            output.len()
        ));
    }
    let actual_target_crc = crc32fast::hash(&output);
    if actual_target_crc != target_crc {
        return Err(format!(
            "BPS target ROM CRC32 mismatch (expected {target_crc:08X}, got {actual_target_crc:08X})"
        ));
    }
    Ok(output)
}

fn signed_vlq_offset(encoded: u64) -> Result<isize, String> {
    let mag = isize::try_from(encoded >> 1).map_err(|_| "BPS offset magnitude overflow")?;
    Ok(if (encoded & 1) != 0 { -mag } else { mag })
}

pub(crate) fn apply_ups(source: &[u8], patch: &[u8]) -> Result<Vec<u8>, String> {
    if patch.len() < 16 || &patch[0..4] != b"UPS1" {
        return Err("Invalid UPS patch header (missing UPS1 signature)".into());
    }

    let footer_start = patch.len() - 12;
    let source_crc = u32::from_le_bytes(patch[footer_start..footer_start + 4].try_into().unwrap());
    let target_crc =
        u32::from_le_bytes(patch[footer_start + 4..footer_start + 8].try_into().unwrap());
    let patch_crc = u32::from_le_bytes(patch[patch.len() - 4..].try_into().unwrap());

    let actual_patch_crc = crc32fast::hash(&patch[..patch.len() - 4]);
    if actual_patch_crc != patch_crc {
        return Err(format!(
            "UPS patch file CRC32 mismatch (expected {patch_crc:08X}, got {actual_patch_crc:08X})"
        ));
    }

    let actual_source_crc = crc32fast::hash(source);
    if actual_source_crc != source_crc {
        return Err(format!(
            "UPS source ROM CRC32 mismatch (patch expects {source_crc:08X}, ROM is {actual_source_crc:08X})"
        ));
    }

    let mut pos = 4;
    let source_size = decode_vlq(patch, &mut pos)?;
    if source_size != source.len() as u64 {
        return Err(format!(
            "UPS source ROM size mismatch (patch expects {source_size} bytes, ROM is {} bytes)",
            source.len()
        ));
    }
    let target_size_u64 = decode_vlq(patch, &mut pos)?;
    let target_size = usize::try_from(target_size_u64)
        .ok()
        .filter(|&v| v <= MAX_ROM_BYTES)
        .ok_or_else(|| "UPS target ROM size exceeds limit".to_string())?;

    let mut output = vec![0u8; target_size];
    let copy_len = source.len().min(target_size);
    output[..copy_len].copy_from_slice(&source[..copy_len]);

    let mut out_pos: usize = 0;
    while pos < footer_start {
        let rel = usize::try_from(decode_vlq(patch, &mut pos)?)
            .map_err(|_| "UPS hunk offset overflow".to_string())?;
        out_pos = out_pos
            .checked_add(rel)
            .ok_or_else(|| "UPS output cursor overflow".to_string())?;
        loop {
            if pos >= footer_start {
                return Err("UPS hunk truncated before 0x00 terminator".into());
            }
            let xor = patch[pos];
            pos += 1;
            if out_pos < output.len() && xor != 0 {
                output[out_pos] ^= xor;
            }
            out_pos = out_pos
                .checked_add(1)
                .ok_or_else(|| "UPS output cursor overflow".to_string())?;
            if xor == 0 {
                break;
            }
        }
    }

    let actual_target_crc = crc32fast::hash(&output);
    if actual_target_crc != target_crc {
        return Err(format!(
            "UPS target ROM CRC32 mismatch (expected {target_crc:08X}, got {actual_target_crc:08X})"
        ));
    }
    Ok(output)
}

pub(crate) fn apply_patch_bytes(
    format: PatchFormat,
    rom: &mut Vec<u8>,
    patch: &[u8],
) -> Result<(), String> {
    match format {
        PatchFormat::Bps => {
            let patched = apply_bps(rom, patch)?;
            *rom = patched;
            Ok(())
        }
        PatchFormat::Ups => {
            let patched = apply_ups(rom, patch)?;
            *rom = patched;
            Ok(())
        }
        PatchFormat::Ips => apply_ips(rom, patch),
    }
}

pub(crate) fn detect_patch_format(patch_path: &Path, patch_bytes: &[u8]) -> Option<PatchFormat> {
    if patch_bytes.starts_with(b"BPS1") {
        Some(PatchFormat::Bps)
    } else if patch_bytes.starts_with(b"UPS1") {
        Some(PatchFormat::Ups)
    } else if patch_bytes.starts_with(b"PATCH") {
        Some(PatchFormat::Ips)
    } else {
        let ext = patch_path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match ext.as_str() {
            "bps" => Some(PatchFormat::Bps),
            "ups" => Some(PatchFormat::Ups),
            "ips" => Some(PatchFormat::Ips),
            _ => None,
        }
    }
}

pub(crate) fn apply_patch_file(
    patch_path: &Path,
    rom: &mut Vec<u8>,
) -> Result<PatchFormat, String> {
    let patch_bytes =
        fs::read(patch_path).map_err(|e| format!("Failed reading patch '{}': {e}", patch_path.display()))?;
    let format = detect_patch_format(patch_path, &patch_bytes)
        .ok_or_else(|| "Unsupported patch file (expected .bps, .ups, or .ips)".to_string())?;
    apply_patch_bytes(format, rom, &patch_bytes)?;
    Ok(format)
}

/// Checks for sidecar `<rom>.bps`, `<rom>.ups`, then `<rom>.ips` (priority BPS > UPS > IPS).
pub(crate) fn apply_auto_patch_if_present(
    rom_path: &Path,
    rom: &mut Vec<u8>,
) -> Result<Option<PatchFormat>, String> {
    for (ext, format) in [
        ("bps", PatchFormat::Bps),
        ("ups", PatchFormat::Ups),
        ("ips", PatchFormat::Ips),
    ] {
        let candidate = rom_path.with_extension(ext);
        if candidate.is_file() {
            let patch_bytes = fs::read(&candidate)
                .map_err(|e| format!("Failed reading {} patch: {e}", format.label()))?;
            apply_patch_bytes(format, rom, &patch_bytes)?;
            return Ok(Some(format));
        }
    }
    Ok(None)
}

pub(crate) fn load_rom_bytes(path: &Path) -> Result<LoadedRomSource, String> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match ext.as_str() {
        "zip" => load_rom_from_zip(path),
        "7z" => load_rom_from_7z(path),
        _ => {
            let bytes = fs::read(path).map_err(|e| format!("Failed reading ROM: {e}"))?;
            Ok(LoadedRomSource {
                bytes,
                inner_name: None,
                source_kind: RomSourceKind::Raw,
            })
        }
    }
}

fn load_rom_from_zip(path: &Path) -> Result<LoadedRomSource, String> {
    let file = File::open(path).map_err(|e| format!("Failed opening ZIP archive: {e}"))?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|e| format!("Invalid ZIP archive: {e}"))?;

    for idx in 0..archive.len() {
        let mut entry = archive
            .by_index(idx)
            .map_err(|e| format!("Failed reading ZIP entry: {e}"))?;
        if entry.is_dir() {
            continue;
        }
        let entry_name = entry.name().to_string();
        if has_rom_extension(&entry_name) {
            if entry.size() > MAX_ROM_BYTES as u64 {
                return Err("ROM inside ZIP archive exceeds 128 MB limit".into());
            }
            let mut bytes = Vec::with_capacity((entry.size() as usize).min(MAX_ROM_BYTES));
            entry
                .read_to_end(&mut bytes)
                .map_err(|e| format!("Failed extracting '{entry_name}' from ZIP: {e}"))?;
            return Ok(LoadedRomSource {
                bytes,
                inner_name: Some(entry_name),
                source_kind: RomSourceKind::Zip,
            });
        }
    }
    Err("No .gba, .agb, or .bin ROM found inside ZIP archive".into())
}

fn load_rom_from_7z(path: &Path) -> Result<LoadedRomSource, String> {
    let mut reader = sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty())
        .map_err(|e| format!("Failed opening 7z archive: {e}"))?;

    let mut extracted: Option<(String, Vec<u8>)> = None;
    let mut too_large = false;
    reader
        .for_each_entries_with_decision(|entry, rdr| {
            if !entry.is_directory() && has_rom_extension(entry.name()) {
                if entry.size() > MAX_ROM_BYTES as u64 {
                    too_large = true;
                    return Ok(sevenz_rust2::EntryDecision::Stop);
                }
                let mut bytes = Vec::with_capacity((entry.size() as usize).min(MAX_ROM_BYTES));
                rdr.read_to_end(&mut bytes)?;
                extracted = Some((entry.name().to_string(), bytes));
                Ok(sevenz_rust2::EntryDecision::Stop)
            } else {
                Ok(sevenz_rust2::EntryDecision::Continue)
            }
        })
        .map_err(|e| format!("Failed reading 7z archive: {e}"))?;

    if too_large {
        return Err("ROM inside 7z archive exceeds 128 MB limit".into());
    }

    if let Some((name, bytes)) = extracted {
        Ok(LoadedRomSource {
            bytes,
            inner_name: Some(name),
            source_kind: RomSourceKind::SevenZip,
        })
    } else {
        Err("No .gba, .agb, or .bin ROM found inside 7z archive".into())
    }
}

pub(crate) fn read_rom_header_summary(path: &Path) -> Option<RomHeaderSummary> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match ext.as_str() {
        "zip" => {
            let file = File::open(path).ok()?;
            let mut archive = zip::ZipArchive::new(BufReader::new(file)).ok()?;
            for idx in 0..archive.len() {
                let entry = archive.by_index(idx).ok()?;
                if !entry.is_dir() && has_rom_extension(entry.name()) {
                    let size = entry.size();
                    let mut header = [0u8; 0xC0];
                    let bytes_read = read_up_to(entry, &mut header).ok()?;
                    return Some(RomHeaderSummary {
                        header,
                        bytes_read,
                        rom_size_bytes: size,
                        source_kind: RomSourceKind::Zip,
                    });
                }
            }
            None
        }
        "7z" => {
            let mut reader =
                sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty()).ok()?;
            let mut found: Option<RomHeaderSummary> = None;
            let _ = reader.for_each_entries_with_decision(|entry, rdr| {
                if !entry.is_directory() && has_rom_extension(entry.name()) {
                    let mut header = [0u8; 0xC0];
                    let bytes_read = read_up_to(rdr, &mut header)?;
                    found = Some(RomHeaderSummary {
                        header,
                        bytes_read,
                        rom_size_bytes: entry.size(),
                        source_kind: RomSourceKind::SevenZip,
                    });
                    Ok(sevenz_rust2::EntryDecision::Stop)
                } else {
                    Ok(sevenz_rust2::EntryDecision::Continue)
                }
            });
            found
        }
        _ => {
            let mut file = File::open(path).ok()?;
            let meta = file.metadata().ok()?;
            let mut header = [0u8; 0xC0];
            let bytes_read = read_up_to(&mut file, &mut header).ok()?;
            Some(RomHeaderSummary {
                header,
                bytes_read,
                rom_size_bytes: meta.len(),
                source_kind: RomSourceKind::Raw,
            })
        }
    }
}

fn read_up_to<R: Read>(mut reader: R, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = reader.read(&mut buf[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    Ok(filled)
}

impl RustboyApp {
    pub(crate) fn load_patch_dialog(&mut self, ctx: &egui::Context) {
        let Some(base_rom) = self.rom_path.clone() else {
            self.notify_error("Load a ROM before applying a patch");
            return;
        };
        if let Some(patch_path) = rfd::FileDialog::new()
            .set_title("Load ROM Patch (BPS / UPS / IPS)")
            .add_filter("ROM Patches", &["bps", "ups", "ips"])
            .pick_file()
            && let Err(e) = self.open_rom_with_patch(&base_rom, None, Some(&patch_path), Some(ctx))
        {
            self.notify_error(&e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn encode_vlq(mut value: u64, out: &mut Vec<u8>) {
        loop {
            let x = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(0x80 | x);
                break;
            }
            out.push(x);
            value -= 1;
        }
    }

    fn encode_signed_vlq(delta: isize, out: &mut Vec<u8>) {
        let mag = (delta.unsigned_abs() as u64) << 1;
        let encoded = if delta < 0 { mag | 1 } else { mag };
        encode_vlq(encoded, out);
    }

    fn finalize_bps(body: &mut Vec<u8>, source: &[u8], target: &[u8]) {
        body.extend_from_slice(&crc32fast::hash(source).to_le_bytes());
        body.extend_from_slice(&crc32fast::hash(target).to_le_bytes());
        let patch_crc = crc32fast::hash(body);
        body.extend_from_slice(&patch_crc.to_le_bytes());
    }

    #[test]
    fn test_ips_soft_patching() {
        let temp_dir = std::env::temp_dir().join("rba_test_ips");
        let _ = fs::create_dir_all(&temp_dir);
        let rom_path = temp_dir.join("test_game.gba");
        let ips_path = temp_dir.join("test_game.ips");

        let mut original_rom = vec![0x00; 64];

        let mut patch = Vec::new();
        patch.extend_from_slice(b"PATCH");
        patch.extend_from_slice(&[0x00, 0x00, 0x04]);
        patch.extend_from_slice(&[0x00, 0x03]);
        patch.extend_from_slice(&[0xAA, 0xBB, 0xCC]);
        patch.extend_from_slice(&[0x00, 0x00, 0x10]);
        patch.extend_from_slice(&[0x00, 0x00]);
        patch.extend_from_slice(&[0x00, 0x04, 0x77]);
        patch.extend_from_slice(b"EOF");

        fs::write(&ips_path, &patch).unwrap();

        let applied = apply_auto_patch_if_present(&rom_path, &mut original_rom).unwrap();
        assert_eq!(applied, Some(PatchFormat::Ips));
        assert_eq!(&original_rom[4..7], &[0xAA, 0xBB, 0xCC]);
        assert_eq!(&original_rom[16..20], &[0x77, 0x77, 0x77, 0x77]);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_bps_patching_all_commands_and_crc_checks() {
        let mut source = vec![0u8; 32];
        for (i, b) in source.iter_mut().enumerate() {
            *b = i as u8;
        }
        // Build expected target of 36 bytes exercising SourceRead, TargetRead, SourceCopy, TargetCopy (overlapping RLE)
        let mut target = Vec::new();
        target.extend_from_slice(&source[0..8]); // SourceRead(8)
        target.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]); // TargetRead(4) at offset 8
        target.extend_from_slice(&source[20..26]); // SourceCopy(6) from 20 -> now output len = 18
        // Overlapping TargetCopy(18) starting from offset 8..10 repeating: seed pattern from offset 8 ([AA, BB, CC, DD])?
        // Actually copy 8 bytes from offset 8 -> output[8..16], wait: offset 16..18 is right before cursor 18 -> repeats 2-byte pattern!
        for i in 0..18usize {
            let b = target[16 + i];
            target.push(b);
        }
        assert_eq!(target.len(), 36);

        let mut bps = Vec::new();
        bps.extend_from_slice(b"BPS1");
        encode_vlq(source.len() as u64, &mut bps);
        encode_vlq(target.len() as u64, &mut bps);
        encode_vlq(4, &mut bps); // metadata len = 4
        bps.extend_from_slice(b"META");

        // cmd 0: SourceRead len=8 -> (7 << 2) | 0
        encode_vlq((7 << 2) | 0, &mut bps);
        // cmd 1: TargetRead len=4 -> (3 << 2) | 1
        encode_vlq((3 << 2) | 1, &mut bps);
        bps.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
        // cmd 2: SourceCopy len=6 -> delta +20
        encode_vlq((5 << 2) | 2, &mut bps);
        encode_signed_vlq(20, &mut bps);
        // cmd 3: TargetCopy len=18 -> delta +16
        encode_vlq((17 << 2) | 3, &mut bps);
        encode_signed_vlq(16, &mut bps);

        finalize_bps(&mut bps, &source, &target);

        let result = apply_bps(&source, &bps).unwrap();
        assert_eq!(result, target);

        // Corrupt source -> error
        let mut wrong_source = source.clone();
        wrong_source[0] ^= 0xFF;
        assert!(apply_bps(&wrong_source, &bps).is_err());

        // Corrupt patch byte -> patch CRC error
        let mut bad_patch = bps.clone();
        bad_patch[8] ^= 0x01;
        assert!(apply_bps(&source, &bad_patch).is_err());
    }

    #[test]
    fn test_ups_patching_xor_and_terminator_semantics() {
        let source = b"RUSTBOY_GBA_ROM_DATA_SOURCE_0001".to_vec();
        let mut target = source.clone();
        // Mutate bytes 4..7 and byte 16..18, plus expand by 4 bytes
        target[4] = b'X';
        target[5] = b'Y';
        target[6] = b'Z';
        target[16] ^= 0x5A;
        target[17] ^= 0xC3;
        target.extend_from_slice(b"TAIL");

        let mut ups = Vec::new();
        ups.extend_from_slice(b"UPS1");
        encode_vlq(source.len() as u64, &mut ups);
        encode_vlq(target.len() as u64, &mut ups);

        // Hunk 1 at offset 4: xor 3 bytes, then 0x00 terminator (advances out_pos to 8)
        encode_vlq(4, &mut ups);
        ups.push(source[4] ^ target[4]);
        ups.push(source[5] ^ target[5]);
        ups.push(source[6] ^ target[6]);
        ups.push(0x00);

        // Hunk 2 at offset 16: rel = 16 - 8 = 8
        encode_vlq(8, &mut ups);
        ups.push(0x5A);
        ups.push(0xC3);
        ups.push(0x00); // out_pos becomes 19

        // Hunk 3 at offset 32: rel = 32 - 19 = 13
        encode_vlq(13, &mut ups);
        ups.extend_from_slice(b"TAIL");
        ups.push(0x00);

        finalize_bps(&mut ups, &source, &target);

        let patched = apply_ups(&source, &ups).unwrap();
        assert_eq!(patched, target);

        let mut wrong_source = source.clone();
        wrong_source[1] ^= 1;
        assert!(apply_ups(&wrong_source, &ups).is_err());
    }

    #[test]
    fn test_auto_patch_priority_bps_over_ups_over_ips() {
        let temp_dir = std::env::temp_dir().join("rba_test_patch_prio");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();
        let rom_path = temp_dir.join("game.gba");

        let source = vec![0x10u8; 16];
        let mut target_bps = source.clone();
        target_bps[0] = 0xB1;
        let mut target_ups = source.clone();
        target_ups[0] = 0xA2;

        // Write IPS changing byte 0 to 0x13
        let mut ips = Vec::new();
        ips.extend_from_slice(b"PATCH");
        ips.extend_from_slice(&[0, 0, 0, 0, 1, 0x13]);
        ips.extend_from_slice(b"EOF");
        fs::write(rom_path.with_extension("ips"), &ips).unwrap();

        // Write UPS changing byte 0 to 0xA2
        let mut ups = Vec::new();
        ups.extend_from_slice(b"UPS1");
        encode_vlq(16, &mut ups);
        encode_vlq(16, &mut ups);
        encode_vlq(0, &mut ups);
        ups.push(0x10 ^ 0xA2);
        ups.push(0x00);
        finalize_bps(&mut ups, &source, &target_ups);
        fs::write(rom_path.with_extension("ups"), &ups).unwrap();

        // Write BPS changing byte 0 to 0xB1
        let mut bps = Vec::new();
        bps.extend_from_slice(b"BPS1");
        encode_vlq(16, &mut bps);
        encode_vlq(16, &mut bps);
        encode_vlq(0, &mut bps);
        encode_vlq((0 << 2) | 1, &mut bps); // TargetRead(1)
        bps.push(0xB1);
        encode_vlq((14 << 2) | 0, &mut bps); // SourceRead(15)
        finalize_bps(&mut bps, &source, &target_bps);
        fs::write(rom_path.with_extension("bps"), &bps).unwrap();

        let mut buf = source.clone();
        let fmt = apply_auto_patch_if_present(&rom_path, &mut buf).unwrap();
        assert_eq!(fmt, Some(PatchFormat::Bps));
        assert_eq!(buf[0], 0xB1);

        // Remove BPS -> UPS should win over IPS
        fs::remove_file(rom_path.with_extension("bps")).unwrap();
        let mut buf2 = source.clone();
        let fmt2 = apply_auto_patch_if_present(&rom_path, &mut buf2).unwrap();
        assert_eq!(fmt2, Some(PatchFormat::Ups));
        assert_eq!(buf2[0], 0xA2);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_zip_archive_rom_extraction_and_header_summary() {
        let temp_dir = std::env::temp_dir().join("rba_test_zip_rom");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();
        let zip_path = temp_dir.join("bundle.zip");

        let mut rom = vec![0u8; 256];
        rom[0xA0..0xAC].copy_from_slice(b"TESTZIPGAME\0");
        rom[0xAC..0xB0].copy_from_slice(b"BTZE");

        {
            let f = File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zw.start_file("readme.txt", opts).unwrap();
            zw.write_all(b"Not a ROM").unwrap();
            zw.start_file("folder/test_game.gba", opts).unwrap();
            zw.write_all(&rom).unwrap();
            zw.finish().unwrap();
        }

        let summary = read_rom_header_summary(&zip_path).unwrap();
        assert_eq!(summary.source_kind, RomSourceKind::Zip);
        assert_eq!(summary.rom_size_bytes, 256);
        assert_eq!(&summary.header[0xAC..0xB0], b"BTZE");

        let loaded = load_rom_bytes(&zip_path).unwrap();
        assert_eq!(loaded.source_kind, RomSourceKind::Zip);
        assert_eq!(loaded.inner_name.as_deref(), Some("folder/test_game.gba"));
        assert_eq!(loaded.bytes, rom);

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
