use alloc::{boxed::Box, collections::BTreeMap, vec::Vec};
use core::ptr::NonNull;
use serde::{Deserialize, Serialize};

use crate::apu::Apu;
use crate::cartridge::{Cartridge, SaveType};
use crate::hw::{HardwareControl, IRQ_HBLANK, IRQ_SERIAL, IRQ_VBLANK, IRQ_VCOUNT, SioResponse};
use crate::ppu::Ppu;
use crate::serde_helpers::{alloc_boxed_array, box_bytes};
use crate::{EventType, Scheduler};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WatchpointKind {
    Read,
    Write,
    ReadWrite,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Watchpoint {
    pub start: u32,
    pub end: u32,
    pub kind: WatchpointKind,
}

impl Watchpoint {
    pub const fn new(start: u32, end: u32, kind: WatchpointKind) -> Self {
        Self { start, end, kind }
    }

    pub const fn addr(addr: u32, kind: WatchpointKind) -> Self {
        Self {
            start: addr,
            end: addr,
            kind,
        }
    }

    #[inline]
    pub fn matches_access(&self, access_addr: u32, size: u32, write: bool) -> bool {
        let kind_ok = match self.kind {
            WatchpointKind::Read => !write,
            WatchpointKind::Write => write,
            WatchpointKind::ReadWrite => true,
        };
        if !kind_ok {
            return false;
        }
        let access_end = access_addr.saturating_add(size.saturating_sub(1));
        access_addr <= self.end && access_end >= self.start
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WatchpointHit {
    pub addr: u32,
    pub pc: u32,
    pub write: bool,
    pub value: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccessType {
    NonSequential,
    Sequential,
}

#[derive(Copy, Clone, Serialize, Deserialize)]
pub struct PrefetchBuffer {
    pub enabled: bool,
    pub active: bool,
    pub head_addr: u32,
    pub tail_addr: u32,
    pub count: u8,
    pub cycle_acc: u64,
}

impl PrefetchBuffer {
    pub const fn new() -> Self {
        Self {
            enabled: false,
            active: false,
            head_addr: 0,
            tail_addr: 0,
            count: 0,
            cycle_acc: 0,
        }
    }

    pub fn invalidate(&mut self) {
        self.active = false;
        self.count = 0;
        self.cycle_acc = 0;
    }
}

const N_WAITSTATES: [u64; 4] = [4, 3, 2, 8];

#[derive(Serialize, Deserialize)]
pub struct SysBus {
    #[serde(with = "box_bytes")]
    pub bios: Box<[u8; 0x4000]>,
    pub bios_present: bool,
    #[serde(with = "box_bytes")]
    pub ewram: Box<[u8; 0x40000]>,
    #[serde(with = "box_bytes")]
    pub iwram: Box<[u8; 0x8000]>,

    pub ppu: Ppu,
    pub apu: Apu,
    pub hw: HardwareControl,
    pub cart: Cartridge,
    pub scheduler: Scheduler,

    pub prefetch: PrefetchBuffer,
    pub sram_wait: u64,
    pub rom_n_wait: [u64; 3],
    pub rom_s_wait: [u64; 3],

    pub open_bus_latch: u32,
    pub bios_latch: u32,
    pub dma_latch: u32,
    pub last_access_was_dma: bool,
    pub irq_sync_delay: u8,

    // 16 KiB fastmem page pointers across 0x0000_0000..0x0FFF_FFFF (rebuilt on new/clone/load_state).
    #[serde(skip, default = "empty_fastmem")]
    pub(crate) fastmem_read: Box<[Option<NonNull<u8>>; 16384]>,
    #[serde(skip, default = "empty_fastmem")]
    pub(crate) fastmem_write: Box<[Option<NonNull<u8>>; 16384]>,
    #[serde(default)]
    pub rtc_cycles: u64,
    // Track last opcode access cycles cleanly for CPU stepping.
    pub last_cycles: u64,
    #[serde(skip)]
    pub instr_fetch_cycles: u64,
    #[serde(default)]
    pub hdraw_start_cycle: u64,
    #[serde(default)]
    pub hblank_start_cycle: u64,
    #[serde(default)]
    pub scanline_dots_started: bool,

    // Debug watchpoints (fast-path `watchpoint_active` bool avoids overhead when empty).
    #[serde(skip)]
    pub watchpoint_active: bool,
    #[serde(skip)]
    pub debug_pc: u32,
    #[serde(default)]
    pub force_next_fetch_nseq: bool,
    #[serde(default)]
    pub rom_burst_active: bool,
    #[serde(default)]
    pub rom_burst_addr: u32,
    #[serde(skip)]
    pub watchpoint_hit: Option<WatchpointHit>,
    #[serde(skip)]
    pub watchpoints: Vec<Watchpoint>,

    // Headless benchmark / telemetry instrumentation
    #[serde(skip)]
    pub bench_active: bool,
    #[serde(skip)]
    pub ppu_bench_time: core::time::Duration,
    #[serde(skip)]
    pub apu_bench_time: core::time::Duration,
    #[serde(skip)]
    pub dma_bench_time: core::time::Duration,
    #[serde(skip)]
    pub fetch_bench_cycles: u64,
    #[serde(skip)]
    pub read_bench_cycles: u64,
    #[serde(skip)]
    pub write_bench_cycles: u64,
    #[serde(skip)]
    pub dma_bench_cycles: u64,

    // Precomputed waitstate table: [region (0..16)][width (0=8bit, 1=16bit, 2=32bit)][access (0=NonSequential, 1=Sequential)]
    #[serde(skip)]
    pub waitstate_table: [[[u8; 2]; 3]; 16],

    // Cached instruction code page (IWRAM, EWRAM, or ROM)
    #[serde(skip)]
    pub code_page_ptr: Option<NonNull<u8>>,
    #[serde(skip)]
    pub code_page_tag: u32,
    #[serde(skip)]
    pub code_page_region: u8,
    /// Inline O(1) dirty 16 KiB page bitmasks for JIT code invalidation (EWRAM: 16 pages, IWRAM: 2 pages).
    #[serde(skip)]
    pub dirty_ewram_pages: u16,
    #[serde(skip)]
    pub dirty_iwram_pages: u8,
    #[serde(skip)]
    pub invalidate_all_jit: bool,
    #[serde(skip)]
    pub rom_patch_originals: BTreeMap<usize, u8>,
}

impl Clone for SysBus {
    fn clone(&self) -> Self {
        let mut cloned = Self {
            bios: self.bios.clone(),
            bios_present: self.bios_present,
            ewram: self.ewram.clone(),
            iwram: self.iwram.clone(),
            ppu: self.ppu.clone(),
            apu: self.apu.clone(),
            hw: self.hw.clone(),
            cart: self.cart.clone(),
            scheduler: self.scheduler.clone(),
            prefetch: self.prefetch,
            sram_wait: self.sram_wait,
            rom_n_wait: self.rom_n_wait,
            rom_s_wait: self.rom_s_wait,
            open_bus_latch: self.open_bus_latch,
            bios_latch: self.bios_latch,
            dma_latch: self.dma_latch,
            last_access_was_dma: self.last_access_was_dma,
            irq_sync_delay: self.irq_sync_delay,
            fastmem_read: empty_fastmem(),
            fastmem_write: empty_fastmem(),
            rtc_cycles: self.rtc_cycles,
            last_cycles: self.last_cycles,
            instr_fetch_cycles: self.instr_fetch_cycles,
            hdraw_start_cycle: self.hdraw_start_cycle,
            hblank_start_cycle: self.hblank_start_cycle,
            scanline_dots_started: self.scanline_dots_started,
            watchpoint_active: self.watchpoint_active,
            debug_pc: self.debug_pc,
            force_next_fetch_nseq: self.force_next_fetch_nseq,
            rom_burst_active: self.rom_burst_active,
            rom_burst_addr: self.rom_burst_addr,
            watchpoint_hit: self.watchpoint_hit,
            watchpoints: self.watchpoints.clone(),
            bench_active: self.bench_active,
            ppu_bench_time: self.ppu_bench_time,
            apu_bench_time: self.apu_bench_time,
            dma_bench_time: self.dma_bench_time,
            fetch_bench_cycles: self.fetch_bench_cycles,
            read_bench_cycles: self.read_bench_cycles,
            write_bench_cycles: self.write_bench_cycles,
            dma_bench_cycles: self.dma_bench_cycles,
            waitstate_table: self.waitstate_table,
            code_page_ptr: None,
            code_page_tag: u32::MAX,
            code_page_region: 0,
            dirty_ewram_pages: 0,
            dirty_iwram_pages: 0,
            invalidate_all_jit: true,
            rom_patch_originals: self.rom_patch_originals.clone(),
        };
        cloned.rebuild_fastmem();
        cloned
    }
}

fn empty_fastmem() -> Box<[Option<NonNull<u8>>; 16384]> {
    alloc_boxed_array(None)
}

unsafe impl Send for SysBus {}
unsafe impl Sync for SysBus {}

impl SysBus {
    pub fn new(bios: Option<&[u8]>, rom: Vec<u8>) -> Self {
        let mut bios_buf = alloc_boxed_array(0u8);
        let bios_present = if let Some(b) = bios {
            let len = b.len().min(0x4000);
            bios_buf[..len].copy_from_slice(&b[..len]);
            true
        } else {
            Self::install_hle_bios_trampoline(&mut bios_buf);
            false
        };

        let mut scheduler = Scheduler::new();
        scheduler.schedule(1006, EventType::PpuHDrawEnd);

        let mut bus = Self {
            bios: bios_buf,
            bios_present,
            ewram: alloc_boxed_array(0u8),
            iwram: alloc_boxed_array(0u8),
            ppu: Ppu::new(),
            apu: Apu::new(),
            hw: HardwareControl::new(),
            cart: Cartridge::new(rom),
            scheduler,
            prefetch: PrefetchBuffer::new(),
            sram_wait: 5,
            rom_n_wait: [5, 5, 5],
            rom_s_wait: [3, 5, 9],
            open_bus_latch: 0,
            bios_latch: 0xE129_F000,
            dma_latch: 0,
            last_access_was_dma: false,
            irq_sync_delay: 0,
            fastmem_read: empty_fastmem(),
            fastmem_write: empty_fastmem(),
            rtc_cycles: 0,
            last_cycles: 0,
            instr_fetch_cycles: 0,
            hdraw_start_cycle: 0,
            hblank_start_cycle: 0,
            scanline_dots_started: false,
            watchpoint_active: false,
            debug_pc: 0,
            force_next_fetch_nseq: false,
            rom_burst_active: false,
            rom_burst_addr: 0,
            watchpoint_hit: None,
            watchpoints: Vec::new(),
            bench_active: false,
            ppu_bench_time: core::time::Duration::ZERO,
            apu_bench_time: core::time::Duration::ZERO,
            dma_bench_time: core::time::Duration::ZERO,
            fetch_bench_cycles: 0,
            read_bench_cycles: 0,
            write_bench_cycles: 0,
            dma_bench_cycles: 0,
            waitstate_table: [[[1; 2]; 3]; 16],
            code_page_ptr: None,
            code_page_tag: u32::MAX,
            code_page_region: 0,
            dirty_ewram_pages: 0,
            dirty_iwram_pages: 0,
            invalidate_all_jit: false,
            rom_patch_originals: BTreeMap::new(),
        };
        bus.update_waitcnt(0x0000);
        bus.rebuild_fastmem();
        bus
    }

    fn install_hle_bios_trampoline(bios: &mut [u8; 0x4000]) {
        let write_w = |buf: &mut [u8], addr: usize, val: u32| {
            buf[addr..addr + 4].copy_from_slice(&val.to_le_bytes());
        };
        // Vector table entries
        write_w(bios, 0x0000, 0xEA00_0006); // B 0x0020
        write_w(bios, 0x0008, 0xEA00_0054); // SWI -> B 0x0160
        write_w(bios, 0x0018, 0xEA00_0042); // IRQ -> B 0x0128
        // Standard GBA BIOS IRQ trampoline at 0x0128
        write_w(bios, 0x0128, 0xE92D_500F); // STMDB SP!, {R0-R3, R12, LR}
        write_w(bios, 0x012C, 0xE3A0_0301); // MOV R0, #0x04000000
        write_w(bios, 0x0130, 0xE28F_E000); // ADD LR, PC, #0
        write_w(bios, 0x0134, 0xE510_F004); // LDR PC, [R0, #-4] ([0x03007FFC])
        write_w(bios, 0x0138, 0xE8BD_500F); // LDMIA SP!, {R0-R3, R12, LR}
        write_w(bios, 0x013C, 0xE25E_F004); // SUBS PC, LR, #4
        // HLE IntrWait sleep loop check at 0x0140
        write_w(bios, 0x0140, 0xEFFE_00FE); // SWI 0xFE (encoded in both upper and lower fields)
        // Matches retail BIOS word at 0x0144 (prefetched when SUBS PC, LR, #4 at 0x013C finishes IRQ)
        write_w(bios, 0x0144, 0xE55E_C002);

        // Clean-room ARM HLE SWI dispatcher (0x0160..0x01D0) + CpuSet (0x01D4..0x0274).
        // Mirrors the real GBA BIOS SWI calling convention (SVC->SYS mode switch, reading the SWI
        // comment byte from [lr, #-2], bounds-checked table dispatch, CpuSet subroutine, SYS->SVC return):
        //  - HALTCNT/POSTFLG writes execute with genuine BIOS privilege (`PC < 0x4000`) and halt
        //    mid-routine so wake-up and epilogue timing match real hardware.
        //  - Reading `[lr, #-2]` from IWRAM vs ROM naturally reflects the ROM waitstate delta.
        write_w(bios, 0x0160, 0xE92D_5800); // stmfd sp!, {r11, r12, lr}
        write_w(bios, 0x0164, 0xE14F_B000); // mrs r11, spsr
        write_w(bios, 0x0168, 0xE92D_0800); // stmfd sp!, {r11}
        write_w(bios, 0x016C, 0xE31B_0020); // tst r11, #0x20 (Thumb check)
        write_w(bios, 0x0170, 0xE55E_C002); // ldrb r12, [lr, #-2] (SWI comment byte)
        write_w(bios, 0x0174, 0xE20B_B080); // and r11, r11, #0x80 (preserve caller I flag)
        write_w(bios, 0x0178, 0xE38B_B01F); // orr r11, r11, #0x1F (SYS mode)
        write_w(bios, 0x017C, 0xE121_F00B); // msr cpsr_c, r11
        write_w(bios, 0x0180, 0xE92D_4004); // stmfd sp!, {r2, lr}
        write_w(bios, 0x0184, 0xE35C_002B); // cmp r12, #0x2B (SWI table bounds check)
        write_w(bios, 0x0188, 0x2A00_0004); // bhs 0x01A0 (swi_return)
        write_w(bios, 0x018C, 0xE59F_B03C); // ldr r11, [pc, #0x3C] (0x01D0 -> table ptr)
        write_w(bios, 0x0190, 0xE24C_C00B); // sub r12, r12, #0x0B
        write_w(bios, 0x0194, 0xE79B_C10C); // ldr r12, [r11, r12, lsl #2]
        write_w(bios, 0x0198, 0xE1A0_E00F); // mov lr, pc (0x01A0)
        write_w(bios, 0x019C, 0xE12F_FF1C); // bx r12 (call CpuSet)
        // SWI return epilogue (0x01A0):
        write_w(bios, 0x01A0, 0xE8BD_4004); // ldmfd sp!, {r2, lr}
        write_w(bios, 0x01A4, 0xE10F_B000); // mrs r11, cpsr
        write_w(bios, 0x01A8, 0xE3CB_B01F); // bic r11, r11, #0x1F
        write_w(bios, 0x01AC, 0xE38B_B093); // orr r11, r11, #0x93 (SVC + IRQ disabled)
        write_w(bios, 0x01B0, 0xE121_F00B); // msr cpsr_c, r11
        write_w(bios, 0x01B4, 0xE8BD_0800); // ldmfd sp!, {r11}
        write_w(bios, 0x01B8, 0xE16F_F00B); // msr spsr_fsxc, r11
        write_w(bios, 0x01BC, 0xE8BD_5800); // ldmfd sp!, {r11, r12, lr}
        write_w(bios, 0x01C0, 0xE1B0_F00E); // movs pc, lr
        // Prefetched BIOS open-bus words right after SWI return (matching retail GBA BIOS latch):
        write_w(bios, 0x01C4, 0xE3A0_2004);
        write_w(bios, 0x01C8, 0xE3A0_2004);
        // SWI dispatch table entry for 0x0B -> CpuSet at 0x01D4, plus table base pointer literal:
        write_w(bios, 0x01CC, 0x0000_01D4);
        write_w(bios, 0x01D0, 0x0000_01CC);

        // CpuSet subroutine at 0x01D4..0x0274:
        write_w(bios, 0x01D4, 0xE92D_5FF8); // stmfd sp!, {r3-r12, lr}
        write_w(bios, 0x01D8, 0xE350_0402); // cmp r0, #0x02000000 (BIOS source protection)
        write_w(bios, 0x01DC, 0x3A00_0023); // blo 0x0270 (done)
        write_w(bios, 0x01E0, 0xE351_0402); // cmp r1, #0x02000000 (BIOS dest protection)
        write_w(bios, 0x01E4, 0x3A00_0021); // blo 0x0270 (done)
        write_w(bios, 0x01E8, 0xE202_5401); // and r5, r2, #0x01000000 (fixed-src flag)
        write_w(bios, 0x01EC, 0xE202_6301); // and r6, r2, #0x04000000 (32-bit flag)
        write_w(bios, 0x01F0, 0xE1A0_3582); // mov r3, r2, lsl #11
        write_w(bios, 0x01F4, 0xE1B0_35A3); // movs r3, r3, lsr #11 (count = cnt & 0x1FFFFF)
        write_w(bios, 0x01F8, 0x0A00_001C); // beq 0x0270 (done)
        write_w(bios, 0x01FC, 0xE356_0000); // cmp r6, #0
        write_w(bios, 0x0200, 0x1A00_000D); // bne 0x023C (copy32)
        // 16-bit CpuSet (r0 &= !1, r1 &= !1)
        write_w(bios, 0x0204, 0xE3C0_0001); // bic r0, r0, #1
        write_w(bios, 0x0208, 0xE3C1_1001); // bic r1, r1, #1
        write_w(bios, 0x020C, 0xE355_0000); // cmp r5, #0
        write_w(bios, 0x0210, 0x1A00_0004); // bne 0x0228 (fill16)
        // copy16 loop (0x0214):
        write_w(bios, 0x0214, 0xE0D0_40B2); // ldrh r4, [r0], #2
        write_w(bios, 0x0218, 0xE0C1_40B2); // strh r4, [r1], #2
        write_w(bios, 0x021C, 0xE253_3001); // subs r3, r3, #1
        write_w(bios, 0x0220, 0x1AFF_FFFB); // bne 0x0214
        write_w(bios, 0x0224, 0xEA00_0011); // b 0x0270 (done)
        // fill16 loop (0x0228):
        write_w(bios, 0x0228, 0xE1D0_40B0); // ldrh r4, [r0]
        write_w(bios, 0x022C, 0xE0C1_40B2); // strh r4, [r1], #2
        write_w(bios, 0x0230, 0xE253_3001); // subs r3, r3, #1
        write_w(bios, 0x0234, 0x1AFF_FFFB); // bne 0x0228
        write_w(bios, 0x0238, 0xEA00_000C); // b 0x0270 (done)
        // copy32 (0x023C):
        write_w(bios, 0x023C, 0xE3C0_0003); // bic r0, r0, #3
        write_w(bios, 0x0240, 0xE3C1_1003); // bic r1, r1, #3
        write_w(bios, 0x0244, 0xE355_0000); // cmp r5, #0
        write_w(bios, 0x0248, 0x1A00_0004); // bne 0x0260 (fill32)
        // copy32 loop (0x024C):
        write_w(bios, 0x024C, 0xE490_4004); // ldr r4, [r0], #4
        write_w(bios, 0x0250, 0xE481_4004); // str r4, [r1], #4
        write_w(bios, 0x0254, 0xE253_3001); // subs r3, r3, #1
        write_w(bios, 0x0258, 0x1AFF_FFFB); // bne 0x024C
        write_w(bios, 0x025C, 0xEA00_0003); // b 0x0270 (done)
        // fill32 loop (0x0260):
        write_w(bios, 0x0260, 0xE590_4000); // ldr r4, [r0]
        write_w(bios, 0x0264, 0xE481_4004); // str r4, [r1], #4
        write_w(bios, 0x0268, 0xE253_3001); // subs r3, r3, #1
        write_w(bios, 0x026C, 0x1AFF_FFFB); // bne 0x0260
        // done (0x0270):
        write_w(bios, 0x0270, 0xE8BD_5FF8); // ldmfd sp!, {r3-r12, lr}
        write_w(bios, 0x0274, 0xE12F_FF1E); // bx lr
    }

    pub fn rebuild_fastmem(&mut self) {
        self.invalidate_code_page();
        self.fastmem_read.fill(None);
        self.fastmem_write.fill(None);
        let ewram_base = self.ewram.as_mut_ptr();
        let iwram_base = self.iwram.as_mut_ptr();
        let rom_base = self.cart.rom.as_ptr();
        let rom_len = self.cart.rom.len();
        let gpio_en = self.cart.gpio_enabled();
        let eff_save = self.cart.effective_save_type();
        let eeprom_possible = matches!(eff_save, SaveType::Eeprom512B | SaveType::Eeprom8K)
            || (eff_save == SaveType::None && self.cart.save_type_override.is_none());

        for page in 0..16384usize {
            let addr = (page as u32) << 14;
            let region = (addr >> 24) & 0x0F;
            let (r_ptr, w_ptr): (Option<*const u8>, Option<*mut u8>) = match region {
                0x02 => {
                    let off = (addr as usize) & 0x3FFFF;
                    let p = unsafe { ewram_base.add(off) };
                    (Some(p as *const u8), Some(p))
                }
                0x03 => {
                    let off = (addr as usize) & 0x7FFF;
                    let p = unsafe { iwram_base.add(off) };
                    (Some(p as *const u8), Some(p))
                }
                0x05 | 0x06 => (None, None),
                0x08..=0x0D => {
                    let off = (addr as usize) & 0x01FF_FFFF;
                    // Exclude first page of WS0/1/2 (0x08/0x0A/0x0C offset 0) when GPIO is enabled,
                    // and exclude EEPROM pages in region 0x0D matching Cartridge::is_eeprom_addr.
                    let is_gpio_page = gpio_en && off == 0;
                    let is_eeprom_page = region == 0x0D
                        && eeprom_possible
                        && (rom_len <= 0x0100_0000 || off >= 0x01FF_C000);
                    if !is_gpio_page && !is_eeprom_page && off + 0x4000 <= rom_len {
                        (Some(unsafe { rom_base.add(off) }), None)
                    } else {
                        (None, None)
                    }
                }
                _ => (None, None),
            };
            self.fastmem_read[page] = r_ptr.and_then(|p| NonNull::new(p as *mut u8));
            self.fastmem_write[page] = w_ptr.and_then(NonNull::new);
        }
    }

    /// Updates save-type override on the cartridge and refreshes fastmem page mappings
    /// so EEPROM reads/writes never bypass through cached ROM pointers.
    pub fn set_save_type_override(&mut self, ov: Option<SaveType>) {
        self.cart.set_save_type_override(ov);
        self.rebuild_fastmem();
    }

    pub fn update_waitcnt(&mut self, val: u16) {
        self.sram_wait = 1 + N_WAITSTATES[(val & 3) as usize];

        let ws0_n = 1 + N_WAITSTATES[((val >> 2) & 3) as usize];
        let ws0_s = if (val & (1 << 4)) != 0 { 2 } else { 3 };
        let ws1_n = 1 + N_WAITSTATES[((val >> 5) & 3) as usize];
        let ws1_s = if (val & (1 << 7)) != 0 { 2 } else { 5 };
        let ws2_n = 1 + N_WAITSTATES[((val >> 8) & 3) as usize];
        let ws2_s = if (val & (1 << 10)) != 0 { 2 } else { 9 };

        self.rom_n_wait = [ws0_n, ws1_n, ws2_n];
        self.rom_s_wait = [ws0_s, ws1_s, ws2_s];

        let pref_en = (val & (1 << 14)) != 0;
        if !pref_en {
            self.prefetch.invalidate();
        }
        self.prefetch.enabled = pref_en;

        self.rebuild_waitstate_table();
        self.invalidate_all_jit = true;
    }

    pub fn rebuild_waitstate_table(&mut self) {
        // Clear all to 1 cycle default
        self.waitstate_table = [[[1; 2]; 3]; 16];

        // Region 0x02: EWRAM (16-bit bus; waitstates = 15 - MEMCNT[27:24], default 0xD -> 2 waitstates -> 3/6 cycles)
        let ewram_ws = 15u8.saturating_sub(((self.hw.memcnt >> 24) & 0x0F) as u8);
        let ewram_16 = 1u8.saturating_add(ewram_ws);
        let ewram_32 = ewram_16.saturating_mul(2);
        self.waitstate_table[0x02][0] = [ewram_16, ewram_16];
        self.waitstate_table[0x02][1] = [ewram_16, ewram_16];
        self.waitstate_table[0x02][2] = [ewram_32, ewram_32];

        // Region 0x03: IWRAM (32-bit bus, 1 cycle)
        self.waitstate_table[0x03] = [[1; 2]; 3];

        // Region 0x05 (PRAM) & 0x06 (VRAM): 16-bit bus, 1 cycle for 8/16-bit, 2 cycles for 32-bit
        for reg in [0x05, 0x06] {
            self.waitstate_table[reg][0] = [1, 1];
            self.waitstate_table[reg][1] = [1, 1];
            self.waitstate_table[reg][2] = [2, 2];
        }

        // Region 0x07 (OAM): 32-bit bus, 1 cycle
        self.waitstate_table[0x07] = [[1; 2]; 3];

        // Region 0x08..=0x0D: GamePak ROM (16-bit bus)
        let pairs: [&[usize]; 3] = [&[0x08, 0x09], &[0x0A, 0x0B], &[0x0C, 0x0D]];
        for (idx, &reg_pair) in pairs.iter().enumerate() {
            let n = self.rom_n_wait[idx] as u8;
            let s = self.rom_s_wait[idx] as u8;
            for &r in reg_pair {
                // 8-bit
                self.waitstate_table[r][0][0] = n;
                self.waitstate_table[r][0][1] = s;
                // 16-bit
                self.waitstate_table[r][1][0] = n;
                self.waitstate_table[r][1][1] = s;
                // 32-bit: 1 NonSeq/Seq halfword + 1 Seq halfword
                self.waitstate_table[r][2][0] = n.saturating_add(s);
                self.waitstate_table[r][2][1] = s.saturating_add(s);
            }
        }

        // Region 0x0E & 0x0F: SRAM
        let sram_w = self.sram_wait as u8;
        for r in [0x0E, 0x0F] {
            self.waitstate_table[r] = [[sram_w; 2]; 3];
        }
    }

    /// Canonicalizes a 16 KiB page tag (`(addr >> 14) & 0x3FFF`) across EWRAM, IWRAM, and ROM mirrors.
    #[inline(always)]
    pub fn canonical_code_page(page: u32) -> u32 {
        let region = (page >> 10) & 0x0F;
        match region {
            0x02 => (0x02 << 10) | (page & 0x0F),
            0x03 => (0x03 << 10) | (page & 0x01),
            0x08..=0x0D => (0x08 << 10) | (page & 0x07FF),
            _ => page & 0x3FFF,
        }
    }

    /// Drops the active interpreter instruction-page cache (used on branches/DMA without evicting JIT).
    #[inline(always)]
    pub fn invalidate_code_page(&mut self) {
        self.code_page_tag = u32::MAX;
        self.code_page_ptr = None;
    }

    /// Drops both the active interpreter code page cache and requests full JIT cache flush.
    #[inline]
    pub fn invalidate_all_code(&mut self) {
        self.code_page_tag = u32::MAX;
        self.code_page_ptr = None;
        self.dirty_ewram_pages = 0;
        self.dirty_iwram_pages = 0;
        self.invalidate_all_jit = true;
    }

    #[inline(always)]
    pub fn has_dirty_code_pages(&self) -> bool {
        self.dirty_ewram_pages != 0 || self.dirty_iwram_pages != 0
    }

    /// Invalidates cached code-page state and sets the O(1) dirty-page bitmask for JIT
    /// whenever a write to `(region, page)` occurs (handling EWRAM/IWRAM/ROM mirrors).
    #[inline(always)]
    pub(crate) fn invalidate_code_page_if_aliased(&mut self, region: u32, page: u32) {
        if region == 0x02 {
            self.dirty_ewram_pages |= 1u16 << (page & 0x0F);
        } else if region == 0x03 {
            self.dirty_iwram_pages |= 1u8 << (page & 1);
        } else {
            self.invalidate_all_jit = true;
        }
        if self.code_page_ptr.is_some() {
            let matches = if region == 0x03 && self.code_page_region == 0x03 {
                (page & 1) == (self.code_page_tag & 1)
            } else if region == 0x02 && self.code_page_region == 0x02 {
                (page & 0x0F) == (self.code_page_tag & 0x0F)
            } else if (0x08..=0x0D).contains(&region)
                && (0x08..=0x0D).contains(&(self.code_page_region as u32))
            {
                (page & 0x07FF) == (self.code_page_tag & 0x07FF)
            } else {
                page == self.code_page_tag
            };
            if matches {
                self.code_page_tag = u32::MAX;
                self.code_page_ptr = None;
            }
        }
    }

    #[inline]
    pub fn rom_waitstate_idx(addr: u32) -> usize {
        match (addr >> 24) & 0x0F {
            0x08 | 0x09 => 0,
            0x0A | 0x0B => 1,
            _ => 2,
        }
    }

    #[inline]
    pub fn rom_fetch_cycles(&self, addr: u32, access: AccessType) -> u64 {
        let ws = Self::rom_waitstate_idx(addr);
        // Crossing a 128 KiB ROM boundary forces a Non-Sequential cycle on GBA hardware.
        let forced_n = (addr & 0x1FFFF) == 0;
        if access == AccessType::NonSequential || forced_n {
            self.rom_n_wait[ws]
        } else {
            self.rom_s_wait[ws]
        }
    }

    /// Advances the Game Pak ROM 16-bit sequential burst address counter for a single halfword access.
    ///
    /// Returns `(effective_rom_addr, cycles)`. On real GBA hardware, Non-Sequential accesses latch the
    /// full address into the cartridge ROM and start a sequential burst; Sequential accesses omit the
    /// address phase and clock the cartridge's internal 16-bit halfword counter (`A1..A16`), while the
    /// GBA SoC ends the burst (`rom_burst_active = false`) whenever `(addr & 0x1FFFF) == 0x1FFFE`
    /// so the subsequent ROM access is forced Non-Sequential across 128 KiB boundaries.
    #[inline]
    pub fn rom_step_burst(&mut self, aligned: u32, access: AccessType) -> (u32, u64) {
        let ws = Self::rom_waitstate_idx(aligned);
        let (eff_addr, cyc) = if access == AccessType::NonSequential || !self.rom_burst_active {
            let cyc = self.rom_n_wait[ws];
            self.rom_burst_addr = (aligned & !0x1FFFF) | (aligned.wrapping_add(2) & 0x1FFFF);
            (aligned, cyc)
        } else {
            let cyc = self.rom_s_wait[ws];
            let eff = self.rom_burst_addr;
            self.rom_burst_addr = (eff & !0x1FFFF) | (eff.wrapping_add(2) & 0x1FFFF);
            (eff, cyc)
        };
        self.rom_burst_active = (aligned & 0x1FFFF) != 0x1FFFE;
        (eff_addr, cyc)
    }

    #[inline]
    pub fn step_internal(&mut self, cycles: u64) {
        self.step_prefetch(cycles);
    }

    #[inline]
    pub fn step_prefetch(&mut self, cycles: u64) {
        if !self.prefetch.enabled || !self.prefetch.active || cycles == 0 {
            return;
        }
        // GamePak prefetch stops when reaching a 128 KiB boundary.
        if (self.prefetch.tail_addr & 0x1FFFF) == 0 {
            self.prefetch.cycle_acc = 0;
            return;
        }
        let ws = Self::rom_waitstate_idx(self.prefetch.tail_addr);
        let s_cyc = self.rom_s_wait[ws].max(1);
        self.prefetch.cycle_acc = self.prefetch.cycle_acc.saturating_add(cycles);
        if self.prefetch.cycle_acc < s_cyc {
            return;
        }
        let max_by_buf = (8u8.saturating_sub(self.prefetch.count)) as u64;
        let rem_to_boundary = (0x20000 - (self.prefetch.tail_addr & 0x1FFFF)) >> 1;
        let max_steps = max_by_buf.min(rem_to_boundary as u64);
        let steps = (self.prefetch.cycle_acc / s_cyc).min(max_steps);
        if steps > 0 {
            self.prefetch.cycle_acc -= steps * s_cyc;
            self.prefetch.count += steps as u8;
            self.prefetch.tail_addr = self.prefetch.tail_addr.wrapping_add((steps * 2) as u32);
        }
        if self.prefetch.count >= 8 || (self.prefetch.tail_addr & 0x1FFFF) == 0 {
            self.prefetch.cycle_acc = 0;
        }
    }

    #[inline]
    pub fn sync_ppu_hdraw(&mut self) {
        if !self.ppu.dot_accurate_mode {
            return;
        }
        self.sync_ppu_hdraw_now();
    }

    #[inline]
    pub fn sync_ppu_hdraw_now(&mut self) {
        if self.ppu.vcount < 160 && (self.ppu.dispstat & 0x0002) == 0 {
            let elapsed = self
                .scheduler
                .current_cycle
                .saturating_sub(self.hdraw_start_cycle);
            let target_x = ((elapsed.saturating_sub(46)) / 4) as usize;
            let target_x = target_x.min(crate::LCD_WIDTH);
            let cur_x = self.ppu.dot_x as usize;
            if target_x > cur_x {
                if !self.scanline_dots_started {
                    self.ppu.begin_scanline(self.ppu.vcount as usize);
                    self.scanline_dots_started = true;
                }
                self.ppu
                    .render_dot_range(self.ppu.vcount as usize, cur_x, target_x);
                self.ppu.dot_x = target_x as u16;
            }
        }
    }

    pub fn dispatch_event(&mut self, ev_cycle: u64, kind: EventType) {
        match kind {
            EventType::PpuHDrawEnd => {
                self.hblank_start_cycle = ev_cycle;
                self.ppu.dispstat |= 0x0002; // Enter HBlank
                if self.ppu.vcount < 160 {
                    #[cfg(feature = "std")]
                    let t0 = if self.bench_active {
                        Some(std::time::Instant::now())
                    } else {
                        None
                    };

                    if self.ppu.dot_accurate_mode || self.scanline_dots_started {
                        if !self.scanline_dots_started {
                            self.ppu.begin_scanline(self.ppu.vcount as usize);
                        }
                        if (self.ppu.dot_x as usize) < 240 {
                            self.ppu.render_dot_range(
                                self.ppu.vcount as usize,
                                self.ppu.dot_x as usize,
                                240,
                            );
                        }
                        self.ppu.end_scanline();
                        self.scanline_dots_started = false;
                    } else {
                        self.ppu.render_scanline(self.ppu.vcount as usize);
                    }

                    #[cfg(feature = "std")]
                    if let Some(t) = t0 {
                        self.ppu_bench_time += t.elapsed();
                    }

                    self.trigger_dma_by_timing(2);
                }
                // DMA3 Video Capture (timing == 3) runs on scanlines 2..=161 and auto-disables on 162.
                if (2..=161).contains(&self.ppu.vcount) {
                    self.trigger_video_capture_dma();
                } else if self.ppu.vcount == 162
                    && self.hw.dma[3].enabled()
                    && self.hw.dma[3].timing() == 3
                {
                    self.hw.dma[3].cnt &= !0x8000;
                    self.hw.dma[3].active = false;
                    self.hw.recompute_active_dma();
                }
                if (self.ppu.dispstat & 0x0010) != 0 {
                    self.hw.request_irq(IRQ_HBLANK);
                }
                self.scheduler
                    .schedule(ev_cycle + 226, EventType::PpuHBlank);
            }
            EventType::PpuHBlank => {
                self.ppu.dispstat &= !0x0002; // Exit HBlank
                let prev_vcount = self.ppu.vcount;
                self.ppu.finish_hblank_affine(prev_vcount < 160);
                self.ppu.vcount = (prev_vcount + 1) % 228;
                self.ppu.advance_dispcnt_pipeline();

                #[cfg(feature = "std")]
                let t0 = if self.bench_active {
                    Some(std::time::Instant::now())
                } else {
                    None
                };

                // Advance APU once per scanline in HBlank (removes periodic 256-cycle ApuSampleTick wakeups)
                self.apu.advance_scanline(ev_cycle);

                #[cfg(feature = "std")]
                if let Some(t) = t0 {
                    self.apu_bench_time += t.elapsed();
                }

                if self.ppu.vcount == 0 {
                    self.ppu.dispstat &= !0x0001;
                    self.ppu.reload_affine_refs();
                    if self.cart.effective_rtc() {
                        self.rtc_cycles = self.rtc_cycles.saturating_add(crate::CYCLES_PER_FRAME);
                        if self.rtc_cycles >= u64::from(crate::CPU_CLOCK_HZ) {
                            self.cart.rtc.advance_second();
                            self.rtc_cycles -= u64::from(crate::CPU_CLOCK_HZ);
                        }
                    }
                } else if self.ppu.vcount == 160 {
                    self.ppu.dispstat |= 0x0001;
                    self.ppu.frame_ready = true;
                    if (self.ppu.dispstat & 0x0008) != 0 {
                        self.hw.request_irq(IRQ_VBLANK);
                    }
                    self.trigger_dma_by_timing(1);
                } else if self.ppu.vcount == 227 {
                    self.ppu.dispstat &= !0x0001;
                }

                if self.ppu.vcount == self.ppu.vcount_match_target() {
                    self.ppu.dispstat |= 0x0004;
                    if (self.ppu.dispstat & 0x0020) != 0 {
                        self.hw.request_irq(IRQ_VCOUNT);
                    }
                } else {
                    self.ppu.dispstat &= !0x0004;
                }

                self.scanline_dots_started = false;
                if self.ppu.vcount < 160 {
                    self.hdraw_start_cycle = ev_cycle;
                    self.ppu.dot_x = 0;
                    // On-demand dot-accurate sync runs on PPU/PRAM/VRAM/OAM writes and at PpuHDrawEnd,
                    // avoiding 38,400 redundant 4-cycle scheduler wakeups per frame.
                }

                self.scheduler
                    .schedule(ev_cycle + 1006, EventType::PpuHDrawEnd);
            }
            EventType::PpuDotStep => {
                if self.ppu.dot_accurate_mode
                    && self.ppu.vcount < 160
                    && (self.ppu.dispstat & 0x0002) == 0
                {
                    let x = self.ppu.dot_x;
                    if x < 240 {
                        self.ppu.render_dot_range(
                            self.ppu.vcount as usize,
                            x as usize,
                            (x + 1) as usize,
                        );
                        self.ppu.dot_x = x + 1;
                        if x + 1 < 240 {
                            self.scheduler.schedule(ev_cycle + 4, EventType::PpuDotStep);
                        }
                    }
                }
            }
            EventType::TimerOverflow(ch) => {
                self.handle_timer_overflow(ch as usize, ev_cycle);
            }
            EventType::DmaActivate(ch) => {
                let idx = ch as usize;
                if self.hw.dma[idx].enabled() {
                    self.hw.dma[idx].active = true;
                    self.hw.dma[idx].first_unit = true;
                    self.hw.recompute_active_dma();
                }
            }
            EventType::ApuSampleTick => {
                // Periodic APU event retired in favor of scanline batching in EventType::PpuHBlank.
            }
            EventType::SioTransferComplete => {
                self.complete_sio_no_cable();
            }
            EventType::IrqSynchronizer | EventType::None => {}
        }
    }

    /// Auto-completes internal-clock SIO transfers when no link cable is connected.
    pub fn complete_sio_no_cable(&mut self) {
        if self.hw.link_connected || (self.hw.siocnt & 0x0080) == 0 {
            return;
        }
        let mode_bits = (self.hw.siocnt >> 12) & 0x03;
        match mode_bits {
            0 => {
                self.hw.siodata8 = 0x00FF;
            }
            1 => {
                self.hw.siomulti[0] = 0xFFFF;
                self.hw.siomulti[1] = 0xFFFF;
            }
            2 => {
                self.hw.siomulti[0] = self.hw.siodata8;
                self.hw.siomulti[1] = 0xFFFF;
                self.hw.siomulti[2] = 0xFFFF;
                self.hw.siomulti[3] = 0xFFFF;
                // Parent ID = 0 (bits 5..4 = 0), Error = 0 (bit 6 = 0), SD = 1 (bit 3 = 1).
                self.hw.siocnt = (self.hw.siocnt & !0x0074) | 0x0008;
            }
            _ => {}
        }
        self.hw.siocnt &= !0x0080;
        if (self.hw.siocnt & 0x4000) != 0 {
            self.hw.request_irq(IRQ_SERIAL);
        }
    }

    /// Completes an active SIO transfer with peer data delivered by a link-cable frontend.
    pub fn complete_sio_transfer(&mut self, resp: SioResponse) {
        let mode_bits = (self.hw.siocnt >> 12) & 0x03;
        match mode_bits {
            0 => {
                self.hw.siodata8 = u16::from(resp.data8);
            }
            1 => {
                self.hw.siomulti[0] = resp.data32 as u16;
                self.hw.siomulti[1] = (resp.data32 >> 16) as u16;
            }
            2 => {
                self.hw.siomulti = resp.multi;
                let id_bits = (u16::from(resp.multi_id) & 0x03) << 4;
                let err_bit = if resp.error { 0x0040 } else { 0 };
                self.hw.siocnt = (self.hw.siocnt & !0x0074) | 0x0008 | id_bits | err_bit;
            }
            _ => {}
        }
        self.hw.siocnt &= !0x0080;
        self.hw.pending_sio_transfer = None;
        self.scheduler.cancel(EventType::SioTransferComplete);
        if (self.hw.siocnt & 0x4000) != 0 {
            self.hw.request_irq(IRQ_SERIAL);
        }
    }

    #[inline]
    pub fn read_open_bus_32(&self, _addr: u32, pc: u32, is_thumb: bool) -> u32 {
        if self.last_access_was_dma {
            return self.dma_latch;
        }
        let _ = (pc, is_thumb);
        self.open_bus_latch
    }

    #[inline]
    pub fn read_open_bus_16(&self, addr: u32, pc: u32, is_thumb: bool) -> u16 {
        let word = self.read_open_bus_32(addr, pc, is_thumb);
        ((word >> ((addr & 2) * 8)) & 0xFFFF) as u16
    }

    #[inline]
    pub fn read_open_bus_8(&self, addr: u32, pc: u32, is_thumb: bool) -> u8 {
        let word = self.read_open_bus_32(addr, pc, is_thumb);
        ((word >> ((addr & 3) * 8)) & 0xFF) as u8
    }

    #[inline(always)]
    pub fn fetch_16_fast(&mut self, addr: u32) -> (u16, u64) {
        if self.force_next_fetch_nseq {
            self.force_next_fetch_nseq = false;
            return self.fetch_16(addr, AccessType::NonSequential);
        }
        let aligned = addr & !1;
        if aligned >= 0x1000_0000 {
            return self.fetch_16(addr, AccessType::Sequential);
        }
        let page_tag = aligned >> 14;
        if page_tag == self.code_page_tag
            && let Some(ptr) = self.code_page_ptr
        {
            let off = (aligned & 0x3FFE) as usize;
            // SAFETY: `ptr` points to a valid 16 KiB page allocation. `off` is at most 0x3FFE.
            // Reading 2 bytes accesses `0x3FFE..0x4000`, strictly within the page allocation.
            let val = unsafe {
                (ptr.as_ptr().add(off) as *const u16)
                    .read_unaligned()
                    .to_le()
            };
            let region = self.code_page_region as usize;
            let cyc = if (0x08..=0x0D).contains(&region) {
                if self.prefetch.enabled {
                    if self.prefetch.active
                        && self.prefetch.count > 0
                        && self.prefetch.head_addr == aligned
                    {
                        self.prefetch.count -= 1;
                        self.prefetch.head_addr = self.prefetch.head_addr.wrapping_add(2);
                        1
                    } else {
                        self.prefetch.active = true;
                        self.prefetch.count = 0;
                        self.prefetch.cycle_acc = 0;
                        self.prefetch.head_addr = aligned.wrapping_add(2);
                        self.prefetch.tail_addr = aligned.wrapping_add(2);
                        if (aligned & 0x1FFFF) == 0 {
                            self.waitstate_table[region][1][0] as u64
                        } else {
                            self.waitstate_table[region][1][1] as u64
                        }
                    }
                } else if (aligned & 0x1FFFF) == 0 {
                    self.waitstate_table[region][1][0] as u64
                } else {
                    self.waitstate_table[region][1][1] as u64
                }
            } else {
                let c = self.waitstate_table[region][1][1] as u64;
                self.step_prefetch(c);
                c
            };
            self.open_bus_latch = (val as u32) | ((val as u32) << 16);
            if self.bench_active {
                self.fetch_bench_cycles += cyc;
            }
            return (val, cyc);
        }
        self.fetch_16(addr, AccessType::Sequential)
    }

    /// Instruction pipeline fetch for Thumb 16-bit halfword.
    /// Updates `open_bus_latch` and accounts for GamePak prefetch without altering `last_access_was_dma`.
    pub fn fetch_16(&mut self, addr: u32, access: AccessType) -> (u16, u64) {
        let aligned = addr & !1;
        if aligned >= 0x1000_0000 {
            self.invalidate_code_page();
            let val = self.read_open_bus_16(aligned, aligned, true);
            self.step_prefetch(1);
            self.open_bus_latch = (val as u32) | ((val as u32) << 16);
            if self.bench_active {
                self.fetch_bench_cycles += 1;
            }
            return (val, 1);
        }
        let region = (aligned >> 24) & 0x0F;
        let page_tag = aligned >> 14;
        self.code_page_tag = page_tag;
        self.code_page_ptr = self.fastmem_read[page_tag as usize];
        self.code_page_region = region as u8;

        if (0x08..=0x0D).contains(&region) {
            let val = self.read_rom_half_raw(aligned);
            self.open_bus_latch = (val as u32) | ((val as u32) << 16);

            let cyc = if self.prefetch.enabled {
                if self.prefetch.active
                    && self.prefetch.count > 0
                    && self.prefetch.head_addr == aligned
                {
                    self.prefetch.count -= 1;
                    self.prefetch.head_addr = self.prefetch.head_addr.wrapping_add(2);
                    1
                } else {
                    self.prefetch.active = true;
                    self.prefetch.count = 0;
                    self.prefetch.cycle_acc = 0;
                    self.prefetch.head_addr = aligned.wrapping_add(2);
                    self.prefetch.tail_addr = aligned.wrapping_add(2);
                    self.rom_fetch_cycles(aligned, access)
                }
            } else {
                self.rom_fetch_cycles(aligned, access)
            };
            let res = (val, cyc);
            if self.bench_active {
                self.fetch_bench_cycles += cyc;
            }
            return res;
        }

        let saved_dma = self.last_access_was_dma;
        let (val, cyc) = self.read_16_non_rom_raw(aligned, aligned, true);
        self.last_access_was_dma = saved_dma;
        self.step_prefetch(cyc);
        self.open_bus_latch = (val as u32) | ((val as u32) << 16);
        if self.bench_active {
            self.fetch_bench_cycles += cyc;
        }
        (val, cyc)
    }

    #[inline(always)]
    pub fn fetch_32_fast(&mut self, addr: u32) -> (u32, u64) {
        if self.force_next_fetch_nseq {
            self.force_next_fetch_nseq = false;
            return self.fetch_32(addr, AccessType::NonSequential);
        }
        let aligned = addr & !3;
        if aligned >= 0x1000_0000 {
            return self.fetch_32(addr, AccessType::Sequential);
        }
        let page_tag = aligned >> 14;
        if page_tag == self.code_page_tag
            && let Some(ptr) = self.code_page_ptr
        {
            let off = (aligned & 0x3FFC) as usize;
            // SAFETY: `ptr` points to a valid 16 KiB page allocation. `off` is at most 0x3FFC.
            // Reading 4 bytes accesses `0x3FFC..0x4000`, strictly within the page allocation.
            let val = unsafe {
                (ptr.as_ptr().add(off) as *const u32)
                    .read_unaligned()
                    .to_le()
            };
            let region = self.code_page_region as usize;
            let cyc = if (0x08..=0x0D).contains(&region) {
                if self.prefetch.enabled {
                    if self.prefetch.active
                        && self.prefetch.count >= 2
                        && self.prefetch.head_addr == aligned
                    {
                        self.prefetch.count -= 2;
                        self.prefetch.head_addr = self.prefetch.head_addr.wrapping_add(4);
                        2
                    } else if self.prefetch.active
                        && self.prefetch.count == 1
                        && self.prefetch.head_addr == aligned
                    {
                        self.prefetch.count = 0;
                        self.prefetch.head_addr = aligned.wrapping_add(4);
                        self.prefetch.tail_addr = aligned.wrapping_add(4);
                        self.prefetch.cycle_acc = 0;
                        1 + (self.waitstate_table[region][1][1] as u64)
                    } else {
                        self.prefetch.active = true;
                        self.prefetch.count = 0;
                        self.prefetch.cycle_acc = 0;
                        self.prefetch.head_addr = aligned.wrapping_add(4);
                        self.prefetch.tail_addr = aligned.wrapping_add(4);
                        if (aligned & 0x1FFFF) == 0 {
                            self.waitstate_table[region][2][0] as u64
                        } else {
                            self.waitstate_table[region][2][1] as u64
                        }
                    }
                } else if (aligned & 0x1FFFF) == 0 {
                    self.waitstate_table[region][2][0] as u64
                } else {
                    self.waitstate_table[region][2][1] as u64
                }
            } else {
                let c = self.waitstate_table[region][2][1] as u64;
                self.step_prefetch(c);
                c
            };
            self.open_bus_latch = val;
            if self.bench_active {
                self.fetch_bench_cycles += cyc;
            }
            return (val, cyc);
        }
        self.fetch_32(addr, AccessType::Sequential)
    }

    /// Instruction pipeline fetch for ARM 32-bit word.
    /// Correctly keeps upper halfword sequential in GamePak prefetch!
    pub fn fetch_32(&mut self, addr: u32, access: AccessType) -> (u32, u64) {
        let aligned = addr & !3;
        if aligned >= 0x1000_0000 {
            self.invalidate_code_page();
            let val = self.read_open_bus_32(aligned, aligned, false);
            self.step_prefetch(1);
            self.open_bus_latch = val;
            if self.bench_active {
                self.fetch_bench_cycles += 1;
            }
            return (val, 1);
        }
        let region = (aligned >> 24) & 0x0F;
        let page_tag = aligned >> 14;
        self.code_page_tag = page_tag;
        self.code_page_ptr = self.fastmem_read[page_tag as usize];
        self.code_page_region = region as u8;

        if (0x08..=0x0D).contains(&region) {
            let val = self.read_rom_word_raw(aligned);
            self.open_bus_latch = val;

            let cyc = if self.prefetch.enabled {
                if self.prefetch.active
                    && self.prefetch.count >= 2
                    && self.prefetch.head_addr == aligned
                {
                    self.prefetch.count -= 2;
                    self.prefetch.head_addr = self.prefetch.head_addr.wrapping_add(4);
                    2
                } else if self.prefetch.active
                    && self.prefetch.count == 1
                    && self.prefetch.head_addr == aligned
                {
                    self.prefetch.count = 0;
                    self.prefetch.head_addr = aligned.wrapping_add(4);
                    self.prefetch.tail_addr = aligned.wrapping_add(4);
                    self.prefetch.cycle_acc = 0;
                    1 + self.rom_fetch_cycles(aligned + 2, AccessType::Sequential)
                } else {
                    let c0 = self.rom_fetch_cycles(aligned, access);
                    let c1 = self.rom_fetch_cycles(aligned + 2, AccessType::Sequential);
                    self.prefetch.active = true;
                    self.prefetch.count = 0;
                    self.prefetch.cycle_acc = 0;
                    self.prefetch.head_addr = aligned.wrapping_add(4);
                    self.prefetch.tail_addr = aligned.wrapping_add(4);
                    c0 + c1
                }
            } else {
                self.rom_fetch_cycles(aligned, access)
                    + self.rom_fetch_cycles(aligned + 2, AccessType::Sequential)
            };
            let res = (val, cyc);
            if self.bench_active {
                self.fetch_bench_cycles += cyc;
            }
            return res;
        }

        let saved_dma = self.last_access_was_dma;
        let (val, cyc) = self.read_32_non_rom_raw(aligned, aligned, false);
        self.last_access_was_dma = saved_dma;
        self.step_prefetch(cyc);
        self.open_bus_latch = val;
        if self.bench_active {
            self.fetch_bench_cycles += cyc;
        }
        (val, cyc)
    }

    #[inline]
    fn read_rom_half_raw(&self, aligned: u32) -> u16 {
        let page = (aligned >> 14) as usize;
        if page < 16384
            && let Some(ptr) = self.fastmem_read[page]
        {
            let off = (aligned & 0x3FFE) as usize;
            return unsafe {
                (ptr.as_ptr().add(off) as *const u16)
                    .read_unaligned()
                    .to_le()
            };
        }
        self.cart.read_rom_16(aligned)
    }

    #[inline]
    fn read_rom_word_raw(&self, aligned: u32) -> u32 {
        let page = (aligned >> 14) as usize;
        if page < 16384
            && let Some(ptr) = self.fastmem_read[page]
        {
            let off = (aligned & 0x3FFC) as usize;
            return unsafe {
                (ptr.as_ptr().add(off) as *const u32)
                    .read_unaligned()
                    .to_le()
            };
        }
        self.cart.read_rom_32(aligned)
    }

    #[inline]
    pub fn set_watchpoints(&mut self, wps: &[Watchpoint]) {
        self.watchpoints.clear();
        self.watchpoints.extend_from_slice(wps);
        self.watchpoint_active = !self.watchpoints.is_empty();
        self.watchpoint_hit = None;
    }

    #[inline]
    pub fn clear_watchpoints(&mut self) {
        self.watchpoint_active = false;
        self.watchpoint_hit = None;
        self.watchpoints.clear();
    }

    #[cold]
    fn check_watchpoint(&mut self, addr: u32, size: u32, write: bool, value: u32) {
        if self.watchpoint_hit.is_some() {
            return;
        }
        for wp in &self.watchpoints {
            if wp.matches_access(addr, size, write) {
                self.watchpoint_hit = Some(WatchpointHit {
                    addr,
                    pc: self.debug_pc,
                    write,
                    value,
                });
                return;
            }
        }
    }

    #[inline]
    pub fn read_8(&mut self, addr: u32, access: AccessType, pc: u32, is_thumb: bool) -> u8 {
        let val = self.read_8_inner(addr, access, pc, is_thumb);
        if self.watchpoint_active {
            self.check_watchpoint(addr, 1, false, val as u32);
        }
        if self.bench_active {
            self.read_bench_cycles += self.last_cycles;
        }
        val
    }

    fn read_8_inner(&mut self, addr: u32, _access: AccessType, pc: u32, is_thumb: bool) -> u8 {
        self.last_cycles = 1;
        if addr < 0x1000_0000 {
            let page = (addr >> 14) as usize;
            if let Some(ptr) = self.fastmem_read[page] {
                let region = (addr >> 24) & 0x0F;
                if (0x08..=0x0D).contains(&region) {
                    self.prefetch.invalidate();
                    self.force_next_fetch_nseq = true;
                    let aligned = addr & !1;
                    let (_, cyc) = self.rom_step_burst(aligned, AccessType::NonSequential);
                    self.last_cycles = cyc;
                    self.last_access_was_dma = false;
                    // SAFETY: `page` has an active `fastmem_read` pointer within ROM allocation.
                    return unsafe { *ptr.as_ptr().add((addr & 0x3FFF) as usize) };
                }
                let cyc = match region {
                    0x02 => self.waitstate_table[0x02][0][0] as u64,
                    0x03 => 1,
                    0x06 => 1,
                    _ => 1,
                };
                self.step_prefetch(cyc);
                self.last_cycles = cyc;
                self.last_access_was_dma = false;
                // SAFETY: `page` has an active `fastmem_read` pointer only if `off + 0x4000` is within the
                // backing allocation (EWRAM, IWRAM, or ROM). `addr & 0x3FFF` is in `0..0x4000`, so adding
                // this offset is strictly within the allocated 16 KiB page boundary.
                return unsafe { *ptr.as_ptr().add((addr & 0x3FFF) as usize) };
            }
        }

        let region = (addr >> 24) & 0x0F;
        let (val, cyc, clear_dma) = match addr >> 24 {
            0x00 => {
                if addr < 0x4000 {
                    if pc < 0x4000 {
                        let word = u32::from_le_bytes([
                            self.bios[(addr as usize) & 0x3FFC],
                            self.bios[((addr as usize) & 0x3FFC) + 1],
                            self.bios[((addr as usize) & 0x3FFC) + 2],
                            self.bios[((addr as usize) & 0x3FFC) + 3],
                        ]);
                        self.bios_latch = word;
                        ((word >> ((addr & 3) * 8)) as u8, 1, true)
                    } else {
                        (
                            ((self.bios_latch >> ((addr & 3) * 8)) & 0xFF) as u8,
                            1,
                            true,
                        )
                    }
                } else {
                    (self.read_open_bus_8(addr, pc, is_thumb), 1, false)
                }
            }
            0x02 => (
                self.ewram[(addr as usize) & 0x3FFFF],
                self.waitstate_table[0x02][0][0] as u64,
                true,
            ),
            0x03 => (self.iwram[(addr as usize) & 0x7FFF], 1, true),
            0x04 => {
                let off = addr & 0x0FFF_FFFF;
                if off < 0x0400_0400 || matches!(off & 0xFFFF, 0x0800..=0x0803) {
                    let v16 = self.read_io_16(addr & !1, pc, is_thumb);
                    (((v16 >> ((addr & 1) * 8)) & 0xFF) as u8, 1, true)
                } else {
                    (self.read_open_bus_8(addr, pc, is_thumb), 1, false)
                }
            }
            0x05 => (self.ppu.pram[(addr as usize) & 0x3FF], 1, true),
            0x06 => {
                let val = Ppu::mirror_vram(addr as usize, self.ppu.dispcnt)
                    .map(|o| self.ppu.vram[o])
                    .unwrap_or(0);
                (val, 1, true)
            }
            0x07 => (self.ppu.oam[(addr as usize) & 0x3FF], 1, true),
            0x08..=0x0D => {
                self.prefetch.invalidate();
                self.force_next_fetch_nseq = true;
                let (_, c) = self.rom_step_burst(addr & !1, AccessType::NonSequential);
                let v16 = if self.cart.is_eeprom_addr(addr & !1) {
                    self.cart.eeprom.read_bit()
                } else {
                    self.cart.read_rom_16(addr & !1)
                };
                (((v16 >> ((addr & 1) * 8)) & 0xFF) as u8, c, true)
            }
            0x0E | 0x0F => {
                self.prefetch.invalidate();
                self.rom_burst_active = false;
                (self.cart.read_sram(addr), self.sram_wait, true)
            }
            _ => (self.read_open_bus_8(addr, pc, is_thumb), 1, false),
        };

        if !(0x08..=0x0F).contains(&region) || addr >= 0x1000_0000 {
            self.step_prefetch(cyc);
        }
        if clear_dma {
            self.last_access_was_dma = false;
        }
        self.last_cycles = cyc;
        val
    }

    #[inline]
    pub fn read_16(&mut self, addr: u32, access: AccessType, pc: u32, is_thumb: bool) -> u16 {
        let val = self.read_16_inner(addr, access, pc, is_thumb);
        if self.watchpoint_active {
            self.check_watchpoint(addr & !1, 2, false, val as u32);
        }
        if self.bench_active {
            self.read_bench_cycles += self.last_cycles;
        }
        val
    }

    fn read_16_inner(&mut self, addr: u32, access: AccessType, pc: u32, is_thumb: bool) -> u16 {
        let aligned = addr & !1;
        if addr < 0x1000_0000 {
            let page = (aligned >> 14) as usize;
            if let Some(ptr) = self.fastmem_read[page] {
                let region = (aligned >> 24) & 0x0F;
                if (0x08..=0x0D).contains(&region) {
                    self.prefetch.invalidate();
                    self.force_next_fetch_nseq = true;
                    let (eff_addr, cyc) = self.rom_step_burst(aligned, access);
                    self.last_cycles = cyc;
                    self.last_access_was_dma = false;
                    if eff_addr == aligned {
                        let off = (aligned & 0x3FFE) as usize;
                        return unsafe {
                            (ptr.as_ptr().add(off) as *const u16)
                                .read_unaligned()
                                .to_le()
                        };
                    }
                    return self.read_rom_half_raw(eff_addr);
                }
                let cyc = match region {
                    0x02 => self.waitstate_table[0x02][1][0] as u64,
                    0x03 => 1,
                    0x06 => 1,
                    _ => 1,
                };
                self.step_prefetch(cyc);
                self.last_cycles = cyc;
                self.last_access_was_dma = false;
                let off = (aligned & 0x3FFE) as usize;
                // SAFETY: `ptr` points to a valid 16 KiB page. `off` is `aligned & 0x3FFE`, which is at most
                // 0x3FFE. Reading 2 bytes accesses `0x3FFE..0x4000`, staying strictly inside the page.
                // We use `read_unaligned` to avoid any alignment assumptions on host architectures.
                let val = unsafe {
                    (ptr.as_ptr().add(off) as *const u16)
                        .read_unaligned()
                        .to_le()
                };
                return val;
            }
        }

        let region = (aligned >> 24) & 0x0F;
        if (0x08..=0x0D).contains(&region) && addr < 0x1000_0000 {
            self.prefetch.invalidate();
            self.force_next_fetch_nseq = true;
            let (eff_addr, cyc) = self.rom_step_burst(aligned, access);
            let v = if self.cart.is_eeprom_addr(aligned) {
                self.cart.eeprom.read_bit()
            } else {
                self.cart.read_rom_16(eff_addr)
            };
            self.last_cycles = cyc;
            self.last_access_was_dma = false;
            return v;
        }
        if (region == 0x0E || region == 0x0F) && addr < 0x1000_0000 {
            self.prefetch.invalidate();
            self.rom_burst_active = false;
            let b = self.cart.read_sram(addr) as u16;
            self.last_cycles = self.sram_wait;
            self.last_access_was_dma = false;
            // 16-bit read from 8-bit SRAM bus mirrors byte across both halves
            return b | (b << 8);
        }

        let (val, cyc) = self.read_16_non_rom_raw(aligned, pc, is_thumb);
        self.step_prefetch(cyc);
        self.last_cycles = cyc;
        val
    }

    fn read_16_non_rom_raw(&mut self, aligned: u32, pc: u32, is_thumb: bool) -> (u16, u64) {
        match aligned >> 24 {
            0x00 => {
                if aligned < 0x4000 {
                    if pc < 0x4000 {
                        let base = (aligned as usize) & 0x3FFC;
                        let word = u32::from_le_bytes([
                            self.bios[base],
                            self.bios[base + 1],
                            self.bios[base + 2],
                            self.bios[base + 3],
                        ]);
                        self.bios_latch = word;
                        self.last_access_was_dma = false;
                        (((word >> ((aligned & 2) * 8)) & 0xFFFF) as u16, 1)
                    } else {
                        self.last_access_was_dma = false;
                        (
                            ((self.bios_latch >> ((aligned & 2) * 8)) & 0xFFFF) as u16,
                            1,
                        )
                    }
                } else {
                    (self.read_open_bus_16(aligned, pc, is_thumb), 1)
                }
            }
            0x02 => {
                let off = (aligned as usize) & 0x3FFFE;
                self.last_access_was_dma = false;
                (
                    u16::from_le_bytes([self.ewram[off], self.ewram[off + 1]]),
                    self.waitstate_table[0x02][1][0] as u64,
                )
            }
            0x03 => {
                let off = (aligned as usize) & 0x7FFE;
                self.last_access_was_dma = false;
                (
                    u16::from_le_bytes([self.iwram[off], self.iwram[off + 1]]),
                    1,
                )
            }
            0x04 => {
                let v = self.read_io_16(aligned, pc, is_thumb);
                (v, 1)
            }
            0x05 => {
                let off = (aligned as usize) & 0x3FE;
                self.last_access_was_dma = false;
                (
                    u16::from_le_bytes([self.ppu.pram[off], self.ppu.pram[off + 1]]),
                    1,
                )
            }
            0x06 => {
                let off = Ppu::mirror_vram(aligned as usize, self.ppu.dispcnt);
                self.last_access_was_dma = false;
                let val = match off {
                    Some(o) => {
                        let o = o & !1;
                        u16::from_le_bytes([self.ppu.vram[o], self.ppu.vram[o + 1]])
                    }
                    None => 0,
                };
                (val, 1)
            }
            0x07 => {
                let off = (aligned as usize) & 0x3FE;
                self.last_access_was_dma = false;
                (
                    u16::from_le_bytes([self.ppu.oam[off], self.ppu.oam[off + 1]]),
                    1,
                )
            }
            _ => (self.read_open_bus_16(aligned, pc, is_thumb), 1),
        }
    }

    #[inline]
    pub fn read_32(&mut self, addr: u32, access: AccessType, pc: u32) -> u32 {
        let val = self.read_32_inner(addr, access, pc);
        if self.watchpoint_active {
            self.check_watchpoint(addr & !3, 4, false, val);
        }
        if self.bench_active {
            self.read_bench_cycles += self.last_cycles;
        }
        val
    }

    fn read_32_inner(&mut self, addr: u32, access: AccessType, pc: u32) -> u32 {
        let aligned = addr & !3;
        if addr < 0x1000_0000 {
            let page = (aligned >> 14) as usize;
            if let Some(ptr) = self.fastmem_read[page] {
                let region = (aligned >> 24) & 0x0F;
                if (0x08..=0x0D).contains(&region) {
                    self.prefetch.invalidate();
                    self.force_next_fetch_nseq = true;
                    let (eff_lo, c0) = self.rom_step_burst(aligned, access);
                    let (eff_hi, c1) =
                        self.rom_step_burst(aligned.wrapping_add(2), AccessType::Sequential);
                    self.last_cycles = c0 + c1;
                    self.last_access_was_dma = false;
                    if eff_lo == aligned && eff_hi == aligned.wrapping_add(2) {
                        let off = (aligned & 0x3FFC) as usize;
                        return unsafe {
                            (ptr.as_ptr().add(off) as *const u32)
                                .read_unaligned()
                                .to_le()
                        };
                    }
                    let lo = self.read_rom_half_raw(eff_lo) as u32;
                    let hi = self.read_rom_half_raw(eff_hi) as u32;
                    return lo | (hi << 16);
                }
                let cyc = match region {
                    0x02 => self.waitstate_table[0x02][2][0] as u64,
                    0x03 => 1,
                    0x06 => 2,
                    _ => 1,
                };
                self.step_prefetch(cyc);
                self.last_cycles = cyc;
                self.last_access_was_dma = false;
                let off = (aligned & 0x3FFC) as usize;
                // SAFETY: `ptr` points to a valid 16 KiB page. `off` is `aligned & 0x3FFC`, which is at most
                // 0x3FFC. Reading 4 bytes accesses `0x3FFC..0x4000`, staying strictly inside the page.
                // We use `read_unaligned` to avoid any alignment assumptions on host architectures.
                return unsafe {
                    (ptr.as_ptr().add(off) as *const u32)
                        .read_unaligned()
                        .to_le()
                };
            }
        }

        let region = (aligned >> 24) & 0x0F;
        if (0x08..=0x0D).contains(&region) && addr < 0x1000_0000 {
            self.prefetch.invalidate();
            self.force_next_fetch_nseq = true;
            let (eff_lo, c0) = self.rom_step_burst(aligned, access);
            let (eff_hi, c1) = self.rom_step_burst(aligned.wrapping_add(2), AccessType::Sequential);
            let v = if self.cart.is_eeprom_addr(aligned) {
                self.cart.eeprom.read_bit() as u32
            } else if eff_lo == aligned && eff_hi == aligned.wrapping_add(2) {
                self.cart.read_rom_32(aligned)
            } else {
                let lo = self.cart.read_rom_16(eff_lo) as u32;
                let hi = self.cart.read_rom_16(eff_hi) as u32;
                lo | (hi << 16)
            };
            self.last_cycles = c0 + c1;
            self.last_access_was_dma = false;
            return v;
        }
        if (region == 0x0E || region == 0x0F) && addr < 0x1000_0000 {
            self.prefetch.invalidate();
            self.rom_burst_active = false;
            let b = self.cart.read_sram(addr) as u32;
            self.last_cycles = self.sram_wait;
            self.last_access_was_dma = false;
            return b * 0x0101_0101;
        }

        let (val, cyc) = self.read_32_non_rom_raw(aligned, pc, false);
        self.step_prefetch(cyc);
        self.last_cycles = cyc;
        val
    }

    fn read_32_non_rom_raw(&mut self, aligned: u32, pc: u32, is_thumb: bool) -> (u32, u64) {
        match aligned >> 24 {
            0x00 => {
                if aligned < 0x4000 {
                    if pc < 0x4000 {
                        let off = (aligned as usize) & 0x3FFC;
                        let word = u32::from_le_bytes([
                            self.bios[off],
                            self.bios[off + 1],
                            self.bios[off + 2],
                            self.bios[off + 3],
                        ]);
                        self.bios_latch = word;
                        self.last_access_was_dma = false;
                        (word, 1)
                    } else {
                        self.last_access_was_dma = false;
                        (self.bios_latch, 1)
                    }
                } else {
                    (self.read_open_bus_32(aligned, pc, is_thumb), 1)
                }
            }
            0x02 => {
                let off = (aligned as usize) & 0x3FFFC;
                self.last_access_was_dma = false;
                (
                    u32::from_le_bytes([
                        self.ewram[off],
                        self.ewram[off + 1],
                        self.ewram[off + 2],
                        self.ewram[off + 3],
                    ]),
                    self.waitstate_table[0x02][2][0] as u64,
                )
            }
            0x03 => {
                let off = (aligned as usize) & 0x7FFC;
                self.last_access_was_dma = false;
                (
                    u32::from_le_bytes([
                        self.iwram[off],
                        self.iwram[off + 1],
                        self.iwram[off + 2],
                        self.iwram[off + 3],
                    ]),
                    1,
                )
            }
            0x04 => {
                // Keep `last_access_was_dma` untouched until both halves are read so open-bus pairs work cleanly!
                let was_dma = self.last_access_was_dma;
                let lo = self.read_io_16(aligned, pc, is_thumb) as u32;
                self.last_access_was_dma = was_dma;
                let hi = self.read_io_16(aligned + 2, pc, is_thumb) as u32;
                (lo | (hi << 16), 1)
            }
            0x05 => {
                let off = (aligned as usize) & 0x3FC;
                self.last_access_was_dma = false;
                (
                    u32::from_le_bytes([
                        self.ppu.pram[off],
                        self.ppu.pram[off + 1],
                        self.ppu.pram[off + 2],
                        self.ppu.pram[off + 3],
                    ]),
                    2,
                )
            }
            0x06 => {
                let off = Ppu::mirror_vram(aligned as usize, self.ppu.dispcnt);
                self.last_access_was_dma = false;
                let val = match off {
                    Some(o) => {
                        let o = o & !3;
                        u32::from_le_bytes([
                            self.ppu.vram[o],
                            self.ppu.vram[o + 1],
                            self.ppu.vram[o + 2],
                            self.ppu.vram[o + 3],
                        ])
                    }
                    None => 0,
                };
                (val, 2)
            }
            0x07 => {
                let off = (aligned as usize) & 0x3FC;
                self.last_access_was_dma = false;
                (
                    u32::from_le_bytes([
                        self.ppu.oam[off],
                        self.ppu.oam[off + 1],
                        self.ppu.oam[off + 2],
                        self.ppu.oam[off + 3],
                    ]),
                    1,
                )
            }
            _ => (self.read_open_bus_32(aligned, pc, is_thumb), 1),
        }
    }

    pub fn read_16_dma(&mut self, addr: u32, access: AccessType) -> (u16, u64) {
        let v = self.read_16(addr, access, 0x0800_0000, false);
        (v, self.last_cycles)
    }

    pub fn read_32_dma(&mut self, addr: u32, access: AccessType) -> (u32, u64) {
        let v = self.read_32(addr, access, 0x0800_0000);
        (v, self.last_cycles)
    }

    #[inline]
    pub fn write_8(&mut self, addr: u32, val: u8, access: AccessType) -> u64 {
        let cyc = self.write_8_inner(addr, val, access);
        if self.watchpoint_active {
            self.check_watchpoint(addr, 1, true, val as u32);
        }
        if self.bench_active {
            self.write_bench_cycles += cyc;
        }
        cyc
    }

    fn write_8_inner(&mut self, addr: u32, val: u8, access: AccessType) -> u64 {
        if addr < 0x1000_0000 {
            let page = (addr >> 14) as usize;
            if let Some(ptr) = self.fastmem_write[page] {
                let region = (addr >> 24) & 0x0F;
                let cyc = if region == 0x02 {
                    self.waitstate_table[0x02][0][0] as u64
                } else {
                    1
                };
                self.step_prefetch(cyc);
                self.last_cycles = cyc;
                self.last_access_was_dma = false;
                // SAFETY: `page` has an active `fastmem_write` pointer only for EWRAM or IWRAM.
                // `addr & 0x3FFF` is within `0..0x4000`, strictly within the 16 KiB page.
                unsafe {
                    *ptr.as_ptr().add((addr & 0x3FFF) as usize) = val;
                }
                self.invalidate_code_page_if_aliased(region, page as u32);
                return cyc;
            }
        }

        let region = (addr >> 24) & 0x0F;
        self.last_access_was_dma = false;
        let cyc = match addr >> 24 {
            0x02 => {
                self.ewram[(addr as usize) & 0x3FFFF] = val;
                self.invalidate_code_page_if_aliased(0x02, (addr >> 14) & 0x3FFF);
                self.waitstate_table[0x02][0][0] as u64
            }
            0x03 => {
                self.iwram[(addr as usize) & 0x7FFF] = val;
                self.invalidate_code_page_if_aliased(0x03, (addr >> 14) & 0x3FFF);
                1
            }
            0x04 => {
                self.write_io_8(addr, val);
                1
            }
            0x05 => {
                self.sync_ppu_hdraw();
                // 8-bit writes to PRAM duplicate the byte into both halves of the aligned halfword!
                let off = (addr as usize) & 0x3FE;
                self.ppu.pram[off] = val;
                self.ppu.pram[off + 1] = val;
                1
            }
            0x06 => {
                self.sync_ppu_hdraw();
                // 8-bit writes to BG VRAM only (OBJ VRAM starts at 0x14000 in bitmap modes, 0x10000 in tile modes).
                if let Some(off) = Ppu::mirror_vram(addr as usize, self.ppu.dispcnt) {
                    let obj_start = if (self.ppu.dispcnt & 0x07) >= 3 {
                        0x14000
                    } else {
                        0x10000
                    };
                    if off < obj_start {
                        let aligned = off & !1;
                        self.ppu.vram[aligned] = val;
                        self.ppu.vram[aligned + 1] = val;
                    }
                }
                1
            }
            0x07 => {
                // 8-bit writes to OAM are completely ignored by GBA hardware!
                1
            }
            0x08..=0x0D => {
                self.prefetch.invalidate();
                self.force_next_fetch_nseq = true;
                self.rom_step_burst(addr & !1, access).1
            }
            0x0E | 0x0F if addr < 0x1000_0000 => {
                self.prefetch.invalidate();
                self.rom_burst_active = false;
                self.cart.write_sram(addr, val);
                self.sram_wait
            }
            _ => 1,
        };
        if region < 0x08 || addr >= 0x1000_0000 {
            self.step_prefetch(cyc);
        }
        self.last_cycles = cyc;
        cyc
    }

    #[inline]
    pub fn write_16(&mut self, addr: u32, val: u16, access: AccessType) -> u64 {
        let cyc = self.write_16_inner(addr, val, access);
        if self.watchpoint_active {
            self.check_watchpoint(addr & !1, 2, true, val as u32);
        }
        if self.bench_active {
            self.write_bench_cycles += cyc;
        }
        cyc
    }

    fn write_16_inner(&mut self, addr: u32, val: u16, access: AccessType) -> u64 {
        let aligned = addr & !1;
        if aligned < 0x1000_0000 {
            let page = (aligned >> 14) as usize;
            if let Some(ptr) = self.fastmem_write[page] {
                let region = (aligned >> 24) & 0x0F;
                let cyc = if region == 0x02 {
                    self.waitstate_table[0x02][1][0] as u64
                } else {
                    1
                };
                self.step_prefetch(cyc);
                self.last_cycles = cyc;
                self.last_access_was_dma = false;
                // SAFETY: `ptr` points to a mutable 16 KiB page for EWRAM or IWRAM.
                // `aligned & 0x3FFF` is at most 0x3FFE. Writing 2 bytes via `write_unaligned` stays within
                // the page boundary.
                unsafe {
                    let dest = ptr.as_ptr().add((aligned & 0x3FFF) as usize) as *mut u16;
                    dest.write_unaligned(val.to_le());
                }
                self.invalidate_code_page_if_aliased(region, page as u32);
                return cyc;
            }
        }

        let region = (aligned >> 24) & 0x0F;
        self.last_access_was_dma = false;
        let cyc = match aligned >> 24 {
            0x02 => {
                let off = (aligned as usize) & 0x3FFFE;
                unsafe {
                    (self.ewram.as_mut_ptr().add(off) as *mut u16).write_unaligned(val.to_le());
                }
                self.invalidate_code_page_if_aliased(0x02, (aligned >> 14) & 0x3FFF);
                self.waitstate_table[0x02][1][0] as u64
            }
            0x03 => {
                let off = (aligned as usize) & 0x7FFE;
                unsafe {
                    (self.iwram.as_mut_ptr().add(off) as *mut u16).write_unaligned(val.to_le());
                }
                self.invalidate_code_page_if_aliased(0x03, (aligned >> 14) & 0x3FFF);
                1
            }
            0x04 => {
                self.write_io_16(aligned, val);
                1
            }
            0x05 => {
                self.sync_ppu_hdraw();
                let off = (aligned as usize) & 0x3FE;
                unsafe {
                    (self.ppu.pram.as_mut_ptr().add(off) as *mut u16).write_unaligned(val.to_le());
                }
                1
            }
            0x06 => {
                self.sync_ppu_hdraw();
                if let Some(off) = Ppu::mirror_vram(aligned as usize, self.ppu.dispcnt) {
                    let off = off & !1;
                    unsafe {
                        (self.ppu.vram.as_mut_ptr().add(off) as *mut u16)
                            .write_unaligned(val.to_le());
                    }
                }
                1
            }
            0x07 => {
                self.sync_ppu_hdraw();
                let off = (aligned as usize) & 0x3FE;
                unsafe {
                    (self.ppu.oam.as_mut_ptr().add(off) as *mut u16).write_unaligned(val.to_le());
                }
                1
            }
            0x08..=0x0D => {
                self.prefetch.invalidate();
                self.force_next_fetch_nseq = true;
                let (eff_addr, cyc) = self.rom_step_burst(aligned, access);
                if self.cart.is_eeprom_addr(aligned) {
                    self.cart.eeprom.write_bit(
                        &mut self.cart.sram,
                        &mut self.cart.save_dirty,
                        val,
                        0,
                    );
                } else {
                    self.cart.write_rom_16(eff_addr, val);
                }
                cyc
            }
            0x0E | 0x0F => {
                self.prefetch.invalidate();
                self.rom_burst_active = false;
                // 16-bit write to SRAM writes the low byte corresponding to addr & 1 (shifted!)
                let byte = (val >> ((addr & 1) * 8)) as u8;
                self.cart.write_sram(addr, byte);
                self.sram_wait
            }
            _ => 1,
        };
        if region < 0x08 || aligned >= 0x1000_0000 {
            self.step_prefetch(cyc);
        }
        self.last_cycles = cyc;
        cyc
    }

    #[inline]
    pub fn write_32(&mut self, addr: u32, val: u32, access: AccessType) -> u64 {
        let cyc = self.write_32_inner(addr, val, access);
        if self.watchpoint_active {
            self.check_watchpoint(addr & !3, 4, true, val);
        }
        if self.bench_active {
            self.write_bench_cycles += cyc;
        }
        cyc
    }

    fn write_32_inner(&mut self, addr: u32, val: u32, access: AccessType) -> u64 {
        let aligned = addr & !3;
        if aligned < 0x1000_0000 {
            let page = (aligned >> 14) as usize;
            if let Some(ptr) = self.fastmem_write[page] {
                let region = (aligned >> 24) & 0x0F;
                let cyc = if region == 0x02 {
                    self.waitstate_table[0x02][2][0] as u64
                } else {
                    1
                };
                self.step_prefetch(cyc);
                self.last_cycles = cyc;
                self.last_access_was_dma = false;
                // SAFETY: `ptr` points to a mutable 16 KiB page for EWRAM or IWRAM.
                // `aligned & 0x3FFF` is at most 0x3FFC. Writing 4 bytes via `write_unaligned` stays within
                // the page boundary.
                unsafe {
                    let dest = ptr.as_ptr().add((aligned & 0x3FFF) as usize) as *mut u32;
                    dest.write_unaligned(val.to_le());
                }
                self.invalidate_code_page_if_aliased(region, page as u32);
                return cyc;
            }
        }

        let region = (aligned >> 24) & 0x0F;
        self.last_access_was_dma = false;
        let cyc = match aligned >> 24 {
            0x02 => {
                let off = (aligned as usize) & 0x3FFFC;
                unsafe {
                    (self.ewram.as_mut_ptr().add(off) as *mut u32).write_unaligned(val.to_le());
                }
                self.invalidate_code_page_if_aliased(0x02, (aligned >> 14) & 0x3FFF);
                self.waitstate_table[0x02][2][0] as u64
            }
            0x03 => {
                let off = (aligned as usize) & 0x7FFC;
                unsafe {
                    (self.iwram.as_mut_ptr().add(off) as *mut u32).write_unaligned(val.to_le());
                }
                self.invalidate_code_page_if_aliased(0x03, (aligned >> 14) & 0x3FFF);
                1
            }
            0x04 => {
                let off = aligned & 0x0FFF_FFFF;
                if off == 0x0400_00A0 {
                    self.apu.fifo_a.push_word(val);
                } else if off == 0x0400_00A4 {
                    self.apu.fifo_b.push_word(val);
                } else {
                    self.write_io_16(aligned, val as u16);
                    self.write_io_16(aligned + 2, (val >> 16) as u16);
                }
                1
            }
            0x05 => {
                self.sync_ppu_hdraw();
                let off = (aligned as usize) & 0x3FC;
                unsafe {
                    (self.ppu.pram.as_mut_ptr().add(off) as *mut u32).write_unaligned(val.to_le());
                }
                2
            }
            0x06 => {
                self.sync_ppu_hdraw();
                if let Some(off) = Ppu::mirror_vram(aligned as usize, self.ppu.dispcnt) {
                    let off = off & !3;
                    unsafe {
                        (self.ppu.vram.as_mut_ptr().add(off) as *mut u32)
                            .write_unaligned(val.to_le());
                    }
                }
                2
            }
            0x07 => {
                self.sync_ppu_hdraw();
                let off = (aligned as usize) & 0x3FC;
                unsafe {
                    (self.ppu.oam.as_mut_ptr().add(off) as *mut u32).write_unaligned(val.to_le());
                }
                1
            }
            0x08..=0x0D => {
                self.prefetch.invalidate();
                self.force_next_fetch_nseq = true;
                let (eff_lo, c0) = self.rom_step_burst(aligned, access);
                let (eff_hi, c1) =
                    self.rom_step_burst(aligned.wrapping_add(2), AccessType::Sequential);
                self.cart.write_rom_16(eff_lo, val as u16);
                self.cart.write_rom_16(eff_hi, (val >> 16) as u16);
                c0 + c1
            }
            0x0E | 0x0F => {
                self.prefetch.invalidate();
                self.rom_burst_active = false;
                let byte = (val >> ((addr & 3) * 8)) as u8;
                self.cart.write_sram(addr, byte);
                self.sram_wait
            }
            _ => 1,
        };
        if region < 0x08 || aligned >= 0x1000_0000 {
            self.step_prefetch(cyc);
        }
        self.last_cycles = cyc;
        cyc
    }

    fn debug_read_io_16(&self, addr: u32) -> u16 {
        let off = addr & 0x0FFF_FFFE;
        if off >= 0x0400_0400 {
            return match off & 0xFFFF {
                0x0800 => (self.hw.memcnt & 0xFFFF) as u16,
                0x0802 => (self.hw.memcnt >> 16) as u16,
                _ => 0,
            };
        }
        match off {
            0x0400_0000 => self.ppu.dispcnt,
            0x0400_0002 => self.ppu.greenswap,
            0x0400_0004 => self.ppu.dispstat,
            0x0400_0006 => self.ppu.vcount,
            0x0400_0008 => self.ppu.bgcnt[0],
            0x0400_000A => self.ppu.bgcnt[1],
            0x0400_000C => self.ppu.bgcnt[2],
            0x0400_000E => self.ppu.bgcnt[3],
            0x0400_0048 => self.ppu.winin,
            0x0400_004A => self.ppu.winout,
            0x0400_0050 => self.ppu.bldcnt,
            0x0400_0052 => self.ppu.bldalpha,
            0x0400_0060 => self.apu.sq1.reg_l & 0x007F,
            0x0400_0062 => self.apu.sq1.reg_h & 0xFFC0,
            0x0400_0064 => self.apu.sq1.reg_x & 0x4000,
            0x0400_0068 => self.apu.sq2.reg_h & 0xFFC0,
            0x0400_006C => self.apu.sq2.reg_x & 0x4000,
            0x0400_0070 => self.apu.wave.reg_l & 0x00E0,
            0x0400_0072 => self.apu.wave.reg_h & 0xE000,
            0x0400_0074 => self.apu.wave.reg_x & 0x4000,
            0x0400_0078 => self.apu.noise.reg_l & 0xFF00,
            0x0400_007C => self.apu.noise.reg_h & 0x40FF,
            0x0400_0080 => self.apu.soundcnt_l,
            0x0400_0082 => self.apu.soundcnt_h,
            0x0400_0084 => self.apu.read_soundcnt_x(),
            0x0400_0088 => self.apu.soundbias,
            0x0400_0090..=0x0400_009E => {
                let idx = (off - 0x0400_0090) as usize;
                u16::from_le_bytes([
                    self.apu.wave.read_wave_ram(idx),
                    self.apu.wave.read_wave_ram(idx + 1),
                ])
            }
            0x0400_00BA => self.hw.dma[0].cnt & 0xF7E0,
            0x0400_00C6 => self.hw.dma[1].cnt & 0xF7E0,
            0x0400_00D2 => self.hw.dma[2].cnt & 0xF7E0,
            0x0400_00DE => self.hw.dma[3].cnt & 0xFFE0,
            0x0400_0100 => self
                .hw
                .current_timer_counter(0, self.scheduler.current_cycle),
            0x0400_0102 => self.hw.timers[0].cnt,
            0x0400_0104 => self
                .hw
                .current_timer_counter(1, self.scheduler.current_cycle),
            0x0400_0106 => self.hw.timers[1].cnt,
            0x0400_0108 => self
                .hw
                .current_timer_counter(2, self.scheduler.current_cycle),
            0x0400_010A => self.hw.timers[2].cnt,
            0x0400_010C => self
                .hw
                .current_timer_counter(3, self.scheduler.current_cycle),
            0x0400_010E => self.hw.timers[3].cnt,
            0x0400_0120 => self.hw.siomulti[0],
            0x0400_0122 => self.hw.siomulti[1],
            0x0400_0124 => self.hw.siomulti[2],
            0x0400_0126 => self.hw.siomulti[3],
            0x0400_0128 => self.hw.siocnt,
            0x0400_012A => self.hw.siodata8,
            0x0400_0130 => self.hw.keyinput,
            0x0400_0132 => self.hw.keycnt,
            0x0400_0134 => self.hw.rcnt & 0xC1FF,
            0x0400_0200 => self.hw.ie,
            0x0400_0202 => self.hw.if_flag,
            0x0400_0204 => self.hw.waitcnt & 0x7FFF,
            0x0400_0208 => self.hw.ime & 1,
            0x0400_0300 => self.hw.postflg as u16,
            _ => 0,
        }
    }

    /// Side-effect-free read for Hex Viewer, Disassembler, and Cheat Condition Checks.
    pub fn debug_read_8(&self, addr: u32) -> u8 {
        match (addr >> 24) & 0xFF {
            0x00 => {
                if addr < 0x4000 {
                    self.bios[addr as usize]
                } else {
                    0
                }
            }
            0x02 => self.ewram[(addr as usize) & 0x3FFFF],
            0x03 => self.iwram[(addr as usize) & 0x7FFF],
            0x04 => {
                let v16 = self.debug_read_io_16(addr);
                ((v16 >> ((addr & 1) * 8)) & 0xFF) as u8
            }
            0x05 => self.ppu.pram[(addr as usize) & 0x3FF],
            0x06 => Ppu::mirror_vram(addr as usize, self.ppu.dispcnt)
                .map(|o| self.ppu.vram[o])
                .unwrap_or(0),
            0x07 => self.ppu.oam[(addr as usize) & 0x3FF],
            0x08..=0x0D => {
                let v16 = self.cart.read_rom_16(addr & !1);
                ((v16 >> ((addr & 1) * 8)) & 0xFF) as u8
            }
            0x0E | 0x0F if addr < 0x1000_0000 => self.cart.read_sram(addr),
            _ => 0,
        }
    }

    pub fn debug_read_16(&self, addr: u32) -> u16 {
        let a = addr & !1;
        u16::from_le_bytes([self.debug_read_8(a), self.debug_read_8(a.wrapping_add(1))])
    }

    pub fn debug_read_32(&self, addr: u32) -> u32 {
        let a = addr & !3;
        u32::from_le_bytes([
            self.debug_read_8(a),
            self.debug_read_8(a.wrapping_add(1)),
            self.debug_read_8(a.wrapping_add(2)),
            self.debug_read_8(a.wrapping_add(3)),
        ])
    }

    /// Side-effect-free write for Lua/GDB/debug `poke*` APIs; ignores IO so no side effects trigger.
    pub fn debug_write_8(&mut self, addr: u32, val: u8) {
        match (addr >> 24) & 0xFF {
            0x02 => {
                self.ewram[(addr as usize) & 0x3FFFF] = val;
                self.invalidate_code_page_if_aliased(0x02, (addr >> 14) & 0x3FFF);
            }
            0x03 => {
                self.iwram[(addr as usize) & 0x7FFF] = val;
                self.invalidate_code_page_if_aliased(0x03, (addr >> 14) & 0x3FFF);
            }
            0x05 => self.ppu.pram[(addr as usize) & 0x3FF] = val,
            0x06 => {
                if let Some(off) = Ppu::mirror_vram(addr as usize, self.ppu.dispcnt) {
                    self.ppu.vram[off] = val;
                }
            }
            0x07 => self.ppu.oam[(addr as usize) & 0x3FF] = val,
            0x08..=0x0D => {
                let off = (addr as usize) & 0x01FF_FFFF;
                if off < self.cart.rom.len() {
                    self.cart.rom[off] = val;
                    self.invalidate_code_page_if_aliased(
                        (addr >> 24) & 0x0F,
                        (addr >> 14) & 0x3FFF,
                    );
                }
            }
            0x0E | 0x0F if addr < 0x1000_0000 => self.cart.debug_write_sram(addr, val),
            _ => {}
        }
    }

    pub fn debug_write_16(&mut self, addr: u32, val: u16) {
        let a = addr & !1;
        let bytes = val.to_le_bytes();
        self.debug_write_8(a, bytes[0]);
        self.debug_write_8(a.wrapping_add(1), bytes[1]);
    }

    pub fn debug_write_32(&mut self, addr: u32, val: u32) {
        let a = addr & !3;
        let bytes = val.to_le_bytes();
        self.debug_write_8(a, bytes[0]);
        self.debug_write_8(a.wrapping_add(1), bytes[1]);
        self.debug_write_8(a.wrapping_add(2), bytes[2]);
        self.debug_write_8(a.wrapping_add(3), bytes[3]);
    }

    /// Restores any ROM bytes patched via [`Self::apply_cheat_write_8`].
    pub fn restore_rom_patches(&mut self) {
        if self.rom_patch_originals.is_empty() {
            return;
        }
        let originals = core::mem::take(&mut self.rom_patch_originals);
        for (off, orig) in originals {
            if off < self.cart.rom.len() {
                self.cart.rom[off] = orig;
                let page = (0x08 << 10) | ((off as u32) >> 14);
                self.invalidate_code_page_if_aliased(0x08, page);
            }
        }
    }

    /// Writes a cheat value into RAM, Video RAM, or ROM memory safely.
    pub fn apply_cheat_write_8(&mut self, addr: u32, val: u8) {
        match (addr >> 24) & 0xFF {
            0x02 => {
                self.ewram[(addr as usize) & 0x3FFFF] = val;
                self.invalidate_code_page_if_aliased(0x02, (addr >> 14) & 0x3FFF);
            }
            0x03 => {
                self.iwram[(addr as usize) & 0x7FFF] = val;
                self.invalidate_code_page_if_aliased(0x03, (addr >> 14) & 0x3FFF);
            }
            0x04 => self.write_io_8(addr, val),
            0x05 => self.ppu.pram[(addr as usize) & 0x3FF] = val,
            0x06 => {
                if let Some(off) = Ppu::mirror_vram(addr as usize, self.ppu.dispcnt) {
                    self.ppu.vram[off] = val;
                }
            }
            0x07 => self.ppu.oam[(addr as usize) & 0x3FF] = val,
            0x08..=0x0D => {
                let off = (addr as usize) & 0x01FF_FFFF;
                if off < self.cart.rom.len() {
                    self.rom_patch_originals
                        .entry(off)
                        .or_insert(self.cart.rom[off]);
                    self.cart.rom[off] = val;
                    self.invalidate_code_page_if_aliased(
                        (addr >> 24) & 0x0F,
                        (addr >> 14) & 0x3FFF,
                    );
                }
            }
            _ => {}
        }
    }

    pub fn apply_cheat_write_16(&mut self, addr: u32, val: u16) {
        let a = addr & !1;
        let bytes = val.to_le_bytes();
        self.apply_cheat_write_8(a, bytes[0]);
        self.apply_cheat_write_8(a.wrapping_add(1), bytes[1]);
    }

    pub fn apply_cheat_write_32(&mut self, addr: u32, val: u32) {
        let a = addr & !3;
        let bytes = val.to_le_bytes();
        self.apply_cheat_write_8(a, bytes[0]);
        self.apply_cheat_write_8(a.wrapping_add(1), bytes[1]);
        self.apply_cheat_write_8(a.wrapping_add(2), bytes[2]);
        self.apply_cheat_write_8(a.wrapping_add(3), bytes[3]);
    }
}
