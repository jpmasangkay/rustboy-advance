use gba_core::{DebugControl, GbaCore, StopReason};
use std::io::{self, ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::string::String;
use std::vec::Vec;

pub use crate::rsp::{
    ARM7TDMI_TARGET_XML, GdbAction, GdbSession, PacketOutcome, RspEvent, RspParser,
    encode_rsp_packet, escape_rsp_bytes, rsp_checksum, unescape_rsp_bytes,
};

/// Result produced by polling [`GdbStub`] for network traffic or protocol steps.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GdbPollOutcome {
    /// Emulation pause state transition requested (`Some(true)` = pause, `Some(false)` = run).
    pub pause_request: Option<bool>,
    /// Stop reason observed immediately during an inline single step (`s`) or `step_protocol`.
    pub stop_reason: Option<StopReason>,
    /// Client issued `k` (kill).
    pub kill_requested: bool,
    /// Client issued `D` (detach).
    pub detached: bool,
    /// Peer address if a new client connected on this poll.
    pub client_connected: Option<String>,
    /// True if the active client disconnected on this poll.
    pub client_disconnected: bool,
    /// Non-fatal socket error summary, if any.
    pub last_error: Option<String>,
}

const MAX_READ_BYTES_PER_POLL: usize = 64 * 1024;
const MAX_EVENTS_PER_POLL: usize = 256;
const MAX_WRITE_QUEUE_BYTES: usize = 256 * 1024;
const MAX_DMA_STEP_TICKS: usize = 65_536;

/// Non-blocking TCP GDB Remote Serial Protocol server for [`GbaCore`].
pub struct GdbStub {
    listener: Option<TcpListener>,
    port: u16,
    client: Option<TcpStream>,
    client_addr: Option<String>,
    parser: RspParser,
    /// Underlying `#![no_std]` RSP session state and GDB breakpoints/watchpoints.
    pub session: GdbSession,
    last_sent: Option<String>,
    write_queue: Vec<u8>,
}

impl std::fmt::Debug for GdbStub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GdbStub")
            .field("listening", &self.listener.is_some())
            .field("port", &self.port)
            .field("client_addr", &self.client_addr)
            .finish()
    }
}

impl Default for GdbStub {
    fn default() -> Self {
        Self::new()
    }
}

impl GdbStub {
    /// Creates an inactive GDB stub. Call [`Self::start`] to listen on localhost.
    pub fn new() -> Self {
        Self {
            listener: None,
            port: 2345,
            client: None,
            client_addr: None,
            parser: RspParser::new(),
            session: GdbSession::new(),
            last_sent: None,
            write_queue: Vec::new(),
        }
    }

    /// Starts listening for GDB clients on `127.0.0.1:<port>` (pass `0` for an ephemeral port).
    ///
    /// Returns the actual bound TCP port.
    pub fn start(&mut self, port: u16) -> io::Result<u16> {
        self.stop();
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        listener.set_nonblocking(true)?;
        let bound_port = listener.local_addr()?.port();
        self.port = bound_port;
        self.listener = Some(listener);
        Ok(bound_port)
    }

    /// Closes any connected client and stops the listening socket.
    pub fn stop(&mut self) {
        self.disconnect_client();
        self.listener = None;
    }

    /// Returns `true` while listening on a TCP port.
    pub fn is_listening(&self) -> bool {
        self.listener.is_some()
    }

    /// Returns the configured/bound TCP port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Returns `true` if a GDB client is currently connected.
    pub fn is_client_connected(&self) -> bool {
        self.client.is_some()
    }

    /// Returns the peer socket address of the connected client, if any.
    pub fn client_addr(&self) -> Option<&str> {
        self.client_addr.as_deref()
    }

    /// Populates `out` with `base` plus GDB-registered breakpoints and watchpoints.
    pub fn merged_debug_control(&self, base: &DebugControl, out: &mut DebugControl) {
        self.session.merge_debug_control(base, out);
    }

    /// Formats and transmits an asynchronous halt notification (`S05`/`T05...`) to the client.
    ///
    /// Ignored when the GDB session is already halted (e.g., after an inline single step already
    /// transmitted its stop reply) so duplicate `$S05` / `$T05` packets are never sent.
    pub fn notify_stop(&mut self, reason: &StopReason) {
        if self.client.is_none() || !self.session.is_running() {
            return;
        }
        let payload = self.session.format_stop_reply(reason);
        self.queue_packet(&payload);
        let _ = self.flush_writes();
    }

    /// Polls the listener and active client stream once without advancing free-running frames.
    ///
    /// Single-step packets (`s` / `vCont;s`) are executed immediately against `core` and reply
    /// synchronously with their stop reason. Continuous execution (`c` / `vCont;c`) returns
    /// `pause_request = Some(false)` so the host run-loop resumes emulation.
    pub fn poll_network(&mut self, core: &mut GbaCore, user_ctrl: &DebugControl) -> GdbPollOutcome {
        let mut outcome = GdbPollOutcome::default();

        self.accept_pending_client(&mut outcome);
        if self.client.is_none() {
            return outcome;
        }

        if let Some(err) = self.flush_writes() {
            outcome.last_error = Some(err);
            outcome.client_disconnected = true;
            return outcome;
        }

        let mut events = Vec::new();
        let mut disconnect = false;
        let mut buf = [0u8; 4096];
        let mut bytes_read = 0usize;

        while bytes_read < MAX_READ_BYTES_PER_POLL && events.len() < MAX_EVENTS_PER_POLL {
            let Some(stream) = self.client.as_mut() else {
                break;
            };
            let max_chunk = buf.len().min(MAX_READ_BYTES_PER_POLL - bytes_read);
            match stream.read(&mut buf[..max_chunk]) {
                Ok(0) => {
                    disconnect = true;
                    break;
                }
                Ok(n) => {
                    bytes_read += n;
                    self.parser.feed(&buf[..n], &mut events);
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => {
                    outcome.last_error = Some(std::format!("GDB socket read error: {e}"));
                    disconnect = true;
                    break;
                }
            }
        }

        for event in events {
            if self.client.is_none() || disconnect {
                break;
            }
            match event {
                RspEvent::Ack => {}
                RspEvent::Nack => {
                    if let Some(frame) = self.last_sent.clone() {
                        self.push_write_bytes(frame.as_bytes(), &mut disconnect, &mut outcome);
                    }
                }
                RspEvent::BadChecksum => {
                    if !self.session.no_ack_mode {
                        self.push_write_bytes(b"-", &mut disconnect, &mut outcome);
                    }
                }
                RspEvent::Interrupt => {
                    let pkt = self.session.handle_interrupt();
                    outcome.pause_request = Some(true);
                    if let Some(reply) = pkt.reply {
                        self.queue_packet(&reply);
                    }
                }
                RspEvent::Packet(payload) => {
                    let was_no_ack = self.session.no_ack_mode;
                    if !was_no_ack {
                        self.push_write_bytes(b"+", &mut disconnect, &mut outcome);
                    }
                    let pkt = self.session.handle_packet(core, &payload);
                    if let Some(reply) = pkt.reply {
                        self.queue_packet(&reply);
                    }
                    match pkt.action {
                        GdbAction::None => {}
                        GdbAction::Continue => {
                            outcome.pause_request = Some(false);
                        }
                        GdbAction::Step => {
                            // Exclude the current PC from breakpoints so stepping off an active
                            // breakpoint executes the instruction instead of deadlocking at it.
                            let mut merged = DebugControl::default();
                            self.session.merge_debug_control(user_ctrl, &mut merged);
                            let cur_pc = core.cpu.current_exec_pc() & !1;
                            merged.breakpoints.retain(|&bp| (bp & !1) != cur_pc);

                            // Advance past any active DMA ticks so a single-step retires a CPU instruction.
                            let mut hit = None;
                            for _ in 0..MAX_DMA_STEP_TICKS {
                                let was_dma = core.bus.hw.active_dma.is_some();
                                hit = core.step_one_debug(&merged);
                                if hit.is_some() || !was_dma {
                                    break;
                                }
                            }
                            let reason = hit.unwrap_or(StopReason::FrameComplete);
                            let reply = self.session.format_stop_reply(&reason);
                            self.queue_packet(&reply);
                            outcome.pause_request = Some(true);
                            outcome.stop_reason = Some(reason);
                        }
                        GdbAction::Break => {
                            outcome.pause_request = Some(true);
                        }
                        GdbAction::Detach => {
                            outcome.detached = true;
                            disconnect = true;
                        }
                        GdbAction::Kill => {
                            outcome.kill_requested = true;
                            outcome.pause_request = Some(true);
                            disconnect = true;
                        }
                    }
                }
            }
            if self.write_queue.len() > MAX_WRITE_QUEUE_BYTES {
                outcome.last_error = Some(String::from("GDB write queue overflow; disconnecting"));
                disconnect = true;
            }
        }

        if let Some(err) = self.flush_writes() {
            if outcome.last_error.is_none() {
                outcome.last_error = Some(err);
            }
            outcome.client_disconnected = true;
            return outcome;
        }
        if disconnect {
            self.disconnect_client();
            outcome.client_disconnected = true;
        }
        outcome
    }

    /// Standalone protocol step: processes network packets and, while the session is in the
    /// running state, executes one instruction checking GDB breakpoints and watchpoints.
    pub fn step_protocol(&mut self, core: &mut GbaCore) -> GdbPollOutcome {
        let empty = DebugControl::default();
        let mut outcome = self.poll_network(core, &empty);
        if self.client.is_some() && self.session.is_running() && outcome.stop_reason.is_none() {
            let mut merged = DebugControl::default();
            self.session.merge_debug_control(&empty, &mut merged);
            if let Some(hit) = core.step_one_debug(&merged) {
                self.notify_stop(&hit);
                outcome.pause_request = Some(true);
                outcome.stop_reason = Some(hit);
            }
        }
        outcome
    }

    fn accept_pending_client(&mut self, outcome: &mut GdbPollOutcome) {
        let Some(listener) = self.listener.as_ref() else {
            return;
        };
        loop {
            match listener.accept() {
                Ok((stream, addr)) => {
                    if self.client.is_some() {
                        // Replace stale client or reject secondary client? Keep active client clean.
                        drop(stream);
                        continue;
                    }
                    let _ = stream.set_nonblocking(true);
                    let _ = stream.set_nodelay(true);
                    let addr_str = std::format!("{addr}");
                    self.parser.reset();
                    self.session.reset_connection();
                    self.last_sent = None;
                    self.write_queue.clear();
                    self.client_addr = Some(addr_str.clone());
                    self.client = Some(stream);
                    outcome.client_connected = Some(addr_str);
                    outcome.pause_request = Some(true);
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => {
                    outcome.last_error = Some(std::format!("GDB accept error: {e}"));
                    break;
                }
            }
        }
    }

    fn push_write_bytes(
        &mut self,
        bytes: &[u8],
        disconnect: &mut bool,
        outcome: &mut GdbPollOutcome,
    ) {
        if self.write_queue.len().saturating_add(bytes.len()) > MAX_WRITE_QUEUE_BYTES {
            outcome.last_error = Some(String::from("GDB write queue overflow; disconnecting"));
            *disconnect = true;
            return;
        }
        self.write_queue.extend_from_slice(bytes);
    }

    fn queue_packet(&mut self, payload: &str) {
        let frame = encode_rsp_packet(payload);
        self.write_queue.extend_from_slice(frame.as_bytes());
        self.last_sent = Some(frame);
    }

    fn flush_writes(&mut self) -> Option<String> {
        let Some(stream) = self.client.as_mut() else {
            self.write_queue.clear();
            return None;
        };
        while !self.write_queue.is_empty() {
            match stream.write(&self.write_queue) {
                Ok(0) => break,
                Ok(n) => {
                    self.write_queue.drain(..n);
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => {
                    let msg = std::format!("GDB socket write error: {e}");
                    self.disconnect_client();
                    return Some(msg);
                }
            }
        }
        None
    }

    fn disconnect_client(&mut self) {
        self.client = None;
        self.client_addr = None;
        self.parser.reset();
        self.session.reset_connection();
        self.last_sent = None;
        self.write_queue.clear();
    }
}
