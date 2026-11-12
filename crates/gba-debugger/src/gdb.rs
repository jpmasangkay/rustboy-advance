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
    pub fn notify_stop(&mut self, reason: &StopReason) {
        if self.client.is_none() {
            return;
        }
        let payload = self.session.format_stop_reply(reason);
        self.queue_packet(&payload);
        self.flush_writes();
    }

    /// Polls the listener and active client stream once without advancing free-running frames.
    ///
    /// Single-step packets (`s` / `vCont;s`) are executed immediately against `core` and reply
    /// synchronously with their stop reason. Continuous execution (`c` / `vCont;c`) returns
    /// `pause_request = Some(false)` so the host run-loop resumes emulation.
    pub fn poll_network(
        &mut self,
        core: &mut GbaCore,
        user_ctrl: &DebugControl,
    ) -> GdbPollOutcome {
        let mut outcome = GdbPollOutcome::default();

        self.accept_pending_client(&mut outcome);
        if self.client.is_none() {
            return outcome;
        }

        self.flush_writes();

        let mut events = Vec::new();
        let mut disconnect = false;
        let mut buf = [0u8; 4096];

        while let Some(stream) = self.client.as_mut() {
            match stream.read(&mut buf) {
                Ok(0) => {
                    disconnect = true;
                    break;
                }
                Ok(n) => {
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
            if self.client.is_none() {
                break;
            }
            match event {
                RspEvent::Ack => {}
                RspEvent::Nack => {
                    if let Some(frame) = self.last_sent.clone() {
                        self.write_queue.extend_from_slice(frame.as_bytes());
                    }
                }
                RspEvent::BadChecksum => {
                    if !self.session.no_ack_mode {
                        self.write_queue.push(b'-');
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
                        self.write_queue.push(b'+');
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
                            let mut merged = DebugControl::default();
                            self.session.merge_debug_control(user_ctrl, &mut merged);
                            let hit = core.step_one_debug(&merged);
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
        }

        self.flush_writes();
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

    fn queue_packet(&mut self, payload: &str) {
        let frame = encode_rsp_packet(payload);
        self.write_queue.extend_from_slice(frame.as_bytes());
        self.last_sent = Some(frame);
    }

    fn flush_writes(&mut self) {
        let Some(stream) = self.client.as_mut() else {
            self.write_queue.clear();
            return;
        };
        while !self.write_queue.is_empty() {
            match stream.write(&self.write_queue) {
                Ok(0) => break,
                Ok(n) => {
                    self.write_queue.drain(..n);
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => {
                    self.disconnect_client();
                    break;
                }
            }
        }
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
