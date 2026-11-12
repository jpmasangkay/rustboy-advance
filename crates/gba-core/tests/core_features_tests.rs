use gba_core::bus::AccessType;
use gba_core::cartridge::{FlashChipId, SaveType};
use gba_core::{
    DebugControl, EventType, GbaCore, HostFrontend, MemoryRegionKind, STATE_MAGIC, STATE_VERSION,
    SaveStateError, Scheduler, SioMode, SioResponse, StopReason, Watchpoint, WatchpointKind,
    gamedb, state_info,
};

struct TestFrontend {
    keypad: u16,
    frames: usize,
    last_frame: [u16; gba_core::LCD_WIDTH * gba_core::LCD_HEIGHT],
    audio: Vec<i16>,
}

impl Default for TestFrontend {
    fn default() -> Self {
        Self {
            keypad: 0x03FF,
            frames: 0,
            last_frame: [0; gba_core::LCD_WIDTH * gba_core::LCD_HEIGHT],
            audio: Vec::new(),
        }
    }
}

impl HostFrontend for TestFrontend {
    fn present_frame(&mut self, framebuffer_bgr555: &[u16; gba_core::LCD_WIDTH * gba_core::LCD_HEIGHT]) {
        self.frames += 1;
        self.last_frame = *framebuffer_bgr555;
    }

    fn push_audio_samples(&mut self, samples_interleaved_i16: &[i16]) {
        self.audio.extend_from_slice(samples_interleaved_i16);
    }

    fn poll_keypad(&mut self) -> u16 {
        self.keypad
    }
}

fn rom_from_arm_words(words: &[u32]) -> Vec<u8> {
    let mut rom = vec![0u8; 0x200.max(words.len() * 4)];
    for (i, w) in words.iter().enumerate() {
        let bytes = w.to_le_bytes();
        rom[i * 4..i * 4 + 4].copy_from_slice(&bytes);
    }
    rom
}

fn rom_with_game_code(code: &[u8; 4], extra_words: &[(usize, u32)]) -> Vec<u8> {
    let mut rom = vec![0u8; 0x400];
    rom[0xAC..0xB0].copy_from_slice(code);
    for &(word_idx, w) in extra_words {
        let off = word_idx * 4;
        rom[off..off + 4].copy_from_slice(&w.to_le_bytes());
    }
    rom
}

#[test]
fn scheduler_all_14_slots_never_drop_and_cancel() {
    let mut sched = Scheduler::new();
    let all_events = [
        (900, EventType::PpuHDrawEnd),
        (200, EventType::PpuHBlank),
        (150, EventType::PpuDotStep),
        (400, EventType::TimerOverflow(0)),
        (350, EventType::TimerOverflow(1)),
        (800, EventType::TimerOverflow(2)),
        (500, EventType::TimerOverflow(3)),
        (100, EventType::DmaActivate(0)),
        (750, EventType::DmaActivate(1)),
        (600, EventType::DmaActivate(2)),
        (300, EventType::DmaActivate(3)),
        (450, EventType::ApuSampleTick),
        (250, EventType::IrqSynchronizer),
        (650, EventType::SioTransferComplete),
    ];

    for &(cycle, kind) in &all_events {
        sched.schedule(cycle, kind);
    }
    assert_eq!(sched.next_event_cycle, 100);

    // Cancel one variant and verify the remaining 13 still fire in sorted cycle order.
    sched.cancel(EventType::ApuSampleTick);

    sched.current_cycle = 1000;
    let mut popped = Vec::new();
    while let Some(ev) = sched.pop_due() {
        popped.push(ev);
    }

    assert_eq!(popped.len(), 13);
    let expected = vec![
        (100, EventType::DmaActivate(0)),
        (150, EventType::PpuDotStep),
        (200, EventType::PpuHBlank),
        (250, EventType::IrqSynchronizer),
        (300, EventType::DmaActivate(3)),
        (350, EventType::TimerOverflow(1)),
        (400, EventType::TimerOverflow(0)),
        (500, EventType::TimerOverflow(3)),
        (600, EventType::DmaActivate(2)),
        (650, EventType::SioTransferComplete),
        (750, EventType::DmaActivate(1)),
        (800, EventType::TimerOverflow(2)),
        (900, EventType::PpuHDrawEnd),
    ];
    assert_eq!(popped, expected);
    assert_eq!(sched.next_event_cycle, u64::MAX);
}

#[test]
fn stop_mode_via_haltcnt_and_swi_wakes_only_on_keypad_cart_serial() {
    // HALTCNT path: write 0x80 to 0x0400_0301, then increment r5 after wake and loop.
    let rom = rom_from_arm_words(&[
        0xE3A0_0301, // mov r0, #0x04000000
        0xE3A0_1080, // mov r1, #0x80
        0xE280_2C03, // add r2, r0, #0x300
        0xE5C2_1001, // strb r1, [r2, #1] -> HALTCNT = 0x80 (STOP)
        0xE285_5001, // add r5, r5, #1
        0xEAFF_FFFE, // b .
    ]);

    let mut core = GbaCore::new(None, rom);
    // Enable VBlank + Timer0 + Keypad IRQ in IE, enable VBlank IRQ + Timer0, and KEYCNT on button A.
    core.bus.write_16(
        0x0400_0200,
        (1 << 0) | (1 << 3) | (1 << 12),
        AccessType::NonSequential,
    ); // IE: VBlank | Timer0 | Keypad
    core.bus
        .write_16(0x0400_0004, 1 << 3, AccessType::NonSequential); // DISPSTAT VBlank IRQ enable
    core.bus
        .write_16(0x0400_0100, 0xFFFE, AccessType::NonSequential); // TM0CNT_L
    core.bus
        .write_16(0x0400_0102, 0x00C0, AccessType::NonSequential); // TM0CNT_H enabled + IRQ
    core.bus.write_16(
        0x0400_0132,
        (1 << 14) | 0x0001,
        AccessType::NonSequential,
    ); // KEYCNT IRQ on button A

    let mut frontend = TestFrontend::default();
    for _ in 0..4 {
        core.run_frame(&mut frontend);
    }
    assert!(core.cpu.stopped, "CPU must enter STOP state");
    assert_eq!(core.cpu.gpr[5], 0, "VBlank/Timer0 IRQs must NOT wake STOP");

    let tm0_before = core.peek16(0x0400_0100);
    let cycle_before = core.bus.scheduler.current_cycle;
    frontend.audio.clear();
    core.run_frame(&mut frontend);
    assert!(core.cpu.stopped);
    assert!(
        core.bus.scheduler.current_cycle > cycle_before,
        "scheduler current_cycle should advance over a stopped frame"
    );
    assert_eq!(
        core.peek16(0x0400_0100),
        tm0_before,
        "timers must freeze during STOP"
    );
    assert!(
        frontend.last_frame.iter().all(|&px| px == 0x7FFF),
        "LCD must output blank white (0x7FFF) during STOP"
    );
    assert!(
        !frontend.audio.is_empty() && frontend.audio.iter().all(|&s| s == 0),
        "audio must be silent during STOP"
    );

    // Press button A (active low -> bit 0 clear): raises IRQ_KEYPAD and wakes STOP.
    frontend.keypad = 0x03FE;
    core.run_frame(&mut frontend);
    assert!(!core.cpu.stopped, "keypad IRQ must wake STOP");
    assert_eq!(core.cpu.gpr[5], 1, "execution resumes after STOP");

    // Verify SWI 0x03 Stop also enters STOP and wakes on GamePak IRQ (bit 13).
    let swi_rom = rom_from_arm_words(&[
        0xEF03_0000, // swi 0x03 (Stop)
        0xE285_5001, // add r5, r5, #1
        0xEAFF_FFFE, // b .
    ]);
    let mut core_swi = GbaCore::new(None, swi_rom);
    core_swi.bus.write_16(
        0x0400_0200,
        (1 << 0) | (1 << 13),
        AccessType::NonSequential,
    ); // IE: VBlank | GamePak
    let mut fe2 = TestFrontend::default();
    core_swi.run_frame(&mut fe2);
    assert!(core_swi.cpu.stopped, "SWI 0x03 must enter STOP");
    assert_eq!(core_swi.cpu.gpr[5], 0);

    core_swi.bus.hw.request_irq(1 << 13);
    core_swi.run_frame(&mut fe2);
    assert!(!core_swi.cpu.stopped, "GamePak IRQ must wake STOP");
    assert_eq!(core_swi.cpu.gpr[5], 1);

    // Ensure normal HALT (HALTCNT = 0x00) still wakes on VBlank IRQ.
    let halt_rom = rom_from_arm_words(&[
        0xE3A0_0301, // mov r0, #0x04000000
        0xE3A0_1000, // mov r1, #0x00 (HALT)
        0xE280_2C03, // add r2, r0, #0x300
        0xE5C2_1001, // strb r1, [r2, #1] -> HALTCNT = 0x00
        0xE285_5001, // add r5, r5, #1
        0xEAFF_FFFE, // b .
    ]);
    let mut core_halt = GbaCore::new(None, halt_rom);
    core_halt
        .bus
        .write_16(0x0400_0200, 1 << 0, AccessType::NonSequential); // IE: VBlank
    core_halt
        .bus
        .write_16(0x0400_0004, 1 << 3, AccessType::NonSequential); // DISPSTAT VBlank IRQ enable
    core_halt.run_frame(&mut fe2);
    assert!(!core_halt.cpu.stopped);
    assert!(!core_halt.cpu.halted, "normal HALT must wake on VBlank IRQ");
    assert_eq!(core_halt.cpu.gpr[5], 1);
}

#[test]
fn sio_no_cable_completion_rcnt_defaults_and_link_hook() {
    let rom = rom_from_arm_words(&[0xEAFF_FFFE]);
    let mut core = GbaCore::new(None, rom);

    // Default RCNT power-on bits (0x8000) and masking behavior.
    assert_eq!(core.peek16(0x0400_0134), 0x8000);
    core.bus
        .write_16(0x0400_0134, 0xFFFF, AccessType::NonSequential);
    assert_eq!(core.peek16(0x0400_0134), 0xC1FF);
    // Enter Normal mode via RCNT bit 15 = 0.
    core.bus
        .write_16(0x0400_0134, 0x0000, AccessType::NonSequential);

    // 1) Normal 8-bit internal clock + IRQ enabled (SIOCNT bits: 0=int clk, 1=2MHz(0), 7=start, 14=IRQ).
    core.bus
        .write_8(0x0400_012A, 0x42, AccessType::NonSequential);
    core.bus.write_16(
        0x0400_0128,
        (1 << 0) | (1 << 7) | (1 << 14),
        AccessType::NonSequential,
    );
    assert_ne!(
        core.peek16(0x0400_0128) & (1 << 7),
        0,
        "start bit initially active"
    );

    let mut fe = TestFrontend::default();
    core.run_frame(&mut fe);
    assert_eq!(
        core.peek16(0x0400_0128) & (1 << 7),
        0,
        "Normal 8-bit start bit clears after scheduled duration"
    );
    assert_eq!(core.peek8(0x0400_012A), 0xFF, "Normal 8-bit no-cable reads 0xFF");
    assert_ne!(
        core.peek16(0x0400_0202) & (1 << 7),
        0,
        "SIO IRQ (bit 7) raised on completion"
    );

    // Clear IF bit 7 before next test.
    core.bus
        .write_16(0x0400_0202, 1 << 7, AccessType::NonSequential);

    // 2) Normal 32-bit internal clock + IRQ enabled (bit 12 = 32-bit).
    core.bus
        .write_32(0x0400_0120, 0x1234_5678, AccessType::NonSequential);
    core.bus.write_16(
        0x0400_0128,
        (1 << 0) | (1 << 7) | (1 << 12) | (1 << 14),
        AccessType::NonSequential,
    );
    core.run_frame(&mut fe);
    assert_eq!(core.peek16(0x0400_0128) & (1 << 7), 0);
    assert_eq!(core.peek32(0x0400_0120), 0xFFFF_FFFF);
    assert_ne!(core.peek16(0x0400_0202) & (1 << 7), 0);
    core.bus
        .write_16(0x0400_0202, 1 << 7, AccessType::NonSequential);

    // 3) Multiplayer mode no-cable: SIOMULTI0 gets SIOMLT_SEND, slots 1..3 get 0xFFFF, busy clears.
    core.bus
        .write_16(0x0400_012A, 0xABCD, AccessType::NonSequential); // SIOMLT_SEND
    core.bus.write_16(
        0x0400_0128,
        (1 << 13) | (1 << 7) | (1 << 14),
        AccessType::NonSequential,
    ); // Multi mode + Start + IRQ
    core.run_frame(&mut fe);
    assert_eq!(core.peek16(0x0400_0128) & (1 << 7), 0);
    assert_eq!(core.peek16(0x0400_0120), 0xABCD);
    assert_eq!(core.peek16(0x0400_0122), 0xFFFF);
    assert_eq!(core.peek16(0x0400_0124), 0xFFFF);
    assert_eq!(core.peek16(0x0400_0126), 0xFFFF);
    assert_ne!(core.peek16(0x0400_0202) & (1 << 7), 0);
    core.bus
        .write_16(0x0400_0202, 1 << 7, AccessType::NonSequential);

    // 4) Link-cable hook: when link_connected = true, take_pending_sio_transfer + complete_sio_transfer.
    core.set_link_connected(true);
    assert!(core.link_connected());
    core.bus
        .write_16(0x0400_012A, 0xCAFE, AccessType::NonSequential);
    core.bus.write_16(
        0x0400_0128,
        (1 << 13) | (1 << 7) | (1 << 14),
        AccessType::NonSequential,
    );
    let xfer = core
        .take_pending_sio_transfer()
        .expect("pending SIO transfer must be queued when link_connected = true");
    assert_eq!(xfer.mode, SioMode::Multi);
    assert_eq!(xfer.data8_or_send, 0xCAFE);
    assert!(core.take_pending_sio_transfer().is_none());

    // Even after advancing a frame, busy stays set until host calls complete_sio_transfer.
    core.run_frame(&mut fe);
    assert_ne!(core.peek16(0x0400_0128) & (1 << 7), 0);

    core.complete_sio_transfer(SioResponse {
        data8: 0,
        data32: 0,
        multi: [0xCAFE, 0x1111, 0x2222, 0x3333],
        multi_id: 1,
        error: false,
    });
    assert_eq!(core.peek16(0x0400_0128) & (1 << 7), 0);
    assert_eq!(core.peek16(0x0400_0120), 0xCAFE);
    assert_eq!(core.peek16(0x0400_0122), 0x1111);
    assert_eq!(core.peek16(0x0400_0124), 0x2222);
    assert_eq!(core.peek16(0x0400_0126), 0x3333);
    assert_ne!(core.peek16(0x0400_0202) & (1 << 7), 0);
}

#[test]
fn save_state_rbas_header_lz4_errors_and_no_state_mutation() {
    let rom = rom_from_arm_words(&[
        0xE3A0_102A, // mov r1, #42
        0xEAFF_FFFE, // b .
    ]);
    let mut core = GbaCore::new(None, rom.clone());
    let mut fe = TestFrontend::default();
    core.run_frame(&mut fe);
    core.poke32(0x0300_0010, 0xDEAD_BEEF);

    let state = core.save_state().expect("save_state should succeed");
    let header = state_info(&state).expect("state_info should parse RBAS header");
    assert_eq!(header.magic, STATE_MAGIC);
    assert_eq!(header.version, STATE_VERSION);
    assert_eq!(header.rom_crc32, core.rom_crc32());
    assert!(header.payload_len > 0);
    assert_eq!(GbaCore::state_info(&state), Ok(header));

    // Mutate state in core before testing error rejections.
    core.poke32(0x0300_0010, 0xCAFE_BABE);
    core.cpu.gpr[4] = 0x1234_5678;

    // 1) BadMagic
    let mut bad_magic = state.clone();
    bad_magic[0] = b'X';
    assert_eq!(state_info(&bad_magic), Err(SaveStateError::BadMagic));
    assert_eq!(core.load_state(&bad_magic), Err(SaveStateError::BadMagic));

    // 2) VersionMismatch
    let mut bad_ver = state.clone();
    bad_ver[4..6].copy_from_slice(&(STATE_VERSION + 1).to_le_bytes());
    let parsed_ver = state_info(&bad_ver).expect("state_info inspects mismatched version");
    assert_eq!(parsed_ver.version, STATE_VERSION + 1);
    assert_eq!(core.load_state(&bad_ver), Err(SaveStateError::VersionMismatch));

    // 3) RomMismatch
    let mut other_rom = rom;
    other_rom[0x100] ^= 0xFF;
    let mut other_core = GbaCore::new(None, other_rom);
    assert_eq!(
        other_core.load_state(&state),
        Err(SaveStateError::RomMismatch {
            expected: other_core.rom_crc32(),
            found: header.rom_crc32,
        })
    );

    // 4) Corrupt inputs (empty, truncated header, huge payload_len, corrupted compressed payload).
    assert_eq!(state_info(&[]), Err(SaveStateError::Corrupt));
    assert_eq!(core.load_state(&state[..10]), Err(SaveStateError::Corrupt));
    assert_eq!(core.load_state(&state[..14]), Err(SaveStateError::Corrupt));

    let mut huge_len = state.clone();
    huge_len[10..14].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    assert_eq!(core.load_state(&huge_len), Err(SaveStateError::Corrupt));

    let mut corrupt_payload = state.clone();
    let last = corrupt_payload.len() - 1;
    corrupt_payload[last] ^= 0xFF;
    corrupt_payload[last - 3] ^= 0xAA;
    assert_eq!(core.load_state(&corrupt_payload), Err(SaveStateError::Corrupt));

    // Verify none of the rejected loads mutated existing core state.
    assert_eq!(core.peek32(0x0300_0010), 0xCAFE_BABE);
    assert_eq!(core.cpu.gpr[4], 0x1234_5678);

    // Verify valid load restores previous state cleanly.
    core.load_state(&state).expect("valid state loads");
    assert_eq!(core.peek32(0x0300_0010), 0xDEAD_BEEF);
    assert_eq!(core.cpu.gpr[4], 0);
}

#[test]
fn debug_control_watchpoints_breakpoints_and_step_one_debug() {
    // 0x0800_0000: mov r0, #0x03000000
    // 0x0800_0004: mov r1, #0x5A
    // 0x0800_0008: strb r1, [r0, #0x12]   ; write watchpoint on 0x0300_0010..0x0300_0020
    // 0x0800_000C: ldrb r2, [r0, #0x12]   ; read watchpoint on 0x0300_0012
    // 0x0800_0010: nop                    ; breakpoint
    // 0x0800_0014: b .
    let rom = rom_from_arm_words(&[
        0xE3A0_0403, // mov r0, #0x03000000
        0xE3A0_105A, // mov r1, #0x5A
        0xE5C0_1012, // strb r1, [r0, #0x12]
        0xE5D0_2012, // ldrb r2, [r0, #0x12]
        0xE1A0_0000, // nop
        0xEAFF_FFFE, // b .
    ]);
    let mut core = GbaCore::new(None, rom);
    let mut fe = TestFrontend::default();

    assert!(!core.bus.watchpoint_active);

    // 1) Write watchpoint hit in run_frame_debug.
    let ctrl_write = DebugControl {
        breakpoints: Vec::new(),
        watchpoints: vec![Watchpoint::new(0x0300_0010, 0x0300_0020, WatchpointKind::Write)],
    };
    let stop1 = core.run_frame_debug(&mut fe, &ctrl_write);
    assert_eq!(
        stop1,
        StopReason::Watchpoint {
            addr: 0x0300_0012,
            pc: 0x0800_0008,
            write: true,
            value: 0x5A,
        }
    );
    assert!(!core.bus.watchpoint_active, "fast-path cleared after return");

    // 2) Single-step ReadWrite watchpoint hits on ldrb.
    let ctrl_rw = DebugControl {
        breakpoints: Vec::new(),
        watchpoints: vec![Watchpoint::addr(0x0300_0012, WatchpointKind::ReadWrite)],
    };
    let stop2 = core.step_one_debug(&ctrl_rw);
    assert_eq!(
        stop2,
        Some(StopReason::Watchpoint {
            addr: 0x0300_0012,
            pc: 0x0800_000C,
            write: false,
            value: 0x5A,
        })
    );
    assert!(!core.bus.watchpoint_active);

    // 3) Breakpoint hit via run_frame_with_breakpoints & run_frame_debug.
    let bp = core.run_frame_with_breakpoints(&mut fe, &[0x0800_0010]);
    assert_eq!(bp, Some(0x0800_0010));
}

#[test]
fn peek_poke_side_effect_free_and_memory_regions() {
    let rom = rom_with_game_code(b"TEST", &[(0, 0xEAFF_FFFE)]);
    let mut core = GbaCore::new(None, rom);

    assert_eq!(core.game_code(), "TEST");
    assert_ne!(core.rom_crc32(), 0);

    // Set IF bit 0 and verify peek16/poke16 do NOT acknowledge IF via IO side effects.
    core.bus.hw.if_flag = 0x0001;
    assert_eq!(core.peek16(0x0400_0202), 0x0001);
    assert_eq!(core.bus.hw.if_flag, 0x0001);
    core.poke16(0x0400_0202, 0xFFFF);
    assert_eq!(core.bus.hw.if_flag, 0x0001);

    // Unmapped open-bus peek must NOT mutate open_bus_latch.
    core.bus.open_bus_latch = 0x1234_5678;
    let _ = core.peek32(0x0100_0000);
    assert_eq!(core.bus.open_bus_latch, 0x1234_5678);

    // poke8/16/32 into EWRAM, IWRAM, PAL, VRAM, OAM, SRAM and verify via peek & memory_regions.
    core.poke8(0x0200_0005, 0xA5);
    core.poke16(0x0300_0010, 0xBEEF);
    core.poke32(0x0600_0020, 0x1122_3344);
    assert_eq!(core.peek8(0x0200_0005), 0xA5);
    assert_eq!(core.peek16(0x0300_0010), 0xBEEF);
    assert_eq!(core.peek32(0x0600_0020), 0x1122_3344);

    let regions = core.memory_regions();
    assert_eq!(regions.len(), 6);
    assert_eq!(regions[0].kind, MemoryRegionKind::Ewram);
    assert_eq!(regions[0].base, 0x0200_0000);
    assert_eq!(regions[0].len, 0x40000);
    assert_eq!(regions[0].data[5], 0xA5);
    assert_eq!(regions[1].kind, MemoryRegionKind::Iwram);
    assert_eq!(regions[1].len, 0x8000);
    assert_eq!(&regions[1].data[0x10..0x12], &[0xEF, 0xBE]);
    assert_eq!(regions[2].kind, MemoryRegionKind::PaletteRam);
    assert_eq!(regions[3].kind, MemoryRegionKind::Vram);
    assert_eq!(&regions[3].data[0x20..0x24], &[0x44, 0x33, 0x22, 0x11]);
    assert_eq!(regions[4].kind, MemoryRegionKind::Oam);
    assert_eq!(regions[5].kind, MemoryRegionKind::Sram);
}

#[test]
fn gamedb_lookup_precedence_fallback_and_flash_chip_ids() {
    // 1) GameDB well-known entries exist and resolve expected save types & peripherals.
    let emerald = gamedb::lookup("BPEE").expect("Pokemon Emerald in DB");
    assert_eq!(emerald.save_type, SaveType::Flash128K);
    assert!(emerald.has_rtc);

    let boktai = gamedb::lookup("U3IE").expect("Boktai in DB");
    assert_eq!(boktai.save_type, SaveType::Eeprom8K);
    assert!(boktai.has_rtc);
    assert!(boktai.solar);

    let yoshi = gamedb::lookup("KYGE").expect("Yoshi Topsy-Turvy in DB");
    assert_eq!(yoshi.save_type, SaveType::Eeprom8K);
    assert!(yoshi.tilt);

    let warioware = gamedb::lookup("RZWE").expect("WarioWare Twisted in DB");
    assert!(warioware.gyro && warioware.rumble);

    // 2) DB takes precedence even if no save string exists in ROM bytes.
    let fr_rom = rom_with_game_code(b"BPRE", &[(0, 0xEAFF_FFFE)]);
    let mut fr_core = GbaCore::new(None, fr_rom);
    assert_eq!(fr_core.bus.cart.effective_save_type(), SaveType::Flash128K);

    // Manual override still wins over GameDB.
    fr_core.bus.cart.set_save_type_override(Some(SaveType::Sram32K));
    assert_eq!(fr_core.bus.cart.effective_save_type(), SaveType::Sram32K);

    // 3) Unknown game code falls back to heuristics (SRAM_V string).
    let mut unknown_rom = rom_with_game_code(b"ZZZZ", &[(0, 0xEAFF_FFFE)]);
    unknown_rom[0x200..0x20A].copy_from_slice(b"SRAM_V110\0");
    let unknown_core = GbaCore::new(None, unknown_rom);
    assert_eq!(
        unknown_core.bus.cart.effective_save_type(),
        SaveType::Sram32K
    );

    // 4) Flash 64KB defaults to Panasonic (0x32, 0x1B) and 128KB to Sanyo (0x62, 0x13), overridable.
    let mut flash64_rom = rom_with_game_code(b"ZZZZ", &[(0, 0xEAFF_FFFE)]);
    flash64_rom[0x200..0x20B].copy_from_slice(b"FLASH_V121\0");
    let mut f64_core = GbaCore::new(None, flash64_rom);
    assert_eq!(f64_core.bus.cart.effective_save_type(), SaveType::Flash64K);

    let enter_id = |core: &mut GbaCore| {
        core.bus
            .write_8(0x0E00_5555, 0xAA, AccessType::NonSequential);
        core.bus
            .write_8(0x0E00_2AAA, 0x55, AccessType::NonSequential);
        core.bus
            .write_8(0x0E00_5555, 0x90, AccessType::NonSequential);
    };
    let exit_id = |core: &mut GbaCore| {
        core.bus
            .write_8(0x0E00_5555, 0xAA, AccessType::NonSequential);
        core.bus
            .write_8(0x0E00_2AAA, 0x55, AccessType::NonSequential);
        core.bus
            .write_8(0x0E00_5555, 0xF0, AccessType::NonSequential);
    };

    enter_id(&mut f64_core);
    assert_eq!(
        f64_core.bus.read_8(0x0E00_0000, AccessType::NonSequential, 0, false),
        0x32
    );
    assert_eq!(
        f64_core.bus.read_8(0x0E00_0001, AccessType::NonSequential, 0, false),
        0x1B
    );
    exit_id(&mut f64_core);

    f64_core
        .bus
        .cart
        .set_flash_id_override(Some(FlashChipId::SST_64K));
    enter_id(&mut f64_core);
    assert_eq!(
        f64_core.bus.read_8(0x0E00_0000, AccessType::NonSequential, 0, false),
        FlashChipId::SST_64K.manufacturer
    );
    assert_eq!(
        f64_core.bus.read_8(0x0E00_0001, AccessType::NonSequential, 0, false),
        FlashChipId::SST_64K.device
    );
    exit_id(&mut f64_core);

    // Pokemon Emerald (BPEE -> Flash128K -> Sanyo 0x62, 0x13).
    let em_rom = rom_with_game_code(b"BPEE", &[(0, 0xEAFF_FFFE)]);
    let mut em_core = GbaCore::new(None, em_rom);
    enter_id(&mut em_core);
    assert_eq!(
        em_core.bus.read_8(0x0E00_0000, AccessType::NonSequential, 0, false),
        0x62
    );
    assert_eq!(
        em_core.bus.read_8(0x0E00_0001, AccessType::NonSequential, 0, false),
        0x13
    );
    exit_id(&mut em_core);
}

#[test]
fn idle_loop_skip_safe_polling_vs_countdown_and_gamedb_addr() {
    // Wait-for-VCOUNT == 160 polling loop at 0x0800_0004..0x0800_000C:
    // 0x00: mov r0, #0x04000000
    // 0x04: ldrh r1, [r0, #6]     (0xE1D0_10B6)
    // 0x08: cmp r1, #160          (0xE351_00A0)
    // 0x0C: bne 0x04              (0x1AFF_FFFC)
    // 0x10: add r5, r5, #1
    // 0x14: b .
    let poll_rom = rom_from_arm_words(&[
        0xE3A0_0404, // mov r0, #0x04000000
        0xE1D0_10B6, // ldrh r1, [r0, #6]
        0xE351_00A0, // cmp r1, #160
        0x1AFF_FFFC, // bne 0x0800_0004
        0xE285_5001, // add r5, r5, #1
        0xEAFF_FFFE, // b .
    ]);

    let mut core_skip = GbaCore::new(None, poll_rom);
    assert!(core_skip.idle_skip_enabled);
    let mut fe = TestFrontend::default();
    core_skip.run_frame(&mut fe);
    assert_eq!(core_skip.cpu.gpr[5], 1, "loop must exit cleanly at VCOUNT 160");

    // Countdown loop (has register mutation without safe poll load) must NEVER be skipped:
    // 0x00: mov r1, #500
    // 0x04: subs r1, r1, #1
    // 0x08: bne 0x04
    // 0x0C: mov r5, #99
    // 0x10: b .
    let countdown_rom = rom_from_arm_words(&[
        0xE3A0_1F7D, // mov r1, #500 (0x7D << 2 = 500)
        0xE251_1001, // subs r1, r1, #1
        0x1AFF_FFFD, // bne 0x0800_0004
        0xE3A0_5063, // mov r5, #99
        0xEAFF_FFFE, // b .
    ]);
    let mut count_core = GbaCore::new(None, countdown_rom);
    count_core.run_frame(&mut fe);
    assert_eq!(
        count_core.cpu.gpr[1], 0,
        "countdown must finish all 500 iterations"
    );
    assert_eq!(count_core.cpu.gpr[5], 99);

    // Loop with side effect (add r4, r4, #1; b 0x0800_0000):
    // - Without idle_loop_addr, heuristic refuses to skip it -> r4 runs thousands of iterations.
    // - With idle_loop_addr = Some(0x0800_0000), explicit address triggers idle skip -> far fewer iterations.
    let counter_loop_rom = rom_from_arm_words(&[
        0xE284_4001, // add r4, r4, #1  at 0x0800_0000
        0xEAFF_FFFD, // b 0x0800_0000  (0x04 + 8 + (-3*4) = 0x00)
    ]);
    let mut no_db_core = GbaCore::new(None, counter_loop_rom.clone());
    no_db_core.run_frame(&mut fe);
    let iterations_without_db = no_db_core.cpu.gpr[4];
    assert!(
        iterations_without_db > 10_000,
        "unsafe side-effecting loop must NOT be heuristically skipped (got {iterations_without_db})"
    );

    let mut db_core = GbaCore::new(None, counter_loop_rom);
    db_core.bus.cart.idle_loop_addr = Some(0x0800_0000);
    db_core.run_frame(&mut fe);
    let iterations_with_db = db_core.cpu.gpr[4];
    assert!(
        iterations_with_db < 1_000,
        "explicit gamedb idle_loop_addr should skip idle cycles (got {iterations_with_db})"
    );
}
