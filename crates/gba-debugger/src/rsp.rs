use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use gba_core::{DebugControl, GbaCore, StopReason, Watchpoint, WatchpointKind};

/// Target description XML advertised via `qXfer:features:read:target.xml`.
pub const ARM7TDMI_TARGET_XML: &str = r#"<?xml version="1.0"?>
<!DOCTYPE target SYSTEM "gdb-target.dtd">
<target version="1.0">
  <architecture>arm</architecture>
  <feature name="org.gnu.gdb.arm.core">
    <reg name="r0" bitsize="32" type="uint32"/>
    <reg name="r1" bitsize="32" type="uint32"/>
    <reg name="r2" bitsize="32" type="uint32"/>
    <reg name="r3" bitsize="32" type="uint32"/>
    <reg name="r4" bitsize="32" type="uint32"/>
    <reg name="r5" bitsize="32" type="uint32"/>
    <reg name="r6" bitsize="32" type="uint32"/>
    <reg name="r7" bitsize="32" type="uint32"/>
    <reg name="r8" bitsize="32" type="uint32"/>
    <reg name="r9" bitsize="32" type="uint32"/>
    <reg name="r10" bitsize="32" type="uint32"/>
    <reg name="r11" bitsize="32" type="uint32"/>
    <reg name="r12" bitsize="32" type="uint32"/>
    <reg name="sp" bitsize="32" type="data_ptr"/>
    <reg name="lr" bitsize="32" type="code_ptr"/>
    <reg name="pc" bitsize="32" type="code_ptr"/>
    <reg name="cpsr" bitsize="32" regnum="25"/>
  </feature>
</target>
"#;

/// Computes the 8-bit modulo-256 GDB RSP checksum over `payload`.
pub fn rsp_checksum(payload: &[u8]) -> u8 {
    payload.iter().fold(0u8, |acc, &b| acc.wrapping_add(b))
}

/// Frames `payload` as a complete `$payload#cc` GDB RSP packet.
pub fn encode_rsp_packet(payload: &str) -> String {
    let sum = rsp_checksum(payload.as_bytes());
    format!("${payload}#{sum:02x}")
}

/// Escapes special RSP bytes (`#`, `$`, `}`, `*`) using `}` XOR `0x20`.
pub fn escape_rsp_bytes(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for &b in data {
        match b {
            b'#' | b'$' | b'}' | b'*' => {
                out.push(b'}');
                out.push(b ^ 0x20);
            }
            _ => out.push(b),
        }
    }
    out
}

/// Unescapes RSP `}` XOR `0x20` sequences in-place into a new `Vec<u8>`.
pub fn unescape_rsp_bytes(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'}' && i + 1 < data.len() {
            out.push(data[i + 1] ^ 0x20);
            i += 2;
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    out
}

/// Event decoded from the incoming RSP byte stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RspEvent {
    /// Positive acknowledgement (`+`).
    Ack,
    /// Negative acknowledgement (`-`), requesting retransmission.
    Nack,
    /// Ctrl+C interrupt byte (`0x03`).
    Interrupt,
    /// Valid checksum-verified packet payload.
    Packet(String),
    /// Checksum mismatch on a received packet.
    BadChecksum,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum ParseState {
    #[default]
    Idle,
    Payload,
    Checksum1,
    Checksum2(u8),
}

/// Incremental byte-stream parser for incoming GDB RSP frames.
#[derive(Debug, Default)]
pub struct RspParser {
    state: ParseState,
    buf: Vec<u8>,
}

impl RspParser {
    /// Creates a new stream parser in the idle state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Resets any partially buffered packet.
    pub fn reset(&mut self) {
        self.state = ParseState::Idle;
        self.buf.clear();
    }

    /// Feeds raw bytes from the client and appends decoded [`RspEvent`] items to `events`.
    pub fn feed(&mut self, bytes: &[u8], events: &mut Vec<RspEvent>) {
        for &b in bytes {
            match self.state {
                ParseState::Idle => match b {
                    b'+' => events.push(RspEvent::Ack),
                    b'-' => events.push(RspEvent::Nack),
                    0x03 => events.push(RspEvent::Interrupt),
                    b'$' => {
                        self.buf.clear();
                        self.state = ParseState::Payload;
                    }
                    _ => {}
                },
                ParseState::Payload => {
                    if b == b'$' {
                        // Resynchronize on unexpected start marker.
                        self.buf.clear();
                    } else if b == b'#' {
                        self.state = ParseState::Checksum1;
                    } else {
                        self.buf.push(b);
                    }
                }
                ParseState::Checksum1 => {
                    if let Some(hi) = hex_nibble(b) {
                        self.state = ParseState::Checksum2(hi << 4);
                    } else {
                        self.state = ParseState::Idle;
                        self.buf.clear();
                        events.push(RspEvent::BadChecksum);
                    }
                }
                ParseState::Checksum2(hi) => {
                    self.state = ParseState::Idle;
                    let valid = hex_nibble(b)
                        .map(|lo| (hi | lo) == rsp_checksum(&self.buf))
                        .unwrap_or(false);
                    if valid {
                        let payload = String::from_utf8_lossy(&self.buf).into_owned();
                        events.push(RspEvent::Packet(payload));
                    } else {
                        events.push(RspEvent::BadChecksum);
                    }
                    self.buf.clear();
                }
            }
        }
    }
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn encode_u32_le_hex(val: u32, out: &mut String) {
    for byte in val.to_le_bytes() {
        out.push_str(&format!("{byte:02x}"));
    }
}

fn decode_u32_le_hex(s: &str) -> Option<u32> {
    if s.len() != 8 {
        return None;
    }
    let bytes = decode_hex_bytes(s)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn decode_hex_bytes(s: &str) -> Option<Vec<u8>> {
    if (s.len() & 1) != 0 {
        return None;
    }
    let raw = s.as_bytes();
    let mut out = Vec::with_capacity(raw.len() / 2);
    for &[hi_b, lo_b] in raw.as_chunks::<2>().0 {
        let hi = hex_nibble(hi_b)?;
        let lo = hex_nibble(lo_b)?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

fn parse_hex_u32(s: &str) -> Option<u32> {
    u32::from_str_radix(s.trim(), 16).ok()
}

fn parse_hex_usize(s: &str) -> Option<usize> {
    usize::from_str_radix(s.trim(), 16).ok()
}

/// Execution-control request produced by handling a GDB packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GdbAction {
    /// No execution state transition requested; packet already answered.
    None,
    /// Resume continuous target execution (`c` / `vCont;c`).
    Continue,
    /// Execute a single instruction (`s` / `vCont;s`) and report stop reply.
    Step,
    /// Pause target immediately (`0x03` Ctrl+C interrupt).
    Break,
    /// Detach debugger (`D`).
    Detach,
    /// Terminate debug session / kill target (`k`).
    Kill,
}

/// Result of handling a single decoded RSP packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketOutcome {
    /// Optional raw payload (unframed; caller frames with `$...#cc`) to reply immediately.
    pub reply: Option<String>,
    /// Execution-control transition requested by the command.
    pub action: GdbAction,
    /// Whether no-ack mode (`QStartNoAckMode`) became active after sending `reply`.
    pub enable_no_ack: bool,
}

/// Stateful `#![no_std]` GDB Remote Serial Protocol session for an ARM7TDMI GBA target.
#[derive(Debug)]
pub struct GdbSession {
    /// Whether `QStartNoAckMode` has been negotiated.
    pub no_ack_mode: bool,
    /// Whether `qXfer:features:read:target.xml` has been queried by this client.
    pub target_xml_read: bool,
    sw_breakpoints: Vec<u32>,
    hw_breakpoints: Vec<u32>,
    watchpoints: Vec<Watchpoint>,
    last_stop_reply: String,
    run_state: RunState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunState {
    Halted,
    Running,
}

impl Default for GdbSession {
    fn default() -> Self {
        Self::new()
    }
}

impl GdbSession {
    /// Creates a fresh GDB protocol session starting in the halted state.
    pub fn new() -> Self {
        Self {
            no_ack_mode: false,
            target_xml_read: false,
            sw_breakpoints: Vec::new(),
            hw_breakpoints: Vec::new(),
            watchpoints: Vec::new(),
            last_stop_reply: String::from("S05"),
            run_state: RunState::Halted,
        }
    }

    /// Resets session state for a newly accepted TCP connection.
    pub fn reset_connection(&mut self) {
        self.no_ack_mode = false;
        self.target_xml_read = false;
        self.sw_breakpoints.clear();
        self.hw_breakpoints.clear();
        self.watchpoints.clear();
        self.last_stop_reply = String::from("S05");
        self.run_state = RunState::Halted;
    }

    /// Returns `true` if the GDB target is currently expected to run continuously.
    pub fn is_running(&self) -> bool {
        self.run_state == RunState::Running
    }

    /// Marks the target running (e.g., when transitioning cleanly).
    pub fn set_running(&mut self, running: bool) {
        self.run_state = if running {
            RunState::Running
        } else {
            RunState::Halted
        };
    }

    /// Populates `ctrl` with the union of desktop debugger controls and GDB's breakpoints/watchpoints.
    pub fn merge_debug_control(&self, base: &DebugControl, out: &mut DebugControl) {
        out.breakpoints.clear();
        out.breakpoints.extend_from_slice(&base.breakpoints);
        for &bp in &self.sw_breakpoints {
            if !out.breakpoints.contains(&bp) {
                out.breakpoints.push(bp);
            }
        }
        for &bp in &self.hw_breakpoints {
            if !out.breakpoints.contains(&bp) {
                out.breakpoints.push(bp);
            }
        }

        out.watchpoints.clear();
        out.watchpoints.extend_from_slice(&base.watchpoints);
        for wp in &self.watchpoints {
            if !out.watchpoints.contains(wp) {
                out.watchpoints.push(*wp);
            }
        }
    }

    /// Returns active GDB-registered breakpoints `(sw, hw)` and watchpoints.
    pub fn breakpoint_counts(&self) -> (usize, usize) {
        (
            self.sw_breakpoints.len() + self.hw_breakpoints.len(),
            self.watchpoints.len(),
        )
    }

    /// Checks whether a core [`StopReason`] matches a GDB breakpoint or watchpoint and formats
    /// the appropriate RSP stop-reply packet payload (`S05` / `T05...`).
    pub fn format_stop_reply(&mut self, reason: &StopReason) -> String {
        self.run_state = RunState::Halted;
        let reply = match reason {
            StopReason::Breakpoint(pc) => {
                let norm = pc & !1;
                if self.hw_breakpoints.contains(&norm) {
                    String::from("T05hwbreak:;")
                } else {
                    String::from("T05swbreak:;")
                }
            }
            StopReason::Watchpoint { addr, write, .. } => {
                let tag = self
                    .watchpoints
                    .iter()
                    .find(|w| *addr >= w.start && *addr <= w.end)
                    .map(|w| match w.kind {
                        WatchpointKind::Read => "rwatch",
                        WatchpointKind::Write => "watch",
                        WatchpointKind::ReadWrite => "awatch",
                    })
                    .unwrap_or(if *write { "watch" } else { "rwatch" });
                format!("T05{tag}:{addr:08x};")
            }
            StopReason::FrameComplete => String::from("S05"),
        };
        self.last_stop_reply = reply.clone();
        reply
    }

    /// Records a Ctrl+C interrupt halt and returns its stop reply payload (`"S02"`).
    pub fn interrupt_stop_reply(&mut self) -> String {
        self.run_state = RunState::Halted;
        let reply = String::from("S02");
        self.last_stop_reply = reply.clone();
        reply
    }

    /// Checks whether `reason` came from one of this session's own breakpoints/watchpoints.
    pub fn owns_stop_reason(&self, reason: &StopReason) -> bool {
        match reason {
            StopReason::Breakpoint(pc) => {
                let norm = pc & !1;
                self.sw_breakpoints.contains(&norm) || self.hw_breakpoints.contains(&norm)
            }
            StopReason::Watchpoint { addr, .. } => self
                .watchpoints
                .iter()
                .any(|w| *addr >= w.start && *addr <= w.end),
            StopReason::FrameComplete => false,
        }
    }

    /// Processes a Ctrl+C (`0x03`) interrupt event from the transport.
    pub fn handle_interrupt(&mut self) -> PacketOutcome {
        let reply = self.interrupt_stop_reply();
        PacketOutcome {
            reply: Some(reply),
            action: GdbAction::Break,
            enable_no_ack: false,
        }
    }

    /// Processes a single checksum-verified RSP packet payload against `core`.
    pub fn handle_packet(&mut self, core: &mut GbaCore, payload: &str) -> PacketOutcome {
        if payload == "?" {
            return Self::reply_ok(self.last_stop_reply.clone());
        }
        if payload == "g" {
            return Self::reply_ok(self.read_all_registers(core));
        }
        if let Some(hex) = payload.strip_prefix('G') {
            let reply = if self.write_all_registers(core, hex) {
                String::from("OK")
            } else {
                String::from("E01")
            };
            return Self::reply_ok(reply);
        }
        if let Some(reg_hex) = payload.strip_prefix('p') {
            return match parse_hex_u32(reg_hex) {
                Some(reg) => Self::reply_ok(self.read_single_register(core, reg)),
                None => Self::reply_ok(String::from("E01")),
            };
        }
        if let Some(rest) = payload.strip_prefix('P') {
            if let Some((reg_s, val_s)) = rest.split_once('=')
                && let (Some(reg), Some(val)) = (parse_hex_u32(reg_s), decode_u32_le_hex(val_s))
            {
                self.write_single_register(core, reg, val);
                return Self::reply_ok(String::from("OK"));
            }
            return Self::reply_ok(String::from("E01"));
        }
        if let Some(rest) = payload.strip_prefix('m') {
            return Self::reply_ok(self.read_memory(core, rest));
        }
        if let Some(rest) = payload.strip_prefix('M') {
            return Self::reply_ok(self.write_memory(core, rest));
        }
        if payload.starts_with('X') {
            // Unsupported binary X; GDB falls back cleanly to ASCII hex 'M'.
            return Self::reply_ok(String::new());
        }
        if let Some(rest) = payload.strip_prefix('c') {
            if !rest.is_empty() {
                if let Some(addr) = parse_hex_u32(rest) {
                    core.cpu.flush_pipeline(&mut core.bus, addr & !1);
                } else {
                    return Self::reply_ok(String::from("E01"));
                }
            }
            self.run_state = RunState::Running;
            return PacketOutcome {
                reply: None,
                action: GdbAction::Continue,
                enable_no_ack: false,
            };
        }
        if let Some(rest) = payload.strip_prefix('s') {
            if !rest.is_empty() {
                if let Some(addr) = parse_hex_u32(rest) {
                    core.cpu.flush_pipeline(&mut core.bus, addr & !1);
                } else {
                    return Self::reply_ok(String::from("E01"));
                }
            }
            self.run_state = RunState::Halted;
            return PacketOutcome {
                reply: None,
                action: GdbAction::Step,
                enable_no_ack: false,
            };
        }
        if payload == "vCont?" {
            return Self::reply_ok(String::from("vCont;c;s"));
        }
        if let Some(rest) = payload.strip_prefix("vCont;") {
            return self.handle_vcont(rest);
        }
        if let Some(rest) = payload.strip_prefix('Z') {
            let r = self.handle_insert_point(rest);
            return Self::reply_ok(r);
        }
        if let Some(rest) = payload.strip_prefix('z') {
            let r = self.handle_remove_point(rest);
            return Self::reply_ok(r);
        }
        if payload == "QStartNoAckMode" {
            self.no_ack_mode = true;
            return PacketOutcome {
                reply: Some(String::from("OK")),
                action: GdbAction::None,
                enable_no_ack: true,
            };
        }
        if payload.starts_with("qSupported") {
            return Self::reply_ok(String::from(
                "PacketSize=4000;QStartNoAckMode+;qXfer:features:read+;swbreak+;hwbreak+",
            ));
        }
        if let Some(rest) = payload.strip_prefix("qXfer:features:read:target.xml:") {
            let r = self.handle_xfer_target_xml(rest);
            return Self::reply_ok(r);
        }
        if payload.starts_with("qAttached") {
            return Self::reply_ok(String::from("1"));
        }
        if payload == "qC" {
            return Self::reply_ok(String::from("QC01"));
        }
        if payload == "qfThreadInfo" {
            return Self::reply_ok(String::from("m01"));
        }
        if payload == "qsThreadInfo" {
            return Self::reply_ok(String::from("l"));
        }
        if payload.starts_with('H') {
            return Self::reply_ok(String::from("OK"));
        }
        if payload.starts_with("vMustReplyEmpty") {
            return Self::reply_ok(String::new());
        }
        if payload == "D" || payload.starts_with("D;") {
            self.sw_breakpoints.clear();
            self.hw_breakpoints.clear();
            self.watchpoints.clear();
            self.run_state = RunState::Halted;
            return PacketOutcome {
                reply: Some(String::from("OK")),
                action: GdbAction::Detach,
                enable_no_ack: false,
            };
        }
        if payload == "k" {
            self.sw_breakpoints.clear();
            self.hw_breakpoints.clear();
            self.watchpoints.clear();
            self.run_state = RunState::Halted;
            return PacketOutcome {
                reply: None,
                action: GdbAction::Kill,
                enable_no_ack: false,
            };
        }

        // Unknown / unsupported query: empty reply per GDB RSP specification.
        Self::reply_ok(String::new())
    }

    fn reply_ok(s: String) -> PacketOutcome {
        PacketOutcome {
            reply: Some(s),
            action: GdbAction::None,
            enable_no_ack: false,
        }
    }

    fn handle_vcont(&mut self, rest: &str) -> PacketOutcome {
        let first_action = rest.split(';').next().unwrap_or("");
        let cmd = first_action.split(':').next().unwrap_or("");
        match cmd {
            "c" | "C" => {
                self.run_state = RunState::Running;
                PacketOutcome {
                    reply: None,
                    action: GdbAction::Continue,
                    enable_no_ack: false,
                }
            }
            "s" | "S" => {
                self.run_state = RunState::Halted;
                PacketOutcome {
                    reply: None,
                    action: GdbAction::Step,
                    enable_no_ack: false,
                }
            }
            _ => Self::reply_ok(String::new()),
        }
    }

    fn read_all_registers(&self, core: &GbaCore) -> String {
        if self.target_xml_read {
            // 17 registers described in ARM7TDMI_TARGET_XML: r0..r14, exec_pc, cpsr.
            let mut out = String::with_capacity(17 * 8);
            for i in 0..15 {
                encode_u32_le_hex(core.cpu.gpr[i], &mut out);
            }
            encode_u32_le_hex(core.cpu.current_exec_pc(), &mut out);
            encode_u32_le_hex(core.cpu.cpsr, &mut out);
            out
        } else {
            // Legacy 26-register ARM FPA layout (16*4 + 8*12 + 4 + 4 = 168 bytes).
            let mut out = String::with_capacity(168 * 2);
            for i in 0..15 {
                encode_u32_le_hex(core.cpu.gpr[i], &mut out);
            }
            encode_u32_le_hex(core.cpu.current_exec_pc(), &mut out);
            // 8 x 96-bit FPA registers (f0..f7) + 32-bit fps = 100 zero bytes (200 hex chars)
            for _ in 0..200 {
                out.push('0');
            }
            encode_u32_le_hex(core.cpu.cpsr, &mut out);
            out
        }
    }

    fn write_all_registers(&self, core: &mut GbaCore, hex: &str) -> bool {
        let Some(bytes) = decode_hex_bytes(hex) else {
            return false;
        };
        if bytes.len() < 16 * 4 {
            return false;
        }
        // Validate recognized total sizes: 68B (target.xml), 104B (26x32b), 168B (legacy FPA)
        if !matches!(bytes.len(), 68 | 104 | 168) {
            return false;
        }
        let read_u32 = |off: usize| -> u32 {
            u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
        };
        let cpsr_off = match bytes.len() {
            68 => 16 * 4,
            104 => 25 * 4,
            168 => 164,
            _ => return false,
        };
        let new_cpsr = read_u32(cpsr_off);
        core.cpu.set_cpsr(new_cpsr);
        for i in 0..15 {
            core.cpu.gpr[i] = read_u32(i * 4);
        }
        let new_pc = read_u32(15 * 4);
        core.cpu.flush_pipeline(&mut core.bus, new_pc);
        true
    }

    fn read_single_register(&self, core: &GbaCore, reg: u32) -> String {
        let mut out = String::with_capacity(8);
        let val = match reg {
            0..=14 => core.cpu.gpr[reg as usize],
            15 => core.cpu.current_exec_pc(),
            16 => {
                if self.target_xml_read {
                    core.cpu.cpsr
                } else {
                    0
                }
            }
            25 => core.cpu.cpsr,
            _ => 0,
        };
        encode_u32_le_hex(val, &mut out);
        out
    }

    fn write_single_register(&self, core: &mut GbaCore, reg: u32, val: u32) {
        match reg {
            0..=14 => core.cpu.gpr[reg as usize] = val,
            15 => core.cpu.flush_pipeline(&mut core.bus, val),
            16 if self.target_xml_read => core.cpu.set_cpsr(val),
            25 => core.cpu.set_cpsr(val),
            _ => {}
        }
    }

    fn read_memory(&self, core: &GbaCore, spec: &str) -> String {
        let Some((addr_s, len_s)) = spec.split_once(',') else {
            return String::from("E01");
        };
        let Some(addr) = parse_hex_u32(addr_s) else {
            return String::from("E01");
        };
        let Some(len) = parse_hex_usize(len_s) else {
            return String::from("E01");
        };
        let capped_len = len.min(0x4000);
        let mut out = String::with_capacity(capped_len * 2);
        for off in 0..capped_len {
            let b = core.peek8(addr.wrapping_add(off as u32));
            out.push_str(&format!("{b:02x}"));
        }
        out
    }

    fn write_memory(&self, core: &mut GbaCore, spec: &str) -> String {
        let Some((head, hex_data)) = spec.split_once(':') else {
            return String::from("E01");
        };
        let Some((addr_s, len_s)) = head.split_once(',') else {
            return String::from("E01");
        };
        let Some(addr) = parse_hex_u32(addr_s) else {
            return String::from("E01");
        };
        let Some(len) = parse_hex_usize(len_s) else {
            return String::from("E01");
        };
        let Some(bytes) = decode_hex_bytes(hex_data) else {
            return String::from("E01");
        };
        if bytes.len() != len {
            return String::from("E01");
        }
        for (off, b) in bytes.into_iter().enumerate() {
            core.poke8(addr.wrapping_add(off as u32), b);
        }
        String::from("OK")
    }

    fn handle_insert_point(&mut self, rest: &str) -> String {
        let mut parts = rest.split(',');
        let (Some(ty), Some(addr_s), Some(kind_s)) = (parts.next(), parts.next(), parts.next())
        else {
            return String::from("E01");
        };
        let (Some(addr), Some(kind)) = (parse_hex_u32(addr_s), parse_hex_u32(kind_s)) else {
            return String::from("E01");
        };
        match ty {
            "0" => {
                let norm = addr & !1;
                if !self.sw_breakpoints.contains(&norm) {
                    self.sw_breakpoints.push(norm);
                }
                String::from("OK")
            }
            "1" => {
                let norm = addr & !1;
                if !self.hw_breakpoints.contains(&norm) {
                    self.hw_breakpoints.push(norm);
                }
                String::from("OK")
            }
            "2" | "3" | "4" => {
                let wp_kind = match ty {
                    "2" => WatchpointKind::Write,
                    "3" => WatchpointKind::Read,
                    _ => WatchpointKind::ReadWrite,
                };
                let span = kind.max(1);
                let end = addr.saturating_add(span - 1);
                let wp = Watchpoint::new(addr, end, wp_kind);
                if !self.watchpoints.contains(&wp) {
                    self.watchpoints.push(wp);
                }
                String::from("OK")
            }
            _ => String::new(),
        }
    }

    fn handle_remove_point(&mut self, rest: &str) -> String {
        let mut parts = rest.split(',');
        let (Some(ty), Some(addr_s), Some(kind_s)) = (parts.next(), parts.next(), parts.next())
        else {
            return String::from("E01");
        };
        let (Some(addr), Some(kind)) = (parse_hex_u32(addr_s), parse_hex_u32(kind_s)) else {
            return String::from("E01");
        };
        match ty {
            "0" => {
                self.sw_breakpoints.retain(|&bp| bp != (addr & !1));
                String::from("OK")
            }
            "1" => {
                self.hw_breakpoints.retain(|&bp| bp != (addr & !1));
                String::from("OK")
            }
            "2" | "3" | "4" => {
                let wp_kind = match ty {
                    "2" => WatchpointKind::Write,
                    "3" => WatchpointKind::Read,
                    _ => WatchpointKind::ReadWrite,
                };
                let span = kind.max(1);
                let end = addr.saturating_add(span - 1);
                let target = Watchpoint::new(addr, end, wp_kind);
                self.watchpoints.retain(|w| *w != target);
                String::from("OK")
            }
            _ => String::new(),
        }
    }

    fn handle_xfer_target_xml(&mut self, rest: &str) -> String {
        let Some((off_s, len_s)) = rest.split_once(',') else {
            return String::from("E01");
        };
        let (Some(offset), Some(length)) = (parse_hex_usize(off_s), parse_hex_usize(len_s)) else {
            return String::from("E01");
        };
        self.target_xml_read = true;
        let data = ARM7TDMI_TARGET_XML.as_bytes();
        if offset >= data.len() {
            return String::from("l");
        }
        let end = offset.saturating_add(length).min(data.len());
        let escaped = escape_rsp_bytes(&data[offset..end]);
        let prefix = if end >= data.len() { 'l' } else { 'm' };
        let mut reply = String::with_capacity(1 + escaped.len());
        reply.push(prefix);
        reply.push_str(&String::from_utf8_lossy(&escaped));
        reply
    }
}
