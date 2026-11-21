use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn format_utc_timestamp(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let hour = rem / 3_600;
    let minute = (rem % 3_600) / 60;
    let second = rem % 60;

    // Howard Hinnant's civil-from-days algorithm (UTC).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + i64::from(month <= 2);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn format_utc_now() -> String {
    if let Ok(epoch) = std::env::var("SOURCE_DATE_EPOCH")
        && let Ok(secs) = epoch.trim().parse::<u64>()
    {
        return format_utc_timestamp(secs);
    }
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_utc_timestamp(secs)
}

fn read_git_commit() -> String {
    let rev = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output();
    let Ok(out) = rev else {
        return "unknown".to_string();
    };
    if !out.status.success() {
        return "unknown".to_string();
    }
    let hash = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if hash.is_empty() {
        return "unknown".to_string();
    }
    let dirty = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .output()
        .map(|s| s.status.success() && !String::from_utf8_lossy(&s.stdout).trim().is_empty())
        .unwrap_or(false);
    if dirty { format!("{hash}-dirty") } else { hash }
}

fn build_windows_icon_coff_obj(ico_bytes: &[u8], target_arch: &str) -> Result<Vec<u8>, String> {
    let (machine_id, reloc_type): (u16, u16) = match target_arch {
        "x86_64" => (0x8664, 0x0003), // IMAGE_FILE_MACHINE_AMD64 + IMAGE_REL_AMD64_ADDR32NB
        "aarch64" => (0xAA64, 0x0002), // IMAGE_FILE_MACHINE_ARM64 + IMAGE_REL_ARM64_ADDR32NB
        "x86" => (0x014C, 0x0007),    // IMAGE_FILE_MACHINE_I386 + IMAGE_REL_I386_DIR32NB
        other => return Err(format!("unsupported Windows COFF icon arch: {other}")),
    };
    if ico_bytes.len() < 6 {
        return Err("icon.ico too small".to_string());
    }
    let reserved = u16::from_le_bytes([ico_bytes[0], ico_bytes[1]]);
    let ico_type = u16::from_le_bytes([ico_bytes[2], ico_bytes[3]]);
    let count = u16::from_le_bytes([ico_bytes[4], ico_bytes[5]]) as usize;
    if reserved != 0 || ico_type != 1 || count == 0 || ico_bytes.len() < 6 + count * 16 {
        return Err("invalid ICO header".to_string());
    }

    // Build GRPICONDIR (RT_GROUP_ICON payload) and collect individual RT_ICON payloads.
    let mut grp_data = Vec::with_capacity(6 + count * 14);
    grp_data.extend_from_slice(&0u16.to_le_bytes());
    grp_data.extend_from_slice(&1u16.to_le_bytes());
    grp_data.extend_from_slice(&(count as u16).to_le_bytes());

    let mut payloads: Vec<Vec<u8>> = Vec::with_capacity(count + 1);
    for i in 0..count {
        let entry_off = 6 + i * 16;
        let entry = &ico_bytes[entry_off..entry_off + 16];
        let bytes_in_res = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]) as usize;
        let img_off = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]) as usize;
        if img_off.saturating_add(bytes_in_res) > ico_bytes.len() {
            return Err("ICO frame out of bounds".to_string());
        }
        // GRPICONDIRENTRY: first 12 bytes match ICONDIRENTRY, followed by u16 resource ID (1-based).
        grp_data.extend_from_slice(&entry[0..12]);
        grp_data.extend_from_slice(&((i as u16) + 1).to_le_bytes());
        payloads.push(ico_bytes[img_off..img_off + bytes_in_res].to_vec());
    }
    // Last payload is RT_GROUP_ICON (ID = 1).
    payloads.push(grp_data);
    let total_res = payloads.len(); // count RT_ICONs + 1 RT_GROUP_ICON

    // Layout of `.rsrc` section:
    // 1. Root Resource Directory (16 bytes) + 2 entries (RT_ICON=3, RT_GROUP_ICON=14) (16 bytes) = 32 bytes
    // 2. Type Subdirectories:
    //    - RT_ICON subdir: 16 bytes + count * 8 bytes
    //    - RT_GROUP_ICON subdir: 16 bytes + 1 * 8 bytes = 24 bytes
    // 3. Language Subdirectories (total_res of them, each 16 + 8 = 24 bytes)
    // 4. IMAGE_RESOURCE_DATA_ENTRY array (total_res of them, each 16 bytes)
    // 5. Raw payload bytes (8-byte aligned)
    let root_off = 0usize;
    let rt_icon_dir_off = root_off + 16 + 2 * 8;
    let rt_group_dir_off = rt_icon_dir_off + 16 + count * 8;
    let lang_dirs_off = rt_group_dir_off + 16 + 8;
    let data_entries_off = lang_dirs_off + total_res * 24;
    let raw_payloads_start = (data_entries_off + total_res * 16 + 7) & !7;

    let mut rsrc = Vec::new();
    let push_dir_header = |buf: &mut Vec<u8>, id_entries: u16| {
        buf.extend_from_slice(&0u32.to_le_bytes()); // Characteristics
        buf.extend_from_slice(&0u32.to_le_bytes()); // TimeDateStamp
        buf.extend_from_slice(&0u16.to_le_bytes()); // MajorVersion
        buf.extend_from_slice(&0u16.to_le_bytes()); // MinorVersion
        buf.extend_from_slice(&0u16.to_le_bytes()); // NumberOfNamedEntries
        buf.extend_from_slice(&id_entries.to_le_bytes()); // NumberOfIdEntries
    };
    let push_dir_entry = |buf: &mut Vec<u8>, id: u32, target_off: usize, is_subdir: bool| {
        buf.extend_from_slice(&id.to_le_bytes());
        let off_field = (target_off as u32) | if is_subdir { 0x8000_0000 } else { 0 };
        buf.extend_from_slice(&off_field.to_le_bytes());
    };

    // 1. Root directory (2 ID entries: 3 = RT_ICON, 14 = RT_GROUP_ICON)
    push_dir_header(&mut rsrc, 2);
    push_dir_entry(&mut rsrc, 3, rt_icon_dir_off, true);
    push_dir_entry(&mut rsrc, 14, rt_group_dir_off, true);
    debug_assert_eq!(rsrc.len(), rt_icon_dir_off);

    // 2a. RT_ICON directory (IDs 1..=count -> lang_dirs[0..count])
    push_dir_header(&mut rsrc, count as u16);
    for i in 0..count {
        push_dir_entry(&mut rsrc, (i as u32) + 1, lang_dirs_off + i * 24, true);
    }
    debug_assert_eq!(rsrc.len(), rt_group_dir_off);

    // 2b. RT_GROUP_ICON directory (ID 1 -> lang_dirs[count])
    push_dir_header(&mut rsrc, 1);
    push_dir_entry(&mut rsrc, 1, lang_dirs_off + count * 24, true);
    debug_assert_eq!(rsrc.len(), lang_dirs_off);

    // 3. Language directories (each has 1 entry with Language ID 0x0409 -> data_entries[i])
    for i in 0..total_res {
        push_dir_header(&mut rsrc, 1);
        push_dir_entry(&mut rsrc, 0x0409, data_entries_off + i * 16, false);
    }
    debug_assert_eq!(rsrc.len(), data_entries_off);

    // Compute payload offsets inside `.rsrc`
    let mut payload_offsets = Vec::with_capacity(total_res);
    let mut cur_off = raw_payloads_start;
    for p in &payloads {
        payload_offsets.push(cur_off);
        cur_off = (cur_off + p.len() + 7) & !7;
    }

    // 4. IMAGE_RESOURCE_DATA_ENTRY array (OffsetToData = 0, relocated via ADDR32NB symbol per payload)
    for p in &payloads {
        rsrc.extend_from_slice(&0u32.to_le_bytes()); // OffsetToData (relocated by symbol)
        rsrc.extend_from_slice(&(p.len() as u32).to_le_bytes()); // Size
        rsrc.extend_from_slice(&0u32.to_le_bytes()); // CodePage
        rsrc.extend_from_slice(&0u32.to_le_bytes()); // Reserved
    }
    while rsrc.len() < raw_payloads_start {
        rsrc.push(0);
    }

    // 5. Raw payloads
    for (i, p) in payloads.iter().enumerate() {
        debug_assert_eq!(rsrc.len(), payload_offsets[i]);
        rsrc.extend_from_slice(p);
        while (rsrc.len() & 7) != 0 {
            rsrc.push(0);
        }
    }

    // Assemble x86_64 COFF Object File:
    // - COFF File Header (20 bytes)
    // - 1 Section Header `.rsrc` (40 bytes)
    // - `.rsrc` section data (`rsrc.len()` bytes)
    // - Relocations (`total_res` entries * 10 bytes)
    // - Symbol Table (`1 + total_res` entries * 18 bytes)
    // - String Table (4 bytes: `4u32`)
    let section_data_ptr = 20 + 40;
    let relocs_ptr = section_data_ptr + rsrc.len();
    let symtab_ptr = relocs_ptr + total_res * 10;
    let num_symbols = (1 + total_res) as u32;

    let mut obj = Vec::with_capacity(symtab_ptr + (num_symbols as usize) * 18 + 4);

    // COFF File Header
    obj.extend_from_slice(&machine_id.to_le_bytes());
    obj.extend_from_slice(&1u16.to_le_bytes()); // NumberOfSections: 1
    obj.extend_from_slice(&0u32.to_le_bytes()); // TimeDateStamp
    obj.extend_from_slice(&(symtab_ptr as u32).to_le_bytes()); // PointerToSymbolTable
    obj.extend_from_slice(&num_symbols.to_le_bytes()); // NumberOfSymbols
    obj.extend_from_slice(&0u16.to_le_bytes()); // SizeOfOptionalHeader
    obj.extend_from_slice(&0u16.to_le_bytes()); // Characteristics

    // Section Header `.rsrc`
    obj.extend_from_slice(b".rsrc\0\0\0"); // Name (8 bytes)
    obj.extend_from_slice(&0u32.to_le_bytes()); // VirtualSize
    obj.extend_from_slice(&0u32.to_le_bytes()); // VirtualAddress
    obj.extend_from_slice(&(rsrc.len() as u32).to_le_bytes()); // SizeOfRawData
    obj.extend_from_slice(&(section_data_ptr as u32).to_le_bytes()); // PointerToRawData
    obj.extend_from_slice(&(relocs_ptr as u32).to_le_bytes()); // PointerToRelocations
    obj.extend_from_slice(&0u32.to_le_bytes()); // PointerToLinenumbers
    obj.extend_from_slice(&(total_res as u16).to_le_bytes()); // NumberOfRelocations
    obj.extend_from_slice(&0u16.to_le_bytes()); // NumberOfLinenumbers
    // Characteristics: IMAGE_SCN_CNT_INITIALIZED_DATA (0x40) | IMAGE_SCN_MEM_READ (0x4000_0000)
    obj.extend_from_slice(&0x4000_0040u32.to_le_bytes());

    // Section raw data
    obj.extend_from_slice(&rsrc);

    // Relocations (each 10 bytes: VirtualAddress: u32, SymbolTableIndex: u32, Type: u16)
    for i in 0..total_res {
        let va = (data_entries_off + i * 16) as u32;
        let sym_idx = (i as u32) + 1;
        obj.extend_from_slice(&va.to_le_bytes());
        obj.extend_from_slice(&sym_idx.to_le_bytes());
        obj.extend_from_slice(&reloc_type.to_le_bytes());
    }

    // Symbol Table (each 18 bytes)
    let push_symbol = |buf: &mut Vec<u8>, name: &[u8; 8], value: u32, storage_class: u8| {
        buf.extend_from_slice(name);
        buf.extend_from_slice(&value.to_le_bytes()); // Value
        buf.extend_from_slice(&1i16.to_le_bytes()); // SectionNumber = 1 (.rsrc)
        buf.extend_from_slice(&0u16.to_le_bytes()); // Type = 0
        buf.push(storage_class); // StorageClass (3 = IMAGE_SYM_CLASS_STATIC)
        buf.push(0); // NumberOfAuxSymbols = 0
    };
    push_symbol(&mut obj, b".rsrc\0\0\0", 0, 3);
    for (i, &off) in payload_offsets.iter().enumerate() {
        let mut sym_name = [0u8; 8];
        let s = format!("$R{:05}", i);
        sym_name[..s.len()].copy_from_slice(s.as_bytes());
        push_symbol(&mut obj, &sym_name, off as u32, 3);
    }

    // String Table (empty: 4-byte length = 4)
    obj.extend_from_slice(&4u32.to_le_bytes());
    Ok(obj)
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=assets/icon_64.png");
    println!("cargo:rerun-if-changed=assets/logo.png");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    let git_dir = Path::new("../../.git");
    let git_head = git_dir.join("HEAD");
    if git_head.exists() {
        println!("cargo:rerun-if-changed=../../.git/HEAD");
        if let Ok(head_text) = std::fs::read_to_string(&git_head)
            && let Some(ref_rel) = head_text.trim().strip_prefix("ref:")
        {
            let ref_rel = ref_rel.trim();
            if git_dir.join(ref_rel).exists() {
                println!("cargo:rerun-if-changed=../../.git/{ref_rel}");
            }
        }
    }
    // Give the Windows main thread a 16 MB stack so winit/eframe runs on the OS main thread
    // cleanly without needing a secondary spawned thread (which breaks macOS/Linux event loops),
    // and embed the crab-GBA icon into the Windows .exe resource table.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
        let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
        if target_env == "msvc" {
            println!("cargo:rustc-link-arg=/STACK:16777216");
        } else {
            println!("cargo:rustc-link-arg=-Wl,--stack,16777216");
        }
        let ico_path = Path::new("assets/icon.ico");
        if let Ok(ico_bytes) = std::fs::read(ico_path)
            && let Ok(coff_obj) = build_windows_icon_coff_obj(&ico_bytes, &target_arch)
            && let Ok(out_dir) = std::env::var("OUT_DIR")
        {
            let obj_path = Path::new(&out_dir).join("rustboy_icon_res.obj");
            if std::fs::write(&obj_path, coff_obj).is_ok() {
                println!(
                    "cargo:rustc-link-arg-bin=rustboy-advance={}",
                    obj_path.display()
                );
            }
        }
    }
    println!("cargo:rustc-env=RBA_GIT_COMMIT={}", read_git_commit());
    println!("cargo:rustc-env=RBA_BUILD_TIME_UTC={}", format_utc_now());
}
