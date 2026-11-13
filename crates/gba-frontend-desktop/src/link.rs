//! GBA Link Cable emulation: Local 2-Player lockstep + TCP Netplay bridge.

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use egui::{self, Color32, CornerRadius, RichText, Stroke};
use gba_core::hw::{SioMode, SioResponse, SioTransfer};
use gba_core::{CYCLES_PER_FRAME, GbaCore, HostFrontend, LCD_HEIGHT, LCD_WIDTH};

use crate::app::{ACCENT_TEAL, BG_CARD, BG_PANEL, BORDER_HAIRLINE, RustboyApp, TEXT_MUTED};
use crate::filters::VideoPostProcessor;
use crate::rom_loader::{RomPlatform, detect_rom_platform, load_rom_bytes};

/// Default TCP port for GBA Netplay Link Cable sessions.
pub const DEFAULT_NETPLAY_PORT: u16 = 27888;

const NETPLAY_PROTOCOL_VERSION: u8 = 1;
const SUBFRAME_CHUNK_IDLE_CYCLES: u64 = 1_232;
const SUBFRAME_CHUNK_SIO_CYCLES: u64 = 256;
const NETPLAY_CHUNK_CYCLES: u64 = 35_112; // ~8 lockstep polls per frame
const LOCAL_SIO_PROBE_TIMEOUT_CYCLES: u64 = CYCLES_PER_FRAME;
const NETPLAY_SIO_TIMEOUT: Duration = Duration::from_millis(350);
const MAX_FRAME_PACKET_BYTES: usize = 64 * 1024;

/// Transfer counters displayed in the Link Cable diagnostics UI.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct SioStats {
    pub normal8: u64,
    pub normal32: u64,
    pub multi: u64,
    pub uart: u64,
    pub timeouts: u64,
}

impl SioStats {
    /// Total completed link exchanges across all SIO modes.
    #[allow(dead_code)]
    pub fn total(&self) -> u64 {
        self.normal8 + self.normal32 + self.multi + self.uart
    }

    fn record_mode(&mut self, mode: SioMode) {
        match mode {
            SioMode::Normal8 => self.normal8 += 1,
            SioMode::Normal32 => self.normal32 += 1,
            SioMode::Multi => self.multi += 1,
            SioMode::Uart => self.uart += 1,
        }
    }
}

#[derive(Copy, Clone, Debug)]
struct LatchedSio {
    transfer: SioTransfer,
    age_cycles: u64,
}

/// Active-low 10-bit keypad bitmask for Player 2's secondary keyboard bindings.
///
/// Mapping: `I/J/K/L` = D-Pad Up/Left/Down/Right, `O/U` = A/B, `N/M` = Start/Select,
/// `7/8` = L/R shoulders.
pub(crate) fn p2_keyboard_mask(ctx: &egui::Context) -> u16 {
    if ctx.wants_keyboard_input() {
        return 0x03FF;
    }
    ctx.input(|inp| {
        let mut mask: u16 = 0x03FF;
        let press = |mask: &mut u16, bit: u16, down: bool| {
            if down {
                *mask &= !(1 << bit);
            }
        };
        press(&mut mask, 0, inp.key_down(egui::Key::O));
        press(&mut mask, 1, inp.key_down(egui::Key::U));
        press(&mut mask, 2, inp.key_down(egui::Key::M));
        press(&mut mask, 3, inp.key_down(egui::Key::N));
        press(&mut mask, 4, inp.key_down(egui::Key::L));
        press(&mut mask, 5, inp.key_down(egui::Key::J));
        press(&mut mask, 6, inp.key_down(egui::Key::I));
        press(&mut mask, 7, inp.key_down(egui::Key::K));
        press(&mut mask, 8, inp.key_down(egui::Key::Num8));
        press(&mut mask, 9, inp.key_down(egui::Key::Num7));
        mask
    })
}

fn core_is_multi_ready(core: &GbaCore) -> (bool, u16) {
    let rcnt = core.bus.hw.rcnt;
    let siocnt = core.bus.hw.siocnt;
    let ready = (rcnt & 0x8000) == 0 && ((siocnt >> 12) & 0x03) == 2;
    (ready, core.bus.hw.siodata8)
}

fn build_open_bus_timeout_response(xfer: SioTransfer, multi_id: u8) -> SioResponse {
    SioResponse {
        data8: 0xFF,
        data32: 0xFFFF_FFFF,
        multi: match multi_id {
            0 => [xfer.data8_or_send, 0xFFFF, 0xFFFF, 0xFFFF],
            1 => [0xFFFF, xfer.data8_or_send, 0xFFFF, 0xFFFF],
            _ => [0xFFFF; 4],
        },
        multi_id,
        error: true,
    }
}

fn pair_responses(p1: SioTransfer, p2: SioTransfer) -> Option<(SioResponse, SioResponse, SioMode)> {
    if p1.mode != p2.mode {
        return None;
    }
    let resp_p1 = SioResponse {
        data8: p2.data8_or_send as u8,
        data32: p2.data32,
        multi: [p1.data8_or_send, p2.data8_or_send, 0xFFFF, 0xFFFF],
        multi_id: 0,
        error: false,
    };
    let resp_p2 = SioResponse {
        data8: p1.data8_or_send as u8,
        data32: p1.data32,
        multi: [p1.data8_or_send, p2.data8_or_send, 0xFFFF, 0xFFFF],
        multi_id: 1,
        error: false,
    };
    Some((resp_p1, resp_p2, p1.mode))
}

fn step_core_until(core: &mut GbaCore, target_cycle: u64) -> Option<[u16; LCD_WIDTH * LCD_HEIGHT]> {
    let mut ready_fb = None;
    while core.bus.scheduler.current_cycle < target_cycle {
        core.step_one();
        if core.bus.ppu.frame_ready {
            ready_fb = Some(*core.bus.ppu.framebuffer);
            core.bus.ppu.frame_ready = false;
        }
    }
    ready_fb
}

fn flush_core_to_frontend<F: HostFrontend + ?Sized>(core: &mut GbaCore, fe: &mut F) {
    while let Some((ev_cycle, kind)) = core.bus.scheduler.pop_due() {
        core.bus.dispatch_event(ev_cycle, kind);
    }
    if core.bus.ppu.frame_ready {
        fe.present_frame(&core.bus.ppu.framebuffer);
        core.bus.ppu.frame_ready = false;
    }
    core.bus.apu.flush(core.bus.scheduler.current_cycle);
    if !core.bus.apu.output_buffer.is_empty() {
        fe.push_audio_samples(&core.bus.apu.output_buffer);
        core.bus.apu.output_buffer.clear();
    }
}

fn flush_core_framebuffer_only(core: &mut GbaCore, fb_out: &mut [u16; LCD_WIDTH * LCD_HEIGHT]) {
    while let Some((ev_cycle, kind)) = core.bus.scheduler.pop_due() {
        core.bus.dispatch_event(ev_cycle, kind);
    }
    if core.bus.ppu.frame_ready {
        *fb_out = *core.bus.ppu.framebuffer;
        core.bus.ppu.frame_ready = false;
    }
    core.bus.apu.flush(core.bus.scheduler.current_cycle);
    core.bus.apu.output_buffer.clear();
}

/// Lockstep 2-player local GBA Link Cable session running Player 2 in-process.
pub struct LocalLinkSession {
    pub p2_core: GbaCore,
    pub p2_framebuffer: [u16; LCD_WIDTH * LCD_HEIGHT],
    pub(crate) p2_postproc: VideoPostProcessor,
    pub(crate) p2_lcd_tex: Option<egui::TextureHandle>,
    pub p2_rom_title: String,
    pub p2_rom_code: String,
    pub p2_rom_path: Option<PathBuf>,
    pub p2_sav_path: Option<PathBuf>,
    pub p2_save_write_enabled: bool,
    pub split_view: bool,
    pub stats: SioStats,
    pending_p1: Option<LatchedSio>,
    pending_p2: Option<LatchedSio>,
}

impl LocalLinkSession {
    /// Creates a new local 2-player Link session with an already constructed Player 2 `GbaCore`.
    pub fn new(
        mut p2_core: GbaCore,
        p2_rom_path: Option<PathBuf>,
        p2_sav_path: Option<PathBuf>,
        p2_save_write_enabled: bool,
    ) -> Self {
        p2_core.set_link_connected(true);
        let p2_rom_title = p2_core.bus.cart.game_title();
        let p2_rom_code = p2_core.bus.cart.game_code();
        Self {
            p2_core,
            p2_framebuffer: [0; LCD_WIDTH * LCD_HEIGHT],
            p2_postproc: VideoPostProcessor::new(),
            p2_lcd_tex: None,
            p2_rom_title,
            p2_rom_code,
            p2_rom_path,
            p2_sav_path,
            p2_save_write_enabled,
            split_view: true,
            stats: SioStats::default(),
            pending_p1: None,
            pending_p2: None,
        }
    }

    /// Flushes Player 2's battery `.sav` when Player 2 uses an independent ROM file.
    pub fn flush_p2_battery_save(&mut self) {
        if !self.p2_save_write_enabled {
            return;
        }
        let Some(sav_path) = &self.p2_sav_path else {
            return;
        };
        if !self.p2_core.bus.cart.save_dirty {
            return;
        }
        let data = self.p2_core.bus.cart.export_save_data();
        if !data.is_empty() && std::fs::write(sav_path, data).is_ok() {
            self.p2_core.bus.cart.save_dirty = false;
        }
    }

    /// Advances P1 and P2 in sub-frame lockstep, exchanging Normal8/Normal32/Multi SIO packets.
    pub fn step_frame<F: HostFrontend + ?Sized>(
        &mut self,
        p1: &mut GbaCore,
        p1_fe: &mut F,
        p2_keypad: u16,
    ) {
        p1.set_link_connected(true);
        self.p2_core.set_link_connected(true);

        let p1_kp = p1_fe.poll_keypad() & 0x03FF;
        p1.bus.hw.update_keypad(p1_kp);
        self.p2_core.bus.hw.update_keypad(p2_keypad & 0x03FF);

        let p1_start = p1.bus.scheduler.current_cycle;
        let p2_start = self.p2_core.bus.scheduler.current_cycle;
        let mut offset = 0u64;

        while offset < CYCLES_PER_FRAME {
            let chunk = if self.pending_p1.is_some() || self.pending_p2.is_some() {
                SUBFRAME_CHUNK_SIO_CYCLES
            } else {
                SUBFRAME_CHUNK_IDLE_CYCLES
            };
            offset = (offset + chunk).min(CYCLES_PER_FRAME);

            if let Some(fb) = step_core_until(p1, p1_start + offset) {
                p1_fe.present_frame(&fb);
            }
            if let Some(fb2) = step_core_until(&mut self.p2_core, p2_start + offset) {
                self.p2_framebuffer = fb2;
            }

            if self.pending_p1.is_none()
                && let Some(xfer) = p1.take_pending_sio_transfer()
            {
                self.pending_p1 = Some(LatchedSio {
                    transfer: xfer,
                    age_cycles: 0,
                });
            }
            if self.pending_p2.is_none()
                && let Some(xfer) = self.p2_core.take_pending_sio_transfer()
            {
                self.pending_p2 = Some(LatchedSio {
                    transfer: xfer,
                    age_cycles: 0,
                });
            }

            if let Some(l1) = &mut self.pending_p1 {
                l1.age_cycles = l1.age_cycles.saturating_add(chunk);
            }
            if let Some(l2) = &mut self.pending_p2 {
                l2.age_cycles = l2.age_cycles.saturating_add(chunk);
            }

            self.resolve_pending(p1);
        }

        flush_core_to_frontend(p1, p1_fe);
        flush_core_framebuffer_only(&mut self.p2_core, &mut self.p2_framebuffer);
    }

    fn resolve_pending(&mut self, p1: &mut GbaCore) {
        if let (Some(l1), Some(l2)) = (self.pending_p1, self.pending_p2) {
            let req = l1.transfer.duration_cycles.max(l2.transfer.duration_cycles);
            if l1.transfer.mode == l2.transfer.mode {
                if l1.age_cycles.max(l2.age_cycles) >= req
                    && let Some((r1, r2, mode)) = pair_responses(l1.transfer, l2.transfer)
                {
                    p1.complete_sio_transfer(r1);
                    self.p2_core.complete_sio_transfer(r2);
                    self.pending_p1 = None;
                    self.pending_p2 = None;
                    self.stats.record_mode(mode);
                }
                return;
            }
        }

        // Handle Multi mode where Master writes SIOCNT.Start while Slave leaves SIOCNT.Start=0.
        if let Some(l1) = self.pending_p1
            && self.pending_p2.is_none()
            && l1.transfer.mode == SioMode::Multi
            && l1.age_cycles >= l1.transfer.duration_cycles
        {
            let (p2_multi_ready, p2_send) = core_is_multi_ready(&self.p2_core);
            if p2_multi_ready {
                let multi = [l1.transfer.data8_or_send, p2_send, 0xFFFF, 0xFFFF];
                p1.complete_sio_transfer(SioResponse {
                    data8: p2_send as u8,
                    data32: 0,
                    multi,
                    multi_id: 0,
                    error: false,
                });
                self.p2_core.complete_sio_transfer(SioResponse {
                    data8: l1.transfer.data8_or_send as u8,
                    data32: 0,
                    multi,
                    multi_id: 1,
                    error: false,
                });
                self.pending_p1 = None;
                self.stats.multi += 1;
                return;
            }
        }

        if let Some(l2) = self.pending_p2
            && self.pending_p1.is_none()
            && l2.transfer.mode == SioMode::Multi
            && l2.age_cycles >= l2.transfer.duration_cycles
        {
            let (p1_multi_ready, p1_send) = core_is_multi_ready(p1);
            if p1_multi_ready {
                let multi = [p1_send, l2.transfer.data8_or_send, 0xFFFF, 0xFFFF];
                p1.complete_sio_transfer(SioResponse {
                    data8: l2.transfer.data8_or_send as u8,
                    data32: 0,
                    multi,
                    multi_id: 0,
                    error: false,
                });
                self.p2_core.complete_sio_transfer(SioResponse {
                    data8: p1_send as u8,
                    data32: 0,
                    multi,
                    multi_id: 1,
                    error: false,
                });
                self.pending_p2 = None;
                self.stats.multi += 1;
                return;
            }
        }

        // Prevent internal-clock probes from hanging indefinitely when peer is not in SIO mode.
        if let Some(l1) = self.pending_p1
            && l1.transfer.duration_cycles > 0
            && l1.age_cycles
                >= l1
                    .transfer
                    .duration_cycles
                    .max(LOCAL_SIO_PROBE_TIMEOUT_CYCLES)
        {
            p1.complete_sio_transfer(build_open_bus_timeout_response(l1.transfer, 0));
            self.pending_p1 = None;
            self.stats.timeouts += 1;
        }
        if let Some(l2) = self.pending_p2
            && l2.transfer.duration_cycles > 0
            && l2.age_cycles
                >= l2
                    .transfer
                    .duration_cycles
                    .max(LOCAL_SIO_PROBE_TIMEOUT_CYCLES)
        {
            self.p2_core
                .complete_sio_transfer(build_open_bus_timeout_response(l2.transfer, 1));
            self.pending_p2 = None;
            self.stats.timeouts += 1;
        }
    }
}

/// Role of a TCP Netplay Link participant.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NetplayRole {
    Host,
    Join,
}

/// Observable connection lifecycle state for TCP Netplay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetplayState {
    Listening {
        addr: SocketAddr,
    },
    Connecting {
        target: String,
    },
    Connected {
        peer_addr: String,
        peer_title: String,
        peer_code: String,
    },
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum NetPacket {
    Hello {
        version: u8,
        game_code: [u8; 4],
        rom_title: String,
    },
    SioTransfer {
        seq: u32,
        transfer: SioTransfer,
    },
    SioResponse {
        seq: u32,
        response: SioResponse,
    },
    FrameHeartbeat {
        frame: u64,
        multi_ready: bool,
        multi_send: u16,
        timestamp_ms: u32,
    },
    Pong {
        timestamp_ms: u32,
    },
    Disconnect,
}

impl NetPacket {
    fn encode(&self) -> Vec<u8> {
        let mut body = Vec::with_capacity(48);
        match self {
            Self::Hello {
                version,
                game_code,
                rom_title,
            } => {
                body.push(1);
                body.push(*version);
                body.extend_from_slice(game_code);
                let title_bytes = rom_title.as_bytes();
                let tlen = (title_bytes.len().min(64)) as u16;
                body.extend_from_slice(&tlen.to_be_bytes());
                body.extend_from_slice(&title_bytes[..tlen as usize]);
            }
            Self::SioTransfer { seq, transfer } => {
                body.push(2);
                body.extend_from_slice(&seq.to_be_bytes());
                body.push(match transfer.mode {
                    SioMode::Normal8 => 0,
                    SioMode::Normal32 => 1,
                    SioMode::Multi => 2,
                    SioMode::Uart => 3,
                });
                body.extend_from_slice(&transfer.siocnt.to_be_bytes());
                body.extend_from_slice(&transfer.data8_or_send.to_be_bytes());
                body.extend_from_slice(&transfer.data32.to_be_bytes());
                body.extend_from_slice(&transfer.duration_cycles.to_be_bytes());
            }
            Self::SioResponse { seq, response } => {
                body.push(3);
                body.extend_from_slice(&seq.to_be_bytes());
                body.push(response.data8);
                body.extend_from_slice(&response.data32.to_be_bytes());
                for slot in response.multi {
                    body.extend_from_slice(&slot.to_be_bytes());
                }
                body.push(response.multi_id);
                body.push(u8::from(response.error));
            }
            Self::FrameHeartbeat {
                frame,
                multi_ready,
                multi_send,
                timestamp_ms,
            } => {
                body.push(4);
                body.extend_from_slice(&frame.to_be_bytes());
                body.push(u8::from(*multi_ready));
                body.extend_from_slice(&multi_send.to_be_bytes());
                body.extend_from_slice(&timestamp_ms.to_be_bytes());
            }
            Self::Pong { timestamp_ms } => {
                body.push(5);
                body.extend_from_slice(&timestamp_ms.to_be_bytes());
            }
            Self::Disconnect => {
                body.push(6);
            }
        }
        let mut framed = Vec::with_capacity(4 + body.len());
        framed.extend_from_slice(&(body.len() as u32).to_be_bytes());
        framed.extend_from_slice(&body);
        framed
    }

    fn decode(body: &[u8]) -> Option<Self> {
        let tag = *body.first()?;
        let p = &body[1..];
        let u16_at = |off: usize| -> Option<u16> {
            Some(u16::from_be_bytes([*p.get(off)?, *p.get(off + 1)?]))
        };
        let u32_at = |off: usize| -> Option<u32> {
            Some(u32::from_be_bytes([
                *p.get(off)?,
                *p.get(off + 1)?,
                *p.get(off + 2)?,
                *p.get(off + 3)?,
            ]))
        };
        let u64_at = |off: usize| -> Option<u64> {
            Some(u64::from_be_bytes([
                *p.get(off)?,
                *p.get(off + 1)?,
                *p.get(off + 2)?,
                *p.get(off + 3)?,
                *p.get(off + 4)?,
                *p.get(off + 5)?,
                *p.get(off + 6)?,
                *p.get(off + 7)?,
            ]))
        };

        match tag {
            1 => {
                let version = *p.first()?;
                let game_code = [*p.get(1)?, *p.get(2)?, *p.get(3)?, *p.get(4)?];
                let tlen = u16_at(5)? as usize;
                let end = 7usize.checked_add(tlen)?;
                let title_slice = p.get(7..end)?;
                let rom_title = String::from_utf8_lossy(title_slice).into_owned();
                Some(Self::Hello {
                    version,
                    game_code,
                    rom_title,
                })
            }
            2 => {
                let seq = u32_at(0)?;
                let mode = match *p.get(4)? {
                    0 => SioMode::Normal8,
                    1 => SioMode::Normal32,
                    2 => SioMode::Multi,
                    3 => SioMode::Uart,
                    _ => return None,
                };
                let siocnt = u16_at(5)?;
                let data8_or_send = u16_at(7)?;
                let data32 = u32_at(9)?;
                let duration_cycles = u64_at(13)?;
                Some(Self::SioTransfer {
                    seq,
                    transfer: SioTransfer {
                        mode,
                        siocnt,
                        data8_or_send,
                        data32,
                        duration_cycles,
                    },
                })
            }
            3 => {
                let seq = u32_at(0)?;
                let data8 = *p.get(4)?;
                let data32 = u32_at(5)?;
                let multi = [u16_at(9)?, u16_at(11)?, u16_at(13)?, u16_at(15)?];
                let multi_id = *p.get(17)?;
                let error = *p.get(18)? != 0;
                Some(Self::SioResponse {
                    seq,
                    response: SioResponse {
                        data8,
                        data32,
                        multi,
                        multi_id,
                        error,
                    },
                })
            }
            4 => {
                let frame = u64_at(0)?;
                let multi_ready = *p.get(8)? != 0;
                let multi_send = u16_at(9)?;
                let timestamp_ms = u32_at(11)?;
                Some(Self::FrameHeartbeat {
                    frame,
                    multi_ready,
                    multi_send,
                    timestamp_ms,
                })
            }
            5 => Some(Self::Pong {
                timestamp_ms: u32_at(0)?,
            }),
            6 => Some(Self::Disconnect),
            _ => None,
        }
    }
}

enum NetWorkerCmd {
    Send(NetPacket),
    Shutdown,
}

enum NetWorkerEvent {
    Listening(SocketAddr),
    Connected { peer_addr: String },
    Packet(NetPacket),
    Error(String),
    Closed(String),
}

struct PendingLocalNet {
    seq: u32,
    transfer: SioTransfer,
    started_at: Instant,
}

struct PendingRemoteNet {
    seq: u32,
    transfer: SioTransfer,
}

/// Non-blocking TCP Netplay bridge for GBA SIO transfers over LAN or localhost.
pub struct NetplayLink {
    pub role: NetplayRole,
    pub state: NetplayState,
    pub stats: SioStats,
    pub rtt_ms: Option<u32>,
    bound_addr: Option<SocketAddr>,
    cmd_tx: Sender<NetWorkerCmd>,
    evt_rx: Receiver<NetWorkerEvent>,
    worker: Option<JoinHandle<()>>,
    started_at: Instant,
    frame_counter: u64,
    next_seq: u32,
    local_pending: Option<PendingLocalNet>,
    remote_pending: Option<PendingRemoteNet>,
    remote_multi_state: (bool, u16),
    latest_remote_resp: Option<(u32, SioResponse)>,
}

impl NetplayLink {
    /// Starts hosting a TCP Link Cable server bound to `0.0.0.0:<port>`.
    pub fn start_host(port: u16, rom_title: String, game_code: [u8; 4]) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (evt_tx, evt_rx) = mpsc::channel();
        let hello = NetPacket::Hello {
            version: NETPLAY_PROTOCOL_VERSION,
            game_code,
            rom_title,
        };
        let worker = thread::Builder::new()
            .name("rba-netplay-host".into())
            .spawn(move || run_host_worker(port, hello, cmd_rx, evt_tx))
            .ok();

        Self {
            role: NetplayRole::Host,
            state: NetplayState::Connecting {
                target: format!("0.0.0.0:{port}"),
            },
            stats: SioStats::default(),
            rtt_ms: None,
            bound_addr: None,
            cmd_tx,
            evt_rx,
            worker,
            started_at: Instant::now(),
            frame_counter: 0,
            next_seq: 1,
            local_pending: None,
            remote_pending: None,
            remote_multi_state: (false, 0xFFFF),
            latest_remote_resp: None,
        }
    }

    /// Connects as a client (`multi_id = 1`) to a Netplay Host at `<addr_str>`.
    pub fn start_join(addr_str: String, rom_title: String, game_code: [u8; 4]) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (evt_tx, evt_rx) = mpsc::channel();
        let hello = NetPacket::Hello {
            version: NETPLAY_PROTOCOL_VERSION,
            game_code,
            rom_title,
        };
        let target_label = addr_str.clone();
        let worker = thread::Builder::new()
            .name("rba-netplay-join".into())
            .spawn(move || run_join_worker(addr_str, hello, cmd_rx, evt_tx))
            .ok();

        Self {
            role: NetplayRole::Join,
            state: NetplayState::Connecting {
                target: target_label,
            },
            stats: SioStats::default(),
            rtt_ms: None,
            bound_addr: None,
            cmd_tx,
            evt_rx,
            worker,
            started_at: Instant::now(),
            frame_counter: 0,
            next_seq: 1,
            local_pending: None,
            remote_pending: None,
            remote_multi_state: (false, 0xFFFF),
            latest_remote_resp: None,
        }
    }

    /// Returns `true` when the TCP socket is connected and ready for SIO transfers.
    pub fn is_connected(&self) -> bool {
        matches!(self.state, NetplayState::Connected { .. })
    }

    /// Returns `true` if a Host or Join worker thread is still alive (listening, connecting, or active).
    pub fn is_active(&self) -> bool {
        matches!(
            self.state,
            NetplayState::Listening { .. }
                | NetplayState::Connecting { .. }
                | NetplayState::Connected { .. }
        )
    }

    /// Returns the actual bound local socket address when hosting (useful when binding port `0`).
    #[cfg(test)]
    pub fn bound_addr(&self) -> Option<SocketAddr> {
        self.bound_addr
    }

    /// Polls background network events without blocking the UI thread.
    pub fn poll_events(&mut self) {
        while let Ok(evt) = self.evt_rx.try_recv() {
            match evt {
                NetWorkerEvent::Listening(addr) => {
                    self.bound_addr = Some(addr);
                    self.state = NetplayState::Listening { addr };
                }
                NetWorkerEvent::Connected { peer_addr } => {
                    self.state = NetplayState::Connected {
                        peer_addr,
                        peer_title: "Negotiating...".into(),
                        peer_code: "----".into(),
                    };
                }
                NetWorkerEvent::Packet(pkt) => self.handle_packet(pkt),
                NetWorkerEvent::Error(err) => {
                    self.state = NetplayState::Error(err);
                }
                NetWorkerEvent::Closed(reason) => {
                    if !matches!(self.state, NetplayState::Error(_)) {
                        self.state = NetplayState::Error(reason);
                    }
                }
            }
        }
    }

    fn handle_packet(&mut self, pkt: NetPacket) {
        match pkt {
            NetPacket::Hello {
                version,
                game_code,
                rom_title,
            } => {
                if version != NETPLAY_PROTOCOL_VERSION {
                    self.state = NetplayState::Error(format!(
                        "Protocol mismatch (peer v{version}, local v{NETPLAY_PROTOCOL_VERSION})"
                    ));
                    return;
                }
                let code_str = String::from_utf8_lossy(&game_code).into_owned();
                if let NetplayState::Connected { peer_addr, .. } = &self.state {
                    self.state = NetplayState::Connected {
                        peer_addr: peer_addr.clone(),
                        peer_title: rom_title,
                        peer_code: code_str,
                    };
                }
            }
            NetPacket::SioTransfer { seq, transfer } => {
                self.remote_pending = Some(PendingRemoteNet { seq, transfer });
            }
            NetPacket::SioResponse { seq, response } => {
                self.latest_remote_resp = Some((seq, response));
            }
            NetPacket::FrameHeartbeat {
                frame: _,
                multi_ready,
                multi_send,
                timestamp_ms,
            } => {
                self.remote_multi_state = (multi_ready, multi_send);
                let _ = self
                    .cmd_tx
                    .send(NetWorkerCmd::Send(NetPacket::Pong { timestamp_ms }));
            }
            NetPacket::Pong { timestamp_ms } => {
                let now_ms = self.started_at.elapsed().as_millis() as u32;
                self.rtt_ms = Some(now_ms.saturating_sub(timestamp_ms));
            }
            NetPacket::Disconnect => {
                self.state = NetplayState::Error("Peer disconnected".into());
            }
        }
    }

    /// Advances the local `GbaCore` by one frame while servicing TCP SIO packets.
    pub fn step_frame<F: HostFrontend + ?Sized>(&mut self, core: &mut GbaCore, fe: &mut F) {
        self.poll_events();
        let connected = self.is_connected();
        core.set_link_connected(connected);

        if !connected {
            core.run_frame(fe);
            return;
        }

        let kp = fe.poll_keypad() & 0x03FF;
        core.bus.hw.update_keypad(kp);

        self.frame_counter += 1;
        let (multi_ready, multi_send) = core_is_multi_ready(core);
        let now_ms = self.started_at.elapsed().as_millis() as u32;
        let _ = self
            .cmd_tx
            .send(NetWorkerCmd::Send(NetPacket::FrameHeartbeat {
                frame: self.frame_counter,
                multi_ready,
                multi_send,
                timestamp_ms: now_ms,
            }));

        let start = core.bus.scheduler.current_cycle;
        let mut offset = 0u64;
        while offset < CYCLES_PER_FRAME {
            offset = (offset + NETPLAY_CHUNK_CYCLES).min(CYCLES_PER_FRAME);
            if let Some(fb) = step_core_until(core, start + offset) {
                fe.present_frame(&fb);
            }

            self.poll_events();
            if self.local_pending.is_none()
                && let Some(xfer) = core.take_pending_sio_transfer()
            {
                let seq = self.next_seq;
                self.next_seq = self.next_seq.wrapping_add(1).max(1);
                let _ = self.cmd_tx.send(NetWorkerCmd::Send(NetPacket::SioTransfer {
                    seq,
                    transfer: xfer,
                }));
                self.local_pending = Some(PendingLocalNet {
                    seq,
                    transfer: xfer,
                    started_at: Instant::now(),
                });
            }

            self.resolve_net_sio(core);

            // Brief non-blocking wait on localhost/LAN when an active transfer awaits peer reply.
            if self.local_pending.is_some() {
                let wait_until = Instant::now() + Duration::from_millis(2);
                while self.local_pending.is_some() && Instant::now() < wait_until {
                    self.poll_events();
                    self.resolve_net_sio(core);
                    if self.local_pending.is_some() {
                        thread::sleep(Duration::from_micros(250));
                    }
                }
            }
        }

        flush_core_to_frontend(core, fe);
    }

    fn resolve_net_sio(&mut self, core: &mut GbaCore) {
        match self.role {
            NetplayRole::Host => {
                if let (Some(loc), Some(rem)) =
                    (self.local_pending.as_ref(), self.remote_pending.as_ref())
                    && let Some((resp_host, resp_join, mode)) =
                        pair_responses(loc.transfer, rem.transfer)
                {
                    let rem_seq = rem.seq;
                    core.complete_sio_transfer(resp_host);
                    let _ = self.cmd_tx.send(NetWorkerCmd::Send(NetPacket::SioResponse {
                        seq: rem_seq,
                        response: resp_join,
                    }));
                    self.local_pending = None;
                    self.remote_pending = None;
                    self.stats.record_mode(mode);
                    return;
                }

                // Multi Master -> Join Slave without Join SIOCNT.Start bit.
                if let Some(loc) = self.local_pending.as_ref()
                    && loc.transfer.mode == SioMode::Multi
                    && self.remote_pending.is_none()
                    && self.remote_multi_state.0
                {
                    let peer_send = self.remote_multi_state.1;
                    let multi = [loc.transfer.data8_or_send, peer_send, 0xFFFF, 0xFFFF];
                    core.complete_sio_transfer(SioResponse {
                        data8: peer_send as u8,
                        data32: 0,
                        multi,
                        multi_id: 0,
                        error: false,
                    });
                    let _ = self.cmd_tx.send(NetWorkerCmd::Send(NetPacket::SioResponse {
                        seq: u32::MAX,
                        response: SioResponse {
                            data8: loc.transfer.data8_or_send as u8,
                            data32: 0,
                            multi,
                            multi_id: 1,
                            error: false,
                        },
                    }));
                    self.local_pending = None;
                    self.stats.multi += 1;
                    return;
                }
            }
            NetplayRole::Join => {
                if let Some((resp_seq, resp)) = self.latest_remote_resp.take() {
                    if let Some(loc) = self.local_pending.as_ref()
                        && (resp_seq == loc.seq || resp_seq == u32::MAX)
                    {
                        let mode = loc.transfer.mode;
                        core.complete_sio_transfer(resp);
                        self.local_pending = None;
                        self.stats.record_mode(mode);
                        return;
                    } else if resp_seq == u32::MAX {
                        core.complete_sio_transfer(resp);
                        self.stats.multi += 1;
                        return;
                    }
                }
            }
        }

        if let Some(loc) = self.local_pending.as_ref()
            && loc.transfer.duration_cycles > 0
            && loc.started_at.elapsed() >= NETPLAY_SIO_TIMEOUT
        {
            let id = match self.role {
                NetplayRole::Host => 0,
                NetplayRole::Join => 1,
            };
            core.complete_sio_transfer(build_open_bus_timeout_response(loc.transfer, id));
            self.local_pending = None;
            self.stats.timeouts += 1;
        }
    }

    /// Gracefully shuts down the TCP socket and joins the background thread.
    pub fn shutdown(mut self) {
        let _ = self.cmd_tx.send(NetWorkerCmd::Send(NetPacket::Disconnect));
        let _ = self.cmd_tx.send(NetWorkerCmd::Shutdown);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

fn configure_stream(stream: &TcpStream) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_nonblocking(true);
}

fn run_host_worker(
    port: u16,
    hello: NetPacket,
    cmd_rx: Receiver<NetWorkerCmd>,
    evt_tx: Sender<NetWorkerEvent>,
) {
    let listener = match TcpListener::bind(("0.0.0.0", port)) {
        Ok(l) => l,
        Err(e) => {
            let _ = evt_tx.send(NetWorkerEvent::Error(format!(
                "Failed binding port {port}: {e}"
            )));
            return;
        }
    };
    let _ = listener.set_nonblocking(true);
    if let Ok(local_addr) = listener.local_addr() {
        let _ = evt_tx.send(NetWorkerEvent::Listening(local_addr));
    }

    let mut stream: Option<TcpStream> = None;
    let mut read_buf = Vec::<u8>::with_capacity(1024);

    loop {
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                NetWorkerCmd::Shutdown => return,
                NetWorkerCmd::Send(pkt) => {
                    if let Some(s) = &mut stream
                        && let Err(e) = write_packet(s, &pkt)
                    {
                        let _ = evt_tx.send(NetWorkerEvent::Closed(format!("Send failed: {e}")));
                        return;
                    }
                }
            }
        }

        if stream.is_none() {
            match listener.accept() {
                Ok((s, addr)) => {
                    configure_stream(&s);
                    let mut s = s;
                    if let Err(e) = write_packet(&mut s, &hello) {
                        let _ = evt_tx.send(NetWorkerEvent::Closed(format!(
                            "Handshake send failed: {e}"
                        )));
                        return;
                    }
                    let _ = evt_tx.send(NetWorkerEvent::Connected {
                        peer_addr: addr.to_string(),
                    });
                    stream = Some(s);
                }
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                }
                Err(e) => {
                    let _ = evt_tx.send(NetWorkerEvent::Error(format!("Accept error: {e}")));
                    return;
                }
            }
        }

        if let Some(s) = &mut stream {
            match pump_stream_reads(s, &mut read_buf, &evt_tx) {
                Ok(true) => {}
                Ok(false) => thread::sleep(Duration::from_millis(1)),
                Err(reason) => {
                    let _ = evt_tx.send(NetWorkerEvent::Closed(reason));
                    return;
                }
            }
        }
    }
}

fn run_join_worker(
    addr_str: String,
    hello: NetPacket,
    cmd_rx: Receiver<NetWorkerCmd>,
    evt_tx: Sender<NetWorkerEvent>,
) {
    let resolved = match addr_str.to_socket_addrs() {
        Ok(mut iter) => match iter.next() {
            Some(sa) => sa,
            None => {
                let _ = evt_tx.send(NetWorkerEvent::Error(format!(
                    "Could not resolve '{addr_str}'"
                )));
                return;
            }
        },
        Err(e) => {
            let _ = evt_tx.send(NetWorkerEvent::Error(format!(
                "Invalid address '{addr_str}': {e}"
            )));
            return;
        }
    };

    let mut stream = match TcpStream::connect_timeout(&resolved, Duration::from_secs(4)) {
        Ok(s) => s,
        Err(e) => {
            let _ = evt_tx.send(NetWorkerEvent::Error(format!(
                "Connection to {resolved} failed: {e}"
            )));
            return;
        }
    };
    configure_stream(&stream);
    if let Err(e) = write_packet(&mut stream, &hello) {
        let _ = evt_tx.send(NetWorkerEvent::Closed(format!(
            "Handshake send failed: {e}"
        )));
        return;
    }
    let _ = evt_tx.send(NetWorkerEvent::Connected {
        peer_addr: resolved.to_string(),
    });

    let mut read_buf = Vec::<u8>::with_capacity(1024);
    loop {
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                NetWorkerCmd::Shutdown => return,
                NetWorkerCmd::Send(pkt) => {
                    if let Err(e) = write_packet(&mut stream, &pkt) {
                        let _ = evt_tx.send(NetWorkerEvent::Closed(format!("Send failed: {e}")));
                        return;
                    }
                }
            }
        }

        match pump_stream_reads(&mut stream, &mut read_buf, &evt_tx) {
            Ok(true) => {}
            Ok(false) => thread::sleep(Duration::from_millis(1)),
            Err(reason) => {
                let _ = evt_tx.send(NetWorkerEvent::Closed(reason));
                return;
            }
        }
    }
}

fn write_packet(stream: &mut TcpStream, pkt: &NetPacket) -> std::io::Result<()> {
    let bytes = pkt.encode();
    let mut written = 0;
    let deadline = Instant::now() + Duration::from_millis(50);
    while written < bytes.len() {
        match stream.write(&bytes[written..]) {
            Ok(0) => return Err(std::io::Error::new(ErrorKind::WriteZero, "socket closed")),
            Ok(n) => written += n,
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(std::io::Error::new(
                        ErrorKind::TimedOut,
                        "write buffer full",
                    ));
                }
                thread::sleep(Duration::from_micros(200));
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn pump_stream_reads(
    stream: &mut TcpStream,
    buf: &mut Vec<u8>,
    evt_tx: &Sender<NetWorkerEvent>,
) -> Result<bool, String> {
    let mut got_data = false;
    let mut tmp = [0u8; 4096];
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => return Err("Peer closed TCP connection".into()),
            Ok(n) => {
                got_data = true;
                buf.extend_from_slice(&tmp[..n]);
            }
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(ref e) if e.kind() == ErrorKind::ConnectionReset => {
                return Err("Connection reset by peer".into());
            }
            Err(e) => return Err(format!("Socket read error: {e}")),
        }
    }

    while buf.len() >= 4 {
        let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        if len == 0 || len > MAX_FRAME_PACKET_BYTES {
            return Err(format!("Corrupt frame length ({len} bytes)"));
        }
        if buf.len() < 4 + len {
            break;
        }
        let body: Vec<u8> = buf.drain(..4 + len).skip(4).collect();
        if let Some(pkt) = NetPacket::decode(&body) {
            let _ = evt_tx.send(NetWorkerEvent::Packet(pkt));
        }
    }
    Ok(got_data)
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum LinkUiTab {
    Local,
    Netplay,
}

/// Frontend Link Cable coordinator owning active Local or TCP Netplay sessions and UI state.
pub struct LinkManager {
    pub show_window: bool,
    pub(crate) active_tab: LinkUiTab,
    pub local: Option<LocalLinkSession>,
    pub netplay: Option<NetplayLink>,
    pub host_port_input: String,
    pub join_addr_input: String,
    pub ui_error: Option<String>,
}

impl Default for LinkManager {
    fn default() -> Self {
        Self::new()
    }
}

impl LinkManager {
    /// Creates an idle `LinkManager` with default port `27888`.
    pub fn new() -> Self {
        Self {
            show_window: false,
            active_tab: LinkUiTab::Local,
            local: None,
            netplay: None,
            host_port_input: DEFAULT_NETPLAY_PORT.to_string(),
            join_addr_input: format!("127.0.0.1:{DEFAULT_NETPLAY_PORT}"),
            ui_error: None,
        }
    }

    /// Returns `true` if a Local 2-Player Link session is active.
    pub fn local_is_active(&self) -> bool {
        self.local.is_some()
    }

    /// Returns `true` if Local Link is active AND split-screen viewport is enabled.
    pub fn split_view_active(&self) -> bool {
        self.local.as_ref().is_some_and(|l| l.split_view)
    }

    /// Returns a concise status badge string for the status bar.
    #[allow(dead_code)]
    pub fn status_badge(&self) -> Option<String> {
        if let Some(local) = &self.local {
            return Some(format!("LINK LOCAL 2P • {} xfers", local.stats.total()));
        }
        if let Some(net) = &self.netplay {
            return match &net.state {
                NetplayState::Listening { addr } => Some(format!("LINK WAIT :{}", addr.port())),
                NetplayState::Connecting { target } => Some(format!("LINK DIAL {target}")),
                NetplayState::Connected { peer_addr, .. } => {
                    let rtt = net.rtt_ms.map(|ms| format!(" {ms}ms")).unwrap_or_default();
                    Some(format!("LINK TCP {peer_addr}{rtt}"))
                }
                NetplayState::Error(_) => Some("LINK ERR".into()),
            };
        }
        None
    }

    /// Disconnects any active Local or Netplay link cleanly.
    pub fn disconnect_all(&mut self) {
        if let Some(mut local) = self.local.take() {
            local.flush_p2_battery_save();
        }
        if let Some(net) = self.netplay.take() {
            net.shutdown();
        }
    }
}

impl RustboyApp {
    /// Launches a local Player 2 `GbaCore` from the given ROM path and attaches it as Slave.
    pub(crate) fn start_local_link_with_rom_path(
        &mut self,
        p2_rom_path: &Path,
    ) -> Result<(), String> {
        if self.core.is_none() {
            return Err("Load a GBA ROM for Player 1 before connecting Player 2".into());
        }
        if let Some(net) = self.link.netplay.take() {
            net.shutdown();
        }

        let loaded = load_rom_bytes(p2_rom_path)?;
        if detect_rom_platform(p2_rom_path, loaded.inner_name.as_deref(), &loaded.bytes)
            != RomPlatform::Gba
        {
            return Err("Player 2 Link Cable requires a .gba ROM".into());
        }

        let sav_path = p2_rom_path.with_extension("sav");
        let same_path_as_p1 = self.rom_path.as_ref().is_some_and(|p1p| p1p == p2_rom_path);

        let mut p2_core = GbaCore::new(self.bios_bytes.as_deref(), loaded.bytes);
        if let Ok(sav) = std::fs::read(&sav_path) {
            p2_core.bus.cart.load_save_data(&sav);
        }
        if let Some(p1_core) = &mut self.core {
            p1_core.set_link_connected(true);
        }

        let session = LocalLinkSession::new(
            p2_core,
            Some(p2_rom_path.to_path_buf()),
            Some(sav_path),
            !same_path_as_p1,
        );
        let p2_title = session.p2_rom_title.clone();
        self.link.local = Some(session);
        self.link.ui_error = None;
        self.notify(&format!("Link Cable connected: Player 2 [{p2_title}]"));
        Ok(())
    }

    /// Renders the GBA Link Cable & TCP Netplay modal/floating control window.
    pub(crate) fn draw_link_window(&mut self, ctx: &egui::Context) {
        if let Some(net) = &mut self.link.netplay {
            net.poll_events();
        }
        if !self.link.show_window {
            return;
        }

        let mut open = self.link.show_window;
        let has_gba = self.core.is_some();
        let has_gb = self.gb.is_some();

        egui::Window::new("GBA Link Cable / Netplay")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .min_width(470.0)
            .frame(
                egui::Frame::window(&ctx.style())
                    .fill(BG_PANEL)
                    .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(16.0),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let is_local = self.link.active_tab == LinkUiTab::Local;
                    let is_net = self.link.active_tab == LinkUiTab::Netplay;
                    if ui
                        .selectable_label(is_local, "Local 2-Player Link")
                        .clicked()
                    {
                        self.link.active_tab = LinkUiTab::Local;
                    }
                    if ui
                        .selectable_label(is_net, "Network Link (TCP)")
                        .clicked()
                    {
                        self.link.active_tab = LinkUiTab::Netplay;
                    }
                });
                ui.separator();
                ui.add_space(6.0);

                if has_gb {
                    render_notice_banner(
                        ui,
                        "A Game Boy / GBC ROM is currently active. Load a GBA ROM to enable GBA Link Cable.",
                        false,
                    );
                    ui.add_space(6.0);
                } else if !has_gba {
                    render_notice_banner(
                        ui,
                        "No active GBA game. Launch a .gba ROM first before starting a Link session.",
                        false,
                    );
                    ui.add_space(6.0);
                }

                if let Some(err) = self.link.ui_error.clone() {
                    egui::Frame::new()
                        .fill(Color32::from_rgb(56, 24, 28))
                        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(175, 64, 68)))
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(8.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(err)
                                        .small()
                                        .color(Color32::from_rgb(255, 165, 165)),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.button(RichText::new("Dismiss").small()).clicked() {
                                            self.link.ui_error = None;
                                        }
                                    },
                                );
                            });
                        });
                    ui.add_space(6.0);
                }

                match self.link.active_tab {
                    LinkUiTab::Local => self.draw_link_local_tab(ui, has_gba),
                    LinkUiTab::Netplay => self.draw_link_netplay_tab(ui, has_gba),
                }
            });

        self.link.show_window = open;
    }

    fn draw_link_local_tab(&mut self, ui: &mut egui::Ui, has_gba: bool) {
        if self.link.netplay.is_some() {
            render_notice_banner(
                ui,
                "TCP Netplay is currently active. Disconnect Netplay before starting Local 2-Player Link.",
                true,
            );
            ui.add_space(8.0);
            if ui.button("Disconnect TCP Netplay").clicked() {
                if let Some(net) = self.link.netplay.take() {
                    net.shutdown();
                }
                if let Some(core) = &mut self.core {
                    core.set_link_connected(false);
                }
            }
            return;
        }

        if let Some(local) = &mut self.link.local {
            let mut disconnect = false;
            let mut reset_p2 = false;

            egui::Frame::new()
                .fill(BG_CARD)
                .stroke(Stroke::new(1.0_f32, ACCENT_TEAL))
                .corner_radius(CornerRadius::same(4))
                .inner_margin(12.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("CONNECTED • LOCAL 2-PLAYER")
                                .small()
                                .strong()
                                .color(ACCENT_TEAL),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.monospace(
                                RichText::new(format!(
                                    "P1 Master / P2 Slave [{}]",
                                    local.p2_rom_code
                                ))
                                .small()
                                .color(TEXT_MUTED),
                            );
                        });
                    });
                    ui.add_space(4.0);
                    let cart_lbl = ui.label(
                        RichText::new(format!("Player 2 Cartridge: {}", local.p2_rom_title))
                            .strong(),
                    );
                    if let Some(p2p) = &local.p2_rom_path {
                        cart_lbl.on_hover_text(p2p.display().to_string());
                    }
                    ui.add_space(6.0);
                    ui.checkbox(
                        &mut local.split_view,
                        "Show Side-by-Side Split-Screen Viewport (P1 | P2)",
                    );
                });

            ui.add_space(8.0);
            render_sio_stats_card(ui, &local.stats);
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                if ui.button("Reset Player 2").clicked() {
                    reset_p2 = true;
                }
                if ui.button("Disconnect Local Link").clicked() {
                    disconnect = true;
                }
            });

            if reset_p2 {
                local.p2_core.reset();
                local.p2_core.set_link_connected(true);
                self.notify("Reset Player 2 GBA instance");
            }
            if disconnect {
                if let Some(mut dead) = self.link.local.take() {
                    dead.flush_p2_battery_save();
                }
                if let Some(core) = &mut self.core {
                    core.set_link_connected(false);
                }
                self.notify("Local 2-Player Link disconnected");
            }
        } else {
            let mut pick_current = false;
            let mut pick_file = false;

            egui::Frame::new()
                .fill(BG_CARD)
                .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
                .corner_radius(CornerRadius::same(4))
                .inner_margin(12.0)
                .show(ui, |ui| {
                    ui.label(RichText::new("Idle — Player 2 Not Connected").strong());
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Run two GBA cores in lockstep inside one window for Pokemon trading, Advance Wars, or Mario Kart.",
                        )
                        .small()
                        .color(TEXT_MUTED),
                    );
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        let can_same = has_gba && self.rom_path.is_some();
                        if ui
                            .add_enabled(
                                can_same,
                                egui::Button::new("Use Current ROM for P2"),
                            )
                            .clicked()
                        {
                            pick_current = true;
                        }
                        if ui
                            .add_enabled(has_gba, egui::Button::new("Choose P2 ROM..."))
                            .clicked()
                        {
                            pick_file = true;
                        }
                    });
                });

            ui.add_space(8.0);
            egui::Frame::new()
                .fill(BG_CARD)
                .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
                .corner_radius(CornerRadius::same(4))
                .inner_margin(10.0)
                .show(ui, |ui| {
                    ui.label(RichText::new("Player 2 Controls").small().strong());
                    ui.label(
                        RichText::new(
                            "Keyboard: I/J/K/L = D-Pad  |  O = A  |  U = B  |  N/M = Start/Select  |  7/8 = L/R\nGamepad: 2nd connected controller automatically maps to Player 2.",
                        )
                        .small()
                        .color(TEXT_MUTED),
                    );
                });

            if pick_current
                && let Some(p1_path) = self.rom_path.clone()
                && let Err(e) = self.start_local_link_with_rom_path(&p1_path)
            {
                self.link.ui_error = Some(e);
            }
            if pick_file
                && let Some(path) = rfd::FileDialog::new()
                    .set_title("Select Player 2 GBA ROM")
                    .add_filter("GBA ROMs", &["gba", "agb", "bin", "zip", "7z"])
                    .pick_file()
                && let Err(e) = self.start_local_link_with_rom_path(&path)
            {
                self.link.ui_error = Some(e);
            }
        }
    }

    fn draw_link_netplay_tab(&mut self, ui: &mut egui::Ui, has_gba: bool) {
        if self.link.local.is_some() {
            render_notice_banner(
                ui,
                "Local 2-Player Link is currently active. Disconnect it before hosting or joining TCP Netplay.",
                true,
            );
            return;
        }

        if let Some(net) = &self.link.netplay {
            let state_clone = net.state.clone();
            let stats_copy = net.stats;
            let rtt = net.rtt_ms;
            let role = net.role;
            let mut disconnect = false;

            let (badge, border_col, subtitle) = match &state_clone {
                NetplayState::Listening { addr } => (
                    format!("LISTENING ON 0.0.0.0:{}", addr.port()),
                    Color32::from_rgb(235, 185, 85),
                    "Waiting for remote Join client to connect over TCP...".to_string(),
                ),
                NetplayState::Connecting { target } => (
                    format!("CONNECTING TO {target}"),
                    Color32::from_rgb(235, 185, 85),
                    "Opening TCP connection and exchanging handshake...".to_string(),
                ),
                NetplayState::Connected {
                    peer_addr,
                    peer_title,
                    peer_code,
                } => {
                    let rtt_str = rtt.map(|ms| format!(" • RTT {ms} ms")).unwrap_or_default();
                    (
                        format!(
                            "CONNECTED ({}){rtt_str}",
                            match role {
                                NetplayRole::Host => "Host / P1",
                                NetplayRole::Join => "Client / P2",
                            }
                        ),
                        ACCENT_TEAL,
                        format!("Peer {peer_addr} — ROM: {peer_title} [{peer_code}]"),
                    )
                }
                NetplayState::Error(err) => (
                    "DISCONNECTED (ERROR)".to_string(),
                    Color32::from_rgb(200, 75, 75),
                    err.clone(),
                ),
            };

            egui::Frame::new()
                .fill(BG_CARD)
                .stroke(Stroke::new(1.0_f32, border_col))
                .corner_radius(CornerRadius::same(4))
                .inner_margin(12.0)
                .show(ui, |ui| {
                    ui.label(RichText::new(badge).small().strong().color(border_col));
                    ui.add_space(4.0);
                    ui.label(RichText::new(subtitle).color(TEXT_MUTED));
                });

            ui.add_space(8.0);
            render_sio_stats_card(ui, &stats_copy);
            ui.add_space(8.0);

            if ui.button("Stop / Disconnect Netplay").clicked() {
                disconnect = true;
            }
            if disconnect {
                if let Some(dead) = self.link.netplay.take() {
                    dead.shutdown();
                }
                if let Some(core) = &mut self.core {
                    core.set_link_connected(false);
                }
                self.notify("TCP Netplay disconnected");
            }
            return;
        }

        let mut start_host = false;
        let mut start_join = false;

        egui::Frame::new()
            .fill(BG_CARD)
            .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.label(RichText::new("Host Session (Player 1 / Master)").strong());
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Listen Port:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.link.host_port_input)
                            .desired_width(80.0),
                    );
                    if ui
                        .add_enabled(has_gba, egui::Button::new("Start Hosting (0.0.0.0)"))
                        .clicked()
                    {
                        start_host = true;
                    }
                });
            });

        ui.add_space(8.0);

        egui::Frame::new()
            .fill(BG_CARD)
            .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.label(RichText::new("Join Session (Player 2 / Slave)").strong());
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Host IP:Port:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.link.join_addr_input)
                            .desired_width(180.0),
                    );
                    if ui
                        .add_enabled(has_gba, egui::Button::new("Connect to Host"))
                        .clicked()
                    {
                        start_join = true;
                    }
                });
            });

        if start_host {
            match self.link.host_port_input.trim().parse::<u16>() {
                Ok(port) => {
                    let code = game_code_bytes(&self.rom_code);
                    self.link.netplay =
                        Some(NetplayLink::start_host(port, self.rom_title.clone(), code));
                    self.link.ui_error = None;
                    self.notify(&format!("Hosting TCP Link Cable on port {port}"));
                }
                Err(_) => {
                    self.link.ui_error = Some("Enter a valid TCP port (1..=65535)".into());
                }
            }
        } else if start_join {
            let target = self.link.join_addr_input.trim().to_string();
            if target.is_empty() {
                self.link.ui_error = Some("Enter a host IP:port (e.g. 127.0.0.1:27888)".into());
            } else {
                let normalized = if target.contains(':') {
                    target
                } else {
                    format!("{target}:{DEFAULT_NETPLAY_PORT}")
                };
                let code = game_code_bytes(&self.rom_code);
                self.link.netplay = Some(NetplayLink::start_join(
                    normalized.clone(),
                    self.rom_title.clone(),
                    code,
                ));
                self.link.ui_error = None;
                self.notify(&format!("Connecting TCP Link Cable to {normalized}"));
            }
        }
    }
}

fn game_code_bytes(code: &str) -> [u8; 4] {
    let mut out = *b"----";
    for (i, b) in code.as_bytes().iter().take(4).enumerate() {
        out[i] = *b;
    }
    out
}

fn render_notice_banner(ui: &mut egui::Ui, msg: &str, warn: bool) {
    let stroke = if warn {
        Color32::from_rgb(200, 150, 60)
    } else {
        BORDER_HAIRLINE
    };
    egui::Frame::new()
        .fill(BG_CARD)
        .stroke(Stroke::new(1.0_f32, stroke))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.label(RichText::new(msg).small().color(TEXT_MUTED));
        });
}

fn render_sio_stats_card(ui: &mut egui::Ui, stats: &SioStats) {
    egui::Frame::new()
        .fill(BG_CARD)
        .stroke(Stroke::new(1.0_f32, BORDER_HAIRLINE))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.label(RichText::new("SIO Transfer Telemetry").small().strong());
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.monospace(RichText::new(format!("Normal8: {}", stats.normal8)).small());
                ui.separator();
                ui.monospace(RichText::new(format!("Normal32: {}", stats.normal32)).small());
                ui.separator();
                ui.monospace(RichText::new(format!("Multi: {}", stats.multi)).small());
                ui.separator();
                ui.monospace(
                    RichText::new(format!("Timeouts: {}", stats.timeouts))
                        .small()
                        .color(TEXT_MUTED),
                );
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyFe;
    impl HostFrontend for DummyFe {
        fn present_frame(&mut self, _fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {}
        fn push_audio_samples(&mut self, _samples: &[i16]) {}
        fn poll_keypad(&mut self) -> u16 {
            0x03FF
        }
    }

    struct ArmAsm {
        words: Vec<u32>,
        lits: Vec<(usize, u32)>,
    }

    impl ArmAsm {
        fn new() -> Self {
            Self {
                words: Vec::new(),
                lits: Vec::new(),
            }
        }

        fn label(&self) -> usize {
            self.words.len()
        }

        fn mov_imm(&mut self, rd: u32, imm8: u8) {
            self.words.push(0xE3A0_0000 | (rd << 12) | u32::from(imm8));
        }

        fn add_imm(&mut self, rd: u32, rn: u32, imm8: u8) {
            self.words
                .push(0xE280_0000 | (rn << 16) | (rd << 12) | u32::from(imm8));
        }

        fn ldr_lit(&mut self, rd: u32, val: u32) {
            let idx = self.words.len();
            self.lits.push((idx, val));
            self.words.push(0xE59F_0000 | (rd << 12));
        }

        fn ldr(&mut self, rd: u32, rn: u32, off: u16) {
            self.words
                .push(0xE590_0000 | (rn << 16) | (rd << 12) | u32::from(off));
        }

        fn str(&mut self, rd: u32, rn: u32, off: u16) {
            self.words
                .push(0xE580_0000 | (rn << 16) | (rd << 12) | u32::from(off));
        }

        fn ldrb(&mut self, rd: u32, rn: u32, off: u16) {
            self.words
                .push(0xE5D0_0000 | (rn << 16) | (rd << 12) | u32::from(off));
        }

        fn strb(&mut self, rd: u32, rn: u32, off: u16) {
            self.words
                .push(0xE5C0_0000 | (rn << 16) | (rd << 12) | u32::from(off));
        }

        fn ldrh(&mut self, rd: u32, rn: u32, imm8: u8) {
            let imm_hi = u32::from(imm8 >> 4) << 8;
            let imm_lo = u32::from(imm8 & 0x0F);
            self.words
                .push(0xE1D0_00B0 | (rn << 16) | (rd << 12) | imm_hi | imm_lo);
        }

        fn strh(&mut self, rd: u32, rn: u32, imm8: u8) {
            let imm_hi = u32::from(imm8 >> 4) << 8;
            let imm_lo = u32::from(imm8 & 0x0F);
            self.words
                .push(0xE1C0_00B0 | (rn << 16) | (rd << 12) | imm_hi | imm_lo);
        }

        fn tst_imm(&mut self, rn: u32, imm8: u8) {
            self.words.push(0xE310_0000 | (rn << 16) | u32::from(imm8));
        }

        fn beq(&mut self, target: usize) {
            let cur = self.words.len() as isize;
            let off = (target as isize - cur - 2) as u32 & 0x00FF_FFFF;
            self.words.push(0x0A00_0000 | off);
        }

        fn emit_wait_irq_serial(&mut self) {
            // Wait until IF (R5 + 2) has bit 7 (IRQ_SERIAL) set, then clear IF bit 7
            // and increment irq counter R7.
            let poll = self.label();
            self.ldrh(0, 5, 2);
            self.tst_imm(0, 0x80);
            self.beq(poll);
            self.mov_imm(0, 0x80);
            self.strh(0, 5, 2);
            self.add_imm(7, 7, 1);
            self.str(7, 6, 0x18);
        }

        fn finish(mut self) -> Vec<u8> {
            self.words.push(0xEAFF_FFFE); // B .
            for (instr_idx, lit_val) in self.lits {
                let lit_idx = self.words.len();
                let byte_off = ((lit_idx - instr_idx - 2) * 4) as u32;
                self.words[instr_idx] |= byte_off & 0x0FFF;
                self.words.push(lit_val);
            }
            let mut rom = Vec::with_capacity((self.words.len() * 4).max(256));
            for w in self.words {
                rom.extend_from_slice(&w.to_le_bytes());
            }
            if rom.len() < 256 {
                rom.resize(256, 0);
            }
            rom
        }
    }

    fn build_sio_test_rom(is_master: bool, val8: u8, val32: u32, val_multi: u16) -> Vec<u8> {
        let mut a = ArmAsm::new();
        a.ldr_lit(4, 0x0400_0120); // R4 = SIO base
        a.ldr_lit(5, 0x0400_0200); // R5 = IE/IF base
        a.ldr_lit(6, 0x0300_0100); // R6 = IWRAM result array
        a.mov_imm(7, 0); // R7 = IRQ_SERIAL count

        // Leave RCNT GPIO off (bit 15 = 0) and enable IE bit 7 (IRQ_SERIAL).
        a.mov_imm(0, 0);
        a.strh(0, 4, 0x14); // RCNT = 0
        a.ldr_lit(0, 0xFFFF);
        a.strh(0, 5, 2); // clear IF
        a.mov_imm(0, 0x80);
        a.strh(0, 5, 0); // IE = IRQ_SERIAL

        // 1) Normal 8-bit exchange
        a.mov_imm(0, val8);
        a.strb(0, 4, 0x0A); // SIODATA8
        a.ldr_lit(0, if is_master { 0x4081 } else { 0x4080 });
        a.strh(0, 4, 0x08); // SIOCNT Normal8 + Start + IRQ
        a.emit_wait_irq_serial();
        a.ldrb(0, 4, 0x0A);
        a.str(0, 6, 0x00); // save Normal8 peer byte at 0x0300_0100

        // 2) Normal 32-bit exchange
        a.ldr_lit(0, val32);
        a.str(0, 4, 0x00); // SIODATA32
        a.ldr_lit(0, if is_master { 0x5081 } else { 0x5080 });
        a.strh(0, 4, 0x08); // SIOCNT Normal32 + Start + IRQ
        a.emit_wait_irq_serial();
        a.ldr(0, 4, 0x00);
        a.str(0, 6, 0x04); // save Normal32 peer word at 0x0300_0104

        // 3) Multi-player exchange (Master triggers Start, Slave leaves Start=0)
        a.ldr_lit(0, u32::from(val_multi));
        a.strh(0, 4, 0x0A); // SIOMLT_SEND
        a.ldr_lit(0, if is_master { 0x6083 } else { 0x6003 });
        a.strh(0, 4, 0x08); // SIOCNT Multi (115200 baud + IRQ)
        a.emit_wait_irq_serial();
        a.ldr(0, 4, 0x00); // SIOMULTI0 (lo16) + SIOMULTI1 (hi16)
        a.str(0, 6, 0x08);
        a.ldr(0, 4, 0x04); // SIOMULTI2 (lo16) + SIOMULTI3 (hi16)
        a.str(0, 6, 0x0C);
        a.ldrh(0, 4, 0x08); // SIOCNT (includes multi_id bits [5:4])
        a.str(0, 6, 0x10);

        a.finish()
    }

    #[test]
    fn test_local_link_normal8_normal32_and_multi_with_synthetic_arm_roms() {
        let rom_p1 = build_sio_test_rom(true, 0xA5, 0x1234_5678, 0xCAFE);
        let rom_p2 = build_sio_test_rom(false, 0x5A, 0xDEAD_BEEF, 0xBEEF);

        let mut p1 = GbaCore::new(None, rom_p1);
        let p2 = GbaCore::new(None, rom_p2);
        let mut session = LocalLinkSession::new(p2, None, None, false);
        let mut fe = DummyFe;

        for _ in 0..8 {
            session.step_frame(&mut p1, &mut fe, 0x03FF);
        }

        // Normal8 swapped values
        assert_eq!(p1.peek32(0x0300_0100), 0x5A);
        assert_eq!(session.p2_core.peek32(0x0300_0100), 0xA5);

        // Normal32 swapped values
        assert_eq!(p1.peek32(0x0300_0104), 0xDEAD_BEEF);
        assert_eq!(session.p2_core.peek32(0x0300_0104), 0x1234_5678);

        // Multi slots [P1, P2, 0xFFFF, 0xFFFF] and multi_id (P1=0, P2=1)
        let expected_m01 = 0xBEEF_CAFEu32;
        let expected_m23 = 0xFFFF_FFFFu32;
        assert_eq!(p1.peek32(0x0300_0108), expected_m01);
        assert_eq!(p1.peek32(0x0300_010C), expected_m23);
        assert_eq!(session.p2_core.peek32(0x0300_0108), expected_m01);
        assert_eq!(session.p2_core.peek32(0x0300_010C), expected_m23);
        assert_eq!((p1.peek32(0x0300_0110) >> 4) & 0x03, 0);
        assert_eq!((session.p2_core.peek32(0x0300_0110) >> 4) & 0x03, 1);

        // All 3 IRQ_SERIAL completions observed cleanly by both ARM CPUs
        assert_eq!(p1.peek32(0x0300_0118), 3);
        assert_eq!(session.p2_core.peek32(0x0300_0118), 3);
        assert_eq!(
            session.stats,
            SioStats {
                normal8: 1,
                normal32: 1,
                multi: 1,
                uart: 0,
                timeouts: 0,
            }
        );
    }

    #[test]
    fn test_netplay_tcp_loopback_handshake_and_sio_exchange() {
        let mut host = NetplayLink::start_host(0, "HOSTROM".into(), *b"TST1");
        let wait_listen = Instant::now();
        while host.bound_addr().is_none() && wait_listen.elapsed() < Duration::from_secs(2) {
            host.poll_events();
            thread::sleep(Duration::from_millis(5));
        }
        let port = host.bound_addr().expect("host bound addr").port();

        let mut join =
            NetplayLink::start_join(format!("127.0.0.1:{port}"), "JOINROM".into(), *b"TST1");

        let wait_conn = Instant::now();
        while (!host.is_connected() || !join.is_connected())
            && wait_conn.elapsed() < Duration::from_secs(3)
        {
            host.poll_events();
            join.poll_events();
            thread::sleep(Duration::from_millis(5));
        }
        assert!(host.is_connected(), "host failed to connect");
        assert!(join.is_connected(), "join failed to connect");

        let mut core_h = GbaCore::new(None, build_sio_test_rom(true, 0xA5, 0x1234_5678, 0xCAFE));
        let mut core_j = GbaCore::new(None, build_sio_test_rom(false, 0x5A, 0xDEAD_BEEF, 0xBEEF));
        let mut fe = DummyFe;

        for _ in 0..16 {
            host.step_frame(&mut core_h, &mut fe);
            join.step_frame(&mut core_j, &mut fe);
            if core_h.peek32(0x0300_0118) >= 3 && core_j.peek32(0x0300_0118) >= 3 {
                break;
            }
        }

        assert_eq!(core_h.peek32(0x0300_0100), 0x5A);
        assert_eq!(core_j.peek32(0x0300_0100), 0xA5);
        assert_eq!(core_h.peek32(0x0300_0104), 0xDEAD_BEEF);
        assert_eq!(core_j.peek32(0x0300_0104), 0x1234_5678);
        assert_eq!(core_h.peek32(0x0300_0108), 0xBEEF_CAFE);
        assert_eq!(core_j.peek32(0x0300_0108), 0xBEEF_CAFE);
        assert_eq!(core_h.peek32(0x0300_0118), 3);
        assert_eq!(core_j.peek32(0x0300_0118), 3);

        host.shutdown();
        join.shutdown();
    }
}
