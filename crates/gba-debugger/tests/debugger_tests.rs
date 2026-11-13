use gba_core::{DebugControl, GbaCore, StopReason, Watchpoint, WatchpointKind};
use gba_debugger::{
    CallFrame, CallStackTracker, GdbAction, GdbSession, MAX_CALL_STACK_DEPTH, RspEvent, RspParser,
    decode_io_reg, disassemble_range, encode_rsp_packet, escape_rsp_bytes, format_registers,
    is_call_instruction, is_return_instruction, known_io_regs, rsp_checksum, step_out_return_addr,
    unescape_rsp_bytes,
};

fn core_with_dummy_rom() -> GbaCore {
    GbaCore::new(None, vec![0u8; 0x200])
}

#[test]
fn disassembles_arm_and_thumb_instructions_and_formats_registers() {
    let mut core = core_with_dummy_rom();

    // ARM instructions at 0x0200_0000:
    //   0x0200_0000: MOV R0, #0x42      (0xE3A00042)
    //   0x0200_0004: BL +0x10           (0xEB000002 -> target 0x0200_0014)
    //   0x0200_0008: BX LR              (0xE12FFF1E)
    core.poke32(0x0200_0000, 0xE3A0_0042);
    core.poke32(0x0200_0004, 0xEB00_0002);
    core.poke32(0x0200_0008, 0xE12F_FF1E);

    let arm_lines = disassemble_range(&core, 0x0200_0000, 3, false);
    assert_eq!(arm_lines.len(), 3);
    assert_eq!(arm_lines[0].addr, 0x0200_0000);
    assert!(arm_lines[0].text.to_lowercase().contains("mov"));
    assert!(arm_lines[1].text.to_lowercase().contains("bl"));
    assert!(arm_lines[2].text.to_lowercase().contains("bx"));

    // Thumb instructions at 0x0300_0000:
    //   0x0300_0000: MOV R0, #1         (0x2001)
    //   0x0300_0002: BL first half      (0xF000)
    //   0x0300_0004: BL second half     (0xF802)
    //   0x0300_0006: BX LR              (0x4770)
    core.poke16(0x0300_0000, 0x2001);
    core.poke16(0x0300_0002, 0xF000);
    core.poke16(0x0300_0004, 0xF802);
    core.poke16(0x0300_0006, 0x4770);

    let thumb_lines = disassemble_range(&core, 0x0300_0000, 3, true);
    assert_eq!(thumb_lines.len(), 3);
    assert!(thumb_lines[0].text.to_lowercase().contains("mov"));
    assert!(thumb_lines[1].text.to_lowercase().contains("bl"));
    let bx_line = thumb_lines
        .iter()
        .find(|l| l.addr == 0x0300_0006)
        .expect("BX LR line after 4-byte Thumb BL");
    assert!(bx_line.text.to_lowercase().contains("bx"));

    core.cpu.gpr[0] = 0x1234_5678;
    core.cpu.flush_pipeline(&mut core.bus, 0x0200_0000);
    let reg_dump = format_registers(&core);
    assert!(reg_dump.contains("12345678"), "reg_dump: {reg_dump}");
    assert!(
        reg_dump.to_uppercase().contains("R15") || reg_dump.to_uppercase().contains("PC"),
        "reg_dump: {reg_dump}"
    );
    assert!(reg_dump.contains("0200000"), "reg_dump: {reg_dump}");
}

#[test]
fn detects_arm_and_thumb_call_and_return_instructions() {
    let mut core = core_with_dummy_rom();

    // ARM BL -> returns pc + 4
    core.poke32(0x0200_0000, 0xEB00_0004);
    assert_eq!(
        is_call_instruction(&core, 0x0200_0000, false),
        Some(0x0200_0004)
    );

    // ARM MOV LR, PC followed by BX R0 -> returns pc + 8
    core.poke32(0x0200_0010, 0xE1A0_E00F); // MOV LR, PC
    core.poke32(0x0200_0014, 0xE12F_FF10); // BX R0
    assert_eq!(
        is_call_instruction(&core, 0x0200_0010, false),
        Some(0x0200_0018)
    );

    // Sitting directly on BX R0 with LR == pc + 4 also counts as linked call
    core.cpu.gpr[14] = 0x0200_0018;
    assert_eq!(
        is_call_instruction(&core, 0x0200_0014, false),
        Some(0x0200_0018)
    );

    // ARM MOV LR, PC followed by unconditional B -> returns pc + 8
    core.poke32(0x0200_0020, 0xE1A0_E00F); // MOV LR, PC
    core.poke32(0x0200_0024, 0xEA00_0010); // B +...
    assert_eq!(
        is_call_instruction(&core, 0x0200_0020, false),
        Some(0x0200_0028)
    );

    // Plain ADD is not a call; cond == 0xF (NV) is not a call
    core.poke32(0x0200_0030, 0xE280_0001); // ADD R0, R0, #1
    core.poke32(0x0200_0034, 0xFB00_0000); // cond=0xF
    assert_eq!(is_call_instruction(&core, 0x0200_0030, false), None);
    assert_eq!(is_call_instruction(&core, 0x0200_0034, false), None);

    // Thumb 2-halfword BL -> returns pc + 4
    core.poke16(0x0300_0000, 0xF000);
    core.poke16(0x0300_0002, 0xF808);
    assert_eq!(
        is_call_instruction(&core, 0x0300_0000, true),
        Some(0x0300_0004)
    );

    // Broken Thumb 0xF000 not followed by 0xF800..0xFFFF -> None
    core.poke16(0x0300_0010, 0xF000);
    core.poke16(0x0300_0012, 0x2001);
    assert_eq!(is_call_instruction(&core, 0x0300_0010, true), None);

    // Thumb MOV LR, PC (0x46FE) + BX R1 (0x4708) -> returns pc + 4
    core.poke16(0x0300_0020, 0x46FE);
    core.poke16(0x0300_0022, 0x4708);
    assert_eq!(
        is_call_instruction(&core, 0x0300_0020, true),
        Some(0x0300_0024)
    );

    // Return instruction detection (ARM + Thumb + conditional flags)
    core.poke32(0x0200_0100, 0xE12F_FF1E); // BX LR
    core.poke32(0x0200_0104, 0xE8BD_8010); // LDMFD SP!, {R4, PC}
    core.poke32(0x0200_0108, 0xE25E_F004); // SUBS PC, LR, #4
    core.poke32(0x0200_010C, 0x012F_FF1E); // BXEQ LR
    core.poke32(0x0200_0110, 0xE49D_F004); // LDR PC, [SP], #4
    core.poke32(0x0200_0114, 0xE51F_F004); // LDR PC, [PC, #-4] (jump veneer, NOT return!)
    assert!(is_return_instruction(
        &core,
        0x0200_0100,
        false,
        0x0000_001F
    ));
    assert!(is_return_instruction(
        &core,
        0x0200_0104,
        false,
        0x0000_001F
    ));
    assert!(is_return_instruction(
        &core,
        0x0200_0108,
        false,
        0x0000_001F
    ));
    assert!(!is_return_instruction(
        &core,
        0x0200_010C,
        false,
        0x0000_001F
    )); // Z=0
    assert!(is_return_instruction(
        &core,
        0x0200_010C,
        false,
        0x4000_001F
    )); // Z=1
    assert!(is_return_instruction(
        &core,
        0x0200_0110,
        false,
        0x0000_001F
    ));
    assert!(!is_return_instruction(
        &core,
        0x0200_0114,
        false,
        0x0000_001F
    ));

    core.poke16(0x0300_0100, 0x4770); // Thumb BX LR
    core.poke16(0x0300_0102, 0xBD10); // Thumb POP {R4, PC}
    core.poke16(0x0300_0104, 0x46F7); // Thumb MOV PC, LR
    assert!(is_return_instruction(&core, 0x0300_0100, true, 0x0000_003F));
    assert!(is_return_instruction(&core, 0x0300_0102, true, 0x0000_003F));
    assert!(is_return_instruction(&core, 0x0300_0104, true, 0x0000_003F));
}

#[test]
fn call_stack_tracker_tracks_arm_and_thumb_calls_and_enforces_max_depth() {
    let mut core = core_with_dummy_rom();
    let ctrl = DebugControl::default();
    let mut tracker = CallStackTracker::new();

    // ARM program in EWRAM:
    //   0x0200_0000: BL 0x0200_0020   (offset = (0x20 - 0x08)/4 = 6 -> 0xEB000006)
    //   0x0200_0004: MOV R1, #7       (0xE3A01007)
    // Subroutine at 0x0200_0020:
    //   0x0200_0020: MOV R0, #9       (0xE3A00009)
    //   0x0200_0024: BX LR            (0xE12FFF1E)
    core.poke32(0x0200_0000, 0xEB00_0006);
    core.poke32(0x0200_0004, 0xE3A0_1007);
    core.poke32(0x0200_0020, 0xE3A0_0009);
    core.poke32(0x0200_0024, 0xE12F_FF1E);

    core.cpu.set_cpsr(0x0000_001F); // ARM SYS mode, IRQ disabled
    core.cpu.flush_pipeline(&mut core.bus, 0x0200_0000);

    // Step 1: BL -> enters 0x0200_0020, pushes frame with return_addr = 0x0200_0004
    tracker.step_one_with_tracker(&mut core, &ctrl);
    assert_eq!(core.cpu.current_exec_pc(), 0x0200_0020);
    assert_eq!(tracker.frames().len(), 1);
    assert_eq!(tracker.frames()[0].call_site_pc, 0x0200_0000);
    assert_eq!(tracker.frames()[0].entry_addr, 0x0200_0020);
    assert_eq!(tracker.frames()[0].return_addr, 0x0200_0004);
    assert_eq!(step_out_return_addr(&core, &tracker), Some(0x0200_0004));

    // Step 2: MOV R0, #9 -> stays inside callee
    tracker.step_one_with_tracker(&mut core, &ctrl);
    assert_eq!(tracker.frames().len(), 1);
    assert_eq!(core.cpu.gpr[0], 9);

    // Step 3: BX LR -> pops frame back to 0x0200_0004
    tracker.step_one_with_tracker(&mut core, &ctrl);
    assert_eq!(core.cpu.current_exec_pc(), 0x0200_0004);
    assert!(tracker.frames().is_empty());

    // Thumb BL sequence at 0x0300_0000 -> 0x0300_0010 (offset +16 = +8 halfwords after pc+4=0x0300_0004 -> off = 0x06):
    //   0x0300_0000: 0xF000 (BL H=0)
    //   0x0300_0002: 0xF806 (BL H=1, target = 0x0300_0004 + 12 = 0x0300_0010)
    //   0x0300_0004: 0x2001 (MOV R0, #1)
    //   0x0300_0010: 0x2205 (MOV R2, #5)
    //   0x0300_0012: 0x4770 (BX LR)
    core.poke16(0x0300_0000, 0xF000);
    core.poke16(0x0300_0002, 0xF806);
    core.poke16(0x0300_0004, 0x2001);
    core.poke16(0x0300_0010, 0x2205);
    core.poke16(0x0300_0012, 0x4770);

    core.cpu.set_cpsr(0x0000_003F); // Thumb SYS mode
    core.cpu.flush_pipeline(&mut core.bus, 0x0300_0000);

    // 1st halfword does not push yet
    tracker.step_one_with_tracker(&mut core, &ctrl);
    assert_eq!(core.cpu.current_exec_pc(), 0x0300_0002);
    assert!(tracker.frames().is_empty());

    // 2nd halfword jumps and pushes callee frame
    tracker.step_one_with_tracker(&mut core, &ctrl);
    assert_eq!(core.cpu.current_exec_pc(), 0x0300_0010);
    assert_eq!(tracker.frames().len(), 1);
    assert_eq!(tracker.frames()[0].call_site_pc, 0x0300_0000);
    assert_eq!(tracker.frames()[0].return_addr, 0x0300_0004);
    assert!(tracker.frames()[0].thumb);

    tracker.step_one_with_tracker(&mut core, &ctrl); // MOV R2, #5
    tracker.step_one_with_tracker(&mut core, &ctrl); // BX LR
    assert_eq!(core.cpu.current_exec_pc(), 0x0300_0004);
    assert!(tracker.frames().is_empty());

    // Untaken conditional ARM BLNE (Z=1) should not push
    core.poke32(0x0200_0100, 0x1B00_0004); // BLNE
    core.cpu.set_cpsr(0x4000_001F); // Z=1, ARM mode
    core.cpu.flush_pipeline(&mut core.bus, 0x0200_0100);
    tracker.step_one_with_tracker(&mut core, &ctrl);
    assert_eq!(core.cpu.current_exec_pc(), 0x0200_0104);
    assert!(tracker.frames().is_empty());

    // Cap at MAX_CALL_STACK_DEPTH (64), keeping innermost frames
    for i in 0..(MAX_CALL_STACK_DEPTH as u32 + 6) {
        tracker.push_frame(CallFrame {
            call_site_pc: 0x0800_0000 + i * 4,
            entry_addr: 0x0801_0000 + i * 4,
            return_addr: 0x0800_0004 + i * 4,
            stack_pointer: 0x0300_7F00 - i * 4,
            thumb: false,
            is_irq: false,
        });
    }
    assert_eq!(tracker.frames().len(), MAX_CALL_STACK_DEPTH);
    assert_eq!(tracker.frames()[0].call_site_pc, 0x0800_0000 + 6 * 4);
    assert_eq!(
        step_out_return_addr(&core, &tracker),
        Some(0x0800_0004 + (MAX_CALL_STACK_DEPTH as u32 + 5) * 4)
    );
}

#[test]
fn decodes_gba_mmio_registers_and_bitfields() {
    for &(addr, expected_name, _) in known_io_regs() {
        let desc = decode_io_reg(addr, 0).expect("known IO reg should decode");
        assert_eq!(desc.name, expected_name);
    }

    let dispcnt = decode_io_reg(0x0400_0000, 0x1F43).unwrap();
    assert_eq!(dispcnt.name, "DISPCNT");
    assert!(dispcnt.summary.contains("Mode 3"));
    assert!(dispcnt.summary.contains("BG2"));
    assert!(dispcnt.summary.contains("OBJ"));
    assert!(
        dispcnt
            .fields
            .iter()
            .any(|f| f.name == "BG Mode" && f.value == 3)
    );

    // Active-low keypad: bit 0 (A) = 0, bit 3 (Start) = 0 => A and Start pressed
    let keyinput = decode_io_reg(0x0400_0130, 0x03FF ^ ((1 << 0) | (1 << 3))).unwrap();
    assert_eq!(keyinput.name, "KEYINPUT");
    assert!(keyinput.summary.contains("A"));
    assert!(keyinput.summary.contains("Start"));
    assert!(!keyinput.summary.contains("Select"));

    let dma = decode_io_reg(0x0400_00BA, 0x8400).unwrap();
    assert_eq!(dma.name, "DMA0CNT_H");
    assert!(dma.summary.contains("ENABLED"));
    assert!(dma.summary.contains("32b"));

    let ie = decode_io_reg(0x0400_0200, (1 << 0) | (1 << 12)).unwrap();
    assert!(ie.summary.contains("VBlank"));
    assert!(ie.summary.contains("Keypad"));

    let halt = decode_io_reg(0x0400_0301, 0x80).unwrap();
    assert_eq!(halt.name, "HALTCNT");
    assert!(halt.summary.contains("Stop"));

    assert!(decode_io_reg(0x0200_0000, 0).is_none());
    assert!(decode_io_reg(0x0400_0058, 0).is_none());
}

#[test]
fn rsp_framing_parser_and_protocol_commands_work() {
    assert_eq!(rsp_checksum(b"OK"), 0x9A);
    assert_eq!(encode_rsp_packet("OK"), "$OK#9a");

    let special = b"abc$#}*xyz";
    let esc = escape_rsp_bytes(special);
    assert!(!esc.contains(&b'$'));
    assert!(!esc.contains(&b'#'));
    assert_eq!(unescape_rsp_bytes(&esc), special);

    // Stream parser handles split packets, ack/nack, interrupt, and bad checksum
    let mut parser = RspParser::new();
    let mut events = Vec::new();
    parser.feed(b"+\x03$qSup", &mut events);
    parser.feed(b"ported#37$bad#00-", &mut events);
    assert_eq!(
        events,
        vec![
            RspEvent::Ack,
            RspEvent::Interrupt,
            RspEvent::Packet(String::from("qSupported")),
            RspEvent::BadChecksum,
            RspEvent::Nack,
        ]
    );

    // GdbSession register, memory, target.xml, breakpoints, and watchpoints
    let mut core = core_with_dummy_rom();
    core.cpu.set_cpsr(0x0000_001F);
    core.cpu.gpr[0] = 0xCAFE_BABE;
    core.cpu.flush_pipeline(&mut core.bus, 0x0200_0100);

    let mut session = GdbSession::new();
    assert_eq!(
        session.handle_packet(&mut core, "?").reply.as_deref(),
        Some("S05")
    );

    let sup = session.handle_packet(&mut core, "qSupported:multiprocess+");
    assert!(sup.reply.unwrap().contains("qXfer:features:read+"));

    // Legacy 26-reg FPA layout before target.xml (336 hex chars)
    let legacy_g = session.handle_packet(&mut core, "g").reply.unwrap();
    assert_eq!(legacy_g.len(), 336);
    assert!(legacy_g.starts_with("bebafeca")); // r0 LE

    // Read target.xml -> switches g layout to 17 regs (136 hex chars)
    let xml = session
        .handle_packet(&mut core, "qXfer:features:read:target.xml:0,4000")
        .reply
        .unwrap();
    assert!(xml.starts_with('l'));
    assert!(xml.contains("<architecture>arm</architecture>"));

    let xml_g = session.handle_packet(&mut core, "g").reply.unwrap();
    assert_eq!(xml_g.len(), 136);
    // R15 is at index 15 -> chars 120..128 must match exec PC 0x0200_0100 (LE: 00010002), NOT prefetched 0x0200_0108
    assert_eq!(&xml_g[120..128], "00010002");

    // Single register P/p for CPSR (regnum 25 = 0x19) and PC (regnum 15 = 0x0f)
    assert_eq!(
        session.handle_packet(&mut core, "pf").reply.as_deref(),
        Some("00010002")
    );
    assert_eq!(
        session
            .handle_packet(&mut core, "P19=3f000000")
            .reply
            .as_deref(),
        Some("OK")
    );
    assert_eq!(core.cpu.cpsr, 0x0000_003F);

    // Memory M/m
    assert_eq!(
        session
            .handle_packet(&mut core, "M02000200,4:78563412")
            .reply
            .as_deref(),
        Some("OK")
    );
    assert_eq!(core.peek32(0x0200_0200), 0x1234_5678);
    assert_eq!(
        session
            .handle_packet(&mut core, "m02000200,4")
            .reply
            .as_deref(),
        Some("78563412")
    );

    // Breakpoints and watchpoints merge cleanly into DebugControl
    assert_eq!(
        session
            .handle_packet(&mut core, "Z0,08001234,4")
            .reply
            .as_deref(),
        Some("OK")
    );
    assert_eq!(
        session
            .handle_packet(&mut core, "Z2,02000400,4")
            .reply
            .as_deref(),
        Some("OK")
    );
    assert_eq!(
        session
            .handle_packet(&mut core, "Z3,02000500,2")
            .reply
            .as_deref(),
        Some("OK")
    );
    assert_eq!(
        session
            .handle_packet(&mut core, "Z4,02000600,1")
            .reply
            .as_deref(),
        Some("OK")
    );

    let mut ctrl = DebugControl::default();
    session.merge_debug_control(&DebugControl::default(), &mut ctrl);
    assert!(ctrl.breakpoints.contains(&0x0800_1234));
    assert!(ctrl.watchpoints.contains(&Watchpoint::new(
        0x0200_0400,
        0x0200_0403,
        WatchpointKind::Write
    )));
    assert!(ctrl.watchpoints.contains(&Watchpoint::new(
        0x0200_0500,
        0x0200_0501,
        WatchpointKind::Read
    )));
    assert!(ctrl.watchpoints.contains(&Watchpoint::new(
        0x0200_0600,
        0x0200_0600,
        WatchpointKind::ReadWrite
    )));

    assert_eq!(
        session.format_stop_reply(&StopReason::Watchpoint {
            addr: 0x0200_0402,
            pc: 0x0800_0100,
            write: true,
            value: 1,
        }),
        "T05watch:02000402;"
    );
    assert_eq!(
        session.format_stop_reply(&StopReason::Watchpoint {
            addr: 0x0200_0500,
            pc: 0x0800_0104,
            write: false,
            value: 2,
        }),
        "T05rwatch:02000500;"
    );
    assert_eq!(session.interrupt_stop_reply(), "S02");

    // Remove points and detach
    assert_eq!(
        session
            .handle_packet(&mut core, "z0,08001234,4")
            .reply
            .as_deref(),
        Some("OK")
    );
    assert_eq!(
        session
            .handle_packet(&mut core, "z2,02000400,4")
            .reply
            .as_deref(),
        Some("OK")
    );
    let cont = session.handle_packet(&mut core, "vCont;c:01");
    assert_eq!(cont.action, GdbAction::Continue);
    assert!(session.is_running());

    let det = session.handle_packet(&mut core, "D");
    assert_eq!(det.action, GdbAction::Detach);
    assert!(!session.is_running());
    assert_eq!(session.breakpoint_counts(), (0, 0));
}

#[cfg(feature = "std")]
#[test]
fn tcp_gdb_stub_handles_handshake_watchpoint_continue_and_stop_reply() {
    use gba_debugger::GdbStub;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::{Duration, Instant};

    let mut core = core_with_dummy_rom();
    // ARM program at 0x0200_0000:
    //   0x0200_0000: MOV R0, #0x02000000 (0xE3A00402)
    //   0x0200_0004: ADD R0, R0, #0x400  (0xE2800B01) -> R0 = 0x0200_0400
    //   0x0200_0008: MOV R1, #0x55       (0xE3A01055)
    //   0x0200_000C: STR R1, [R0]        (0xE5801000) -> triggers write watchpoint!
    //   0x0200_0010: B .                 (0xEAFFFFFE)
    core.poke32(0x0200_0000, 0xE3A0_0402);
    core.poke32(0x0200_0004, 0xE280_0B01);
    core.poke32(0x0200_0008, 0xE3A0_1055);
    core.poke32(0x0200_000C, 0xE580_1000);
    core.poke32(0x0200_0010, 0xEAFF_FFFE);
    core.cpu.set_cpsr(0x0000_001F);
    core.cpu.flush_pipeline(&mut core.bus, 0x0200_0000);

    let mut stub = GdbStub::new();
    let port = stub.start(0).expect("bind localhost ephemeral port");

    let mut client = TcpStream::connect(("127.0.0.1", port)).expect("connect to stub");
    client.set_nonblocking(true).unwrap();

    let send_recv =
        |stub: &mut GdbStub, core: &mut GbaCore, client: &mut TcpStream, pkt: &str| -> String {
            let framed = encode_rsp_packet(pkt);
            client.write_all(framed.as_bytes()).unwrap();
            let mut collected = String::new();
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut tmp = [0u8; 1024];
            while Instant::now() < deadline {
                stub.step_protocol(core);
                match client.read(&mut tmp) {
                    Ok(n) if n > 0 => {
                        collected.push_str(&String::from_utf8_lossy(&tmp[..n]));
                        if collected.contains('#') {
                            // Drain trailing checksum 2 hex digits if already buffered
                            if let Some(hash_idx) = collected.rfind('#')
                                && collected.len() >= hash_idx + 3
                            {
                                return collected;
                            }
                        }
                    }
                    _ => std::thread::sleep(Duration::from_millis(2)),
                }
            }
            collected
        };

    let sup = send_recv(&mut stub, &mut core, &mut client, "qSupported");
    assert!(sup.contains("qXfer:features:read+"), "reply was: {sup}");

    let wp = send_recv(&mut stub, &mut core, &mut client, "Z2,02000400,4");
    assert!(wp.contains("$OK#9a"), "reply was: {wp}");

    let c_framed = encode_rsp_packet("c");
    client.write_all(c_framed.as_bytes()).unwrap();

    let mut stop_buf = String::new();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut tmp = [0u8; 512];
    while Instant::now() < deadline {
        stub.step_protocol(&mut core);
        match client.read(&mut tmp) {
            Ok(n) if n > 0 => {
                stop_buf.push_str(&String::from_utf8_lossy(&tmp[..n]));
                if stop_buf.contains("T05watch:02000400;") {
                    break;
                }
            }
            _ => std::thread::sleep(Duration::from_millis(1)),
        }
    }

    assert!(
        stop_buf.contains("T05watch:02000400;"),
        "expected watchpoint stop reply, got: {stop_buf}"
    );
    assert_eq!(core.peek32(0x0200_0400), 0x55);
}
