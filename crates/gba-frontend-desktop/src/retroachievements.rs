//! RetroAchievements runtime: GBA ROM MD5 hashing, `MemAddr` condition VM, async
//! `dorequest.php` web API client, Hardcore mode rules, and HUD unlock toast banner.

use std::{
    collections::VecDeque,
    sync::mpsc::{self, Receiver, Sender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Stroke, Vec2};
use gba_core::GbaCore;
use md5::{Digest, Md5};
use serde::Deserialize;

use crate::config::AppConfig;

const RA_API_URL: &str = "https://retroachievements.org/dorequest.php";
const RA_USER_AGENT: &str = "RustboyAdvance/0.1.0 (GBA Emulator; RetroAchievements)";
const RICH_PRESENCE_INTERVAL: Duration = Duration::from_secs(120);
const TOAST_DURATION: Duration = Duration::from_secs(5);

/// Computes the canonical lowercase hexadecimal MD5 digest of a GBA ROM buffer.
pub fn hash_rom_md5(rom_bytes: &[u8]) -> String {
    let digest = Md5::digest(rom_bytes);
    let mut out = String::with_capacity(32);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

/// Maps a RetroAchievements GBA address onto the GBA CPU bus.
///
/// Official RetroAchievements GBA memory layout:
/// - `0x000000..=0x03FFFF` -> EWRAM `0x0200_0000..=0x0203_FFFF`
/// - `0x040000..=0x047FFF` -> IWRAM `0x0300_0000..=0x0300_7FFF`
/// - Raw bus addresses (`>= 0x0200_0000`, e.g. `0x02xxxxxx`, `0x03xxxxxx`) pass through directly.
pub fn map_ra_gba_addr(addr: u32) -> u32 {
    if addr <= 0x0003_FFFF {
        0x0200_0000 + addr
    } else if (0x0004_0000..=0x0004_7FFF).contains(&addr) {
        0x0300_0000 + (addr - 0x0004_0000)
    } else {
        addr
    }
}

/// Supported RetroAchievements `MemAddr` memory access sizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemSize {
    Lower4,
    Upper4,
    Bits8,
    Bits16,
    Bits24,
    Bits32,
    Bits16BE,
    Bits24BE,
    Bits32BE,
    Bit(u8),
    BitCount,
}

impl MemSize {
    fn read_from_core(self, core: &GbaCore, gba_addr: u32) -> u32 {
        match self {
            Self::Lower4 => u32::from(core.peek8(gba_addr) & 0x0F),
            Self::Upper4 => u32::from((core.peek8(gba_addr) >> 4) & 0x0F),
            Self::Bits8 => u32::from(core.peek8(gba_addr)),
            Self::Bits16 => u32::from(core.peek16(gba_addr)),
            Self::Bits24 => {
                u32::from(core.peek8(gba_addr))
                    | (u32::from(core.peek8(gba_addr.wrapping_add(1))) << 8)
                    | (u32::from(core.peek8(gba_addr.wrapping_add(2))) << 16)
            }
            Self::Bits32 => core.peek32(gba_addr),
            Self::Bits16BE => u32::from(core.peek16(gba_addr).swap_bytes()),
            Self::Bits24BE => {
                (u32::from(core.peek8(gba_addr)) << 16)
                    | (u32::from(core.peek8(gba_addr.wrapping_add(1))) << 8)
                    | u32::from(core.peek8(gba_addr.wrapping_add(2)))
            }
            Self::Bits32BE => core.peek32(gba_addr).swap_bytes(),
            Self::Bit(bit) => u32::from((core.peek8(gba_addr) >> (bit & 7)) & 1),
            Self::BitCount => core.peek8(gba_addr).count_ones(),
        }
    }

    fn value_mask(self) -> u32 {
        match self {
            Self::Lower4 | Self::Upper4 => 0x0F,
            Self::Bits8 | Self::BitCount => 0xFF,
            Self::Bits16 | Self::Bits16BE => 0xFFFF,
            Self::Bits24 | Self::Bits24BE => 0x00FF_FFFF,
            Self::Bits32 | Self::Bits32BE => 0xFFFF_FFFF,
            Self::Bit(_) => 1,
        }
    }
}

/// Whether a memory operand reads current frame, previous frame (`d`), or prior changed (`p`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemValueMode {
    Current,
    Delta,
    Prior,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemRef {
    pub addr: u32,
    pub size: MemSize,
    pub mode: MemValueMode,
    pub bcd: bool,
    pub invert: bool,
}

#[derive(Clone, Debug, Default)]
struct MemRefHistory {
    current: u32,
    delta: u32,
    prior: u32,
    initialized: bool,
}

impl MemRefHistory {
    fn update(&mut self, value: u32) {
        if !self.initialized {
            self.current = value;
            self.delta = 0;
            self.prior = 0;
            self.initialized = true;
            return;
        }
        self.delta = self.current;
        if value != self.current {
            self.prior = self.current;
            self.current = value;
        }
    }

    fn read(&self, mode: MemValueMode) -> u32 {
        match mode {
            MemValueMode::Current => self.current,
            MemValueMode::Delta => self.delta,
            MemValueMode::Prior => self.prior,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArithOp {
    BitAnd,
    Mul,
    Div,
    Add,
    Sub,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OperandExpr {
    Mem {
        mem: MemRef,
        history_idx: usize,
    },
    Const(i64),
    UnaryNeg(Box<OperandExpr>),
    Binary {
        op: ArithOp,
        lhs: Box<OperandExpr>,
        rhs: Box<OperandExpr>,
    },
}

impl OperandExpr {
    fn collect_memrefs(&self, out: &mut Vec<(usize, MemRef)>) {
        match self {
            Self::Mem { mem, history_idx } => out.push((*history_idx, mem.clone())),
            Self::Const(_) => {}
            Self::UnaryNeg(inner) => inner.collect_memrefs(out),
            Self::Binary { lhs, rhs, .. } => {
                lhs.collect_memrefs(out);
                rhs.collect_memrefs(out);
            }
        }
    }

    fn eval(&self, core: &GbaCore, histories: &[MemRefHistory], addr_offset: Option<u32>) -> i64 {
        match self {
            Self::Mem { mem, history_idx } => {
                let mut raw = if let Some(off) = addr_offset {
                    let effective = map_ra_gba_addr(off.wrapping_add(mem.addr));
                    mem.size.read_from_core(core, effective)
                } else {
                    histories
                        .get(*history_idx)
                        .map(|h| h.read(mem.mode))
                        .unwrap_or(0)
                };
                if mem.invert {
                    raw = (!raw) & mem.size.value_mask();
                }
                if mem.bcd {
                    raw = decode_bcd(raw);
                }
                i64::from(raw)
            }
            Self::Const(v) => *v,
            Self::UnaryNeg(inner) => -inner.eval(core, histories, addr_offset),
            Self::Binary { op, lhs, rhs } => {
                let l = lhs.eval(core, histories, addr_offset);
                let r = rhs.eval(core, histories, addr_offset);
                match op {
                    ArithOp::BitAnd => l & r,
                    ArithOp::Mul => l.wrapping_mul(r),
                    ArithOp::Div => {
                        if r == 0 {
                            0
                        } else {
                            l.wrapping_div(r)
                        }
                    }
                    ArithOp::Add => l.wrapping_add(r),
                    ArithOp::Sub => l.wrapping_sub(r),
                }
            }
        }
    }
}

fn decode_bcd(mut v: u32) -> u32 {
    let mut out = 0u32;
    let mut mul = 1u32;
    while v > 0 {
        out = out.saturating_add((v & 0x0F).saturating_mul(mul));
        mul = mul.saturating_mul(10);
        v >>= 4;
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpOp {
    fn eval(self, lhs: i64, rhs: i64) -> bool {
        match self {
            Self::Eq => lhs == rhs,
            Self::Ne => lhs != rhs,
            Self::Lt => lhs < rhs,
            Self::Le => lhs <= rhs,
            Self::Gt => lhs > rhs,
            Self::Ge => lhs >= rhs,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CondFlag {
    Standard,
    ResetIf,
    PauseIf,
    AndNext,
    OrNext,
    AddSource,
    SubSource,
    AddHits,
    SubHits,
    AddAddress,
    Trigger,
    Measured,
    MeasuredIf,
    ResetNextIf,
}

#[derive(Clone, Debug)]
pub struct Condition {
    pub flag: CondFlag,
    pub lhs: OperandExpr,
    pub cmp: Option<(CmpOp, OperandExpr)>,
    pub hit_target: u32,
    pub hits: u32,
}

#[derive(Clone, Debug)]
pub struct ConditionGroup {
    pub conditions: Vec<Condition>,
    pub paused: bool,
}

/// Compiled RetroAchievements trigger AST (`Core` group + optional `Alt` groups separated by `S`).
#[derive(Clone, Debug)]
pub struct TriggerProgram {
    pub groups: Vec<ConditionGroup>,
    histories: Vec<MemRefHistory>,
    mem_slots: Vec<(usize, MemRef)>,
}

/// Evaluation result for a single frame of a `TriggerProgram`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TriggerEvalResult {
    pub triggered: bool,
    pub reset_fired: bool,
    pub measured_current: Option<i64>,
    pub measured_target: Option<i64>,
}

impl TriggerProgram {
    /// Parses a RetroAchievements `MemAddr` trigger string into a stateful evaluator.
    pub fn parse(mem_addr: &str) -> Result<Self, String> {
        let trimmed = mem_addr.trim();
        if trimmed.is_empty() {
            return Err("empty MemAddr string".to_string());
        }

        let group_strs = split_alt_groups(trimmed);
        let mut groups = Vec::with_capacity(group_strs.len());
        let mut next_history_idx = 0usize;

        for g in group_strs {
            let mut conditions = Vec::new();
            for raw_cond in g.split('_') {
                let cond_str = raw_cond.trim();
                if cond_str.is_empty() {
                    continue;
                }
                conditions.push(parse_condition(cond_str, &mut next_history_idx)?);
            }
            if !conditions.is_empty() {
                groups.push(ConditionGroup {
                    conditions,
                    paused: false,
                });
            }
        }

        if groups.is_empty() {
            return Err("MemAddr contained no conditions".to_string());
        }

        let mut mem_slots = Vec::new();
        for g in &groups {
            for cond in &g.conditions {
                cond.lhs.collect_memrefs(&mut mem_slots);
                if let Some((_, rhs)) = &cond.cmp {
                    rhs.collect_memrefs(&mut mem_slots);
                }
            }
        }
        mem_slots.sort_by_key(|(idx, _)| *idx);

        Ok(Self {
            groups,
            histories: vec![MemRefHistory::default(); next_history_idx],
            mem_slots,
        })
    }

    #[allow(dead_code)]
    pub fn reset(&mut self) {
        for group in &mut self.groups {
            group.paused = false;
            for cond in &mut group.conditions {
                cond.hits = 0;
            }
        }
        for h in &mut self.histories {
            *h = MemRefHistory::default();
        }
    }

    /// Evaluates one emulated frame against `core`, updating delta/prior histories and hit counts.
    pub fn evaluate_frame(&mut self, core: &GbaCore) -> TriggerEvalResult {
        // Step 1: Snapshot all referenced memory locations and advance Delta/Prior history.
        for (idx, mem) in &self.mem_slots {
            let gba_addr = map_ra_gba_addr(mem.addr);
            let val = mem.size.read_from_core(core, gba_addr);
            if let Some(h) = self.histories.get_mut(*idx) {
                h.update(val);
            }
        }

        let mut reset_all = false;
        let mut measured_current = None;
        let mut measured_target = None;
        let mut measured_if_ok = true;
        let mut group_met = Vec::with_capacity(self.groups.len());

        // Step 2: Evaluate each group.
        for group in &mut self.groups {
            let res = evaluate_group(group, core, &self.histories);
            group.paused = res.paused;
            if res.reset_all {
                reset_all = true;
            }
            if !res.measured_if_ok {
                measured_if_ok = false;
            }
            if res.measured_current.is_some() {
                measured_current = res.measured_current;
                measured_target = res.measured_target;
            }
            group_met.push(res.met);
        }

        // Step 3: Apply global ResetIf across all conditions in the trigger.
        if reset_all {
            for group in &mut self.groups {
                for cond in &mut group.conditions {
                    cond.hits = 0;
                }
            }
            return TriggerEvalResult {
                triggered: false,
                reset_fired: true,
                measured_current: if measured_if_ok {
                    measured_current
                } else {
                    Some(0)
                },
                measured_target,
            };
        }

        // Core group (index 0) must pass, plus at least one Alt group if any alt groups exist.
        let core_ok = group_met.first().copied().unwrap_or(false);
        let alts_ok = if group_met.len() <= 1 {
            true
        } else {
            group_met[1..].iter().any(|ok| *ok)
        };

        TriggerEvalResult {
            triggered: core_ok && alts_ok,
            reset_fired: false,
            measured_current: if measured_if_ok {
                measured_current
            } else {
                Some(0)
            },
            measured_target,
        }
    }
}

struct GroupEvalOutcome {
    met: bool,
    paused: bool,
    reset_all: bool,
    measured_if_ok: bool,
    measured_current: Option<i64>,
    measured_target: Option<i64>,
}

#[derive(Clone, Copy)]
enum ChainLink {
    And(bool),
    Or(bool),
}

fn evaluate_group(
    group: &mut ConditionGroup,
    core: &GbaCore,
    histories: &[MemRefHistory],
) -> GroupEvalOutcome {
    // First pass: check PauseIf conditions so paused groups freeze all hit counters cleanly.
    let mut source_acc: i64 = 0;
    let mut addr_offset: Option<u32> = None;
    let mut chain: Option<ChainLink> = None;
    let mut raw_matches = Vec::with_capacity(group.conditions.len());
    let mut lhs_values = Vec::with_capacity(group.conditions.len());
    let mut rhs_values = Vec::with_capacity(group.conditions.len());
    let mut any_paused = false;

    for cond in &mut group.conditions {
        let raw_lhs = cond.lhs.eval(core, histories, addr_offset);
        addr_offset = None;

        match cond.flag {
            CondFlag::AddSource => {
                source_acc = source_acc.wrapping_add(raw_lhs);
                raw_matches.push(true);
                lhs_values.push(raw_lhs);
                rhs_values.push(0);
                continue;
            }
            CondFlag::SubSource => {
                source_acc = source_acc.wrapping_sub(raw_lhs);
                raw_matches.push(true);
                lhs_values.push(raw_lhs);
                rhs_values.push(0);
                continue;
            }
            CondFlag::AddAddress => {
                addr_offset = Some(raw_lhs as u32);
                raw_matches.push(true);
                lhs_values.push(raw_lhs);
                rhs_values.push(0);
                continue;
            }
            _ => {}
        }

        let effective_lhs = source_acc.wrapping_add(raw_lhs);
        source_acc = 0;

        let (raw_ok, rhs_val) = if let Some((cmp_op, rhs_expr)) = &cond.cmp {
            let r = rhs_expr.eval(core, histories, None);
            (cmp_op.eval(effective_lhs, r), r)
        } else {
            (effective_lhs != 0, 0)
        };

        let chained_ok = match chain {
            None => raw_ok,
            Some(ChainLink::And(prev)) => prev && raw_ok,
            Some(ChainLink::Or(prev)) => prev || raw_ok,
        };

        lhs_values.push(effective_lhs);
        rhs_values.push(rhs_val);
        raw_matches.push(chained_ok);

        match cond.flag {
            CondFlag::AndNext => {
                let step_val = if cond.hit_target > 0 {
                    // Track hits during active check below if unpaused; for PauseIf pass through.
                    chained_ok
                } else {
                    chained_ok
                };
                chain = Some(ChainLink::And(step_val));
            }
            CondFlag::OrNext => {
                chain = Some(ChainLink::Or(chained_ok));
            }
            CondFlag::PauseIf => {
                chain = None;
                let satisfied = if cond.hit_target > 0 {
                    if chained_ok && cond.hits < cond.hit_target {
                        cond.hits += 1;
                    }
                    cond.hits >= cond.hit_target
                } else {
                    chained_ok
                };
                if satisfied {
                    any_paused = true;
                }
            }
            _ => {
                chain = None;
            }
        }
    }

    if any_paused {
        return GroupEvalOutcome {
            met: false,
            paused: true,
            reset_all: false,
            measured_if_ok: true,
            measured_current: None,
            measured_target: None,
        };
    }

    // Second pass (group is unpaused): advance hitcounts, ResetNextIf, AddHits/SubHits, and check satisfaction.
    let mut reset_all = false;
    let mut reset_next = false;
    let mut add_hits_acc: u32 = 0;
    let mut sub_hits_acc: u32 = 0;
    let mut all_required_met = true;
    let mut measured_if_ok = true;
    let mut measured_current = None;
    let mut measured_target = None;

    for (idx, cond) in group.conditions.iter_mut().enumerate() {
        let mut cond_match = raw_matches[idx];
        if reset_next
            && !matches!(
                cond.flag,
                CondFlag::AddSource | CondFlag::SubSource | CondFlag::AddAddress
            )
        {
            cond.hits = 0;
            cond_match = false;
            reset_next = false;
        }

        match cond.flag {
            CondFlag::AddSource
            | CondFlag::SubSource
            | CondFlag::AddAddress
            | CondFlag::PauseIf => {
                continue;
            }
            CondFlag::ResetNextIf => {
                if cond_match {
                    reset_next = true;
                }
                continue;
            }
            CondFlag::AndNext | CondFlag::OrNext => {
                if cond.hit_target > 0 && cond_match && cond.hits < cond.hit_target {
                    cond.hits += 1;
                }
                continue;
            }
            CondFlag::AddHits => {
                if cond_match && (cond.hit_target == 0 || cond.hits < cond.hit_target) {
                    cond.hits = cond.hits.saturating_add(1);
                }
                add_hits_acc = add_hits_acc.saturating_add(cond.hits);
                continue;
            }
            CondFlag::SubHits => {
                if cond_match && (cond.hit_target == 0 || cond.hits < cond.hit_target) {
                    cond.hits = cond.hits.saturating_add(1);
                }
                sub_hits_acc = sub_hits_acc.saturating_add(cond.hits);
                continue;
            }
            CondFlag::ResetIf => {
                let fired = if cond.hit_target > 0 {
                    if cond_match && cond.hits < cond.hit_target {
                        cond.hits += 1;
                    }
                    cond.hits >= cond.hit_target
                } else {
                    cond_match
                };
                add_hits_acc = 0;
                sub_hits_acc = 0;
                if fired {
                    reset_all = true;
                }
                continue;
            }
            CondFlag::MeasuredIf => {
                let ok = if cond.hit_target > 0 {
                    if cond_match && cond.hits < cond.hit_target {
                        cond.hits += 1;
                    }
                    cond.hits >= cond.hit_target
                } else {
                    cond_match
                };
                add_hits_acc = 0;
                sub_hits_acc = 0;
                if !ok {
                    measured_if_ok = false;
                }
                continue;
            }
            CondFlag::Standard | CondFlag::Trigger | CondFlag::Measured => {
                let satisfied = if cond.hit_target > 0 {
                    let effective_now = cond
                        .hits
                        .saturating_add(add_hits_acc)
                        .saturating_sub(sub_hits_acc);
                    if cond_match && effective_now < cond.hit_target {
                        cond.hits = cond.hits.saturating_add(1);
                    }
                    let effective_after = cond
                        .hits
                        .saturating_add(add_hits_acc)
                        .saturating_sub(sub_hits_acc);
                    if cond.flag == CondFlag::Measured {
                        measured_current = Some(i64::from(effective_after.min(cond.hit_target)));
                        measured_target = Some(i64::from(cond.hit_target));
                    }
                    effective_after >= cond.hit_target
                } else {
                    if cond.flag == CondFlag::Measured {
                        measured_current = Some(lhs_values[idx]);
                        if cond.cmp.is_some() {
                            measured_target = Some(rhs_values[idx]);
                        }
                    }
                    if cond.cmp.is_some() { cond_match } else { true }
                };
                add_hits_acc = 0;
                sub_hits_acc = 0;
                if !satisfied {
                    all_required_met = false;
                }
            }
        }
    }

    GroupEvalOutcome {
        met: all_required_met,
        paused: false,
        reset_all,
        measured_if_ok,
        measured_current,
        measured_target,
    }
}

fn split_alt_groups(input: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let bytes = input.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] == b'S' {
            let preceded_by_0x =
                i >= 2 && bytes[i - 2] == b'0' && (bytes[i - 1] == b'x' || bytes[i - 1] == b'X');
            let is_flag_colon = bytes.get(i + 1) == Some(&b':');
            if !preceded_by_0x && !is_flag_colon {
                parts.push(&input[start..i]);
                start = i + 1;
            }
        }
        i += 1;
    }
    parts.push(&input[start..]);
    parts
}

fn parse_condition(raw: &str, next_history_idx: &mut usize) -> Result<Condition, String> {
    let mut s = raw.trim();

    // Parse optional trailing HitCount `(N)` or legacy `.N.`.
    let mut hit_target = 0u32;
    if s.ends_with(')') {
        if let Some(open) = s.rfind('(') {
            let inner = s[open + 1..s.len() - 1].trim();
            if !inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit()) {
                hit_target = inner
                    .parse::<u32>()
                    .map_err(|_| format!("invalid hit target in '{raw}'"))?;
                s = s[..open].trim();
            }
        }
    } else if s.ends_with('.') {
        let without_last = &s[..s.len() - 1];
        if let Some(prev_dot) = without_last.rfind('.') {
            let inner = &without_last[prev_dot + 1..];
            if !inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit()) {
                hit_target = inner
                    .parse::<u32>()
                    .map_err(|_| format!("invalid hit target in '{raw}'"))?;
                s = without_last[..prev_dot].trim();
            }
        }
    }

    // Parse optional condition flag `X:`.
    let flag = if s.len() >= 2 && &s[1..2] == ":" {
        let prefix = s.as_bytes()[0].to_ascii_uppercase();
        s = s[2..].trim();
        match prefix {
            b'R' => CondFlag::ResetIf,
            b'P' => CondFlag::PauseIf,
            b'N' => CondFlag::AndNext,
            b'O' => CondFlag::OrNext,
            b'A' => CondFlag::AddSource,
            b'B' => CondFlag::SubSource,
            b'C' => CondFlag::AddHits,
            b'D' => CondFlag::SubHits,
            b'I' => CondFlag::AddAddress,
            b'T' => CondFlag::Trigger,
            b'M' | b'G' => CondFlag::Measured,
            b'Q' => CondFlag::MeasuredIf,
            b'Z' => CondFlag::ResetNextIf,
            other => return Err(format!("unknown condition flag '{}:'", other as char)),
        }
    } else {
        CondFlag::Standard
    };

    // Split on comparison operator if present.
    if let Some((lhs_str, cmp_op, rhs_str)) = split_comparison(s) {
        let lhs = parse_expr(lhs_str, next_history_idx)?;
        let rhs = parse_expr(rhs_str, next_history_idx)?;
        Ok(Condition {
            flag,
            lhs,
            cmp: Some((cmp_op, rhs)),
            hit_target,
            hits: 0,
        })
    } else {
        let lhs = parse_expr(s, next_history_idx)?;
        Ok(Condition {
            flag,
            lhs,
            cmp: None,
            hit_target,
            hits: 0,
        })
    }
}

fn split_comparison(s: &str) -> Option<(&str, CmpOp, &str)> {
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let two = bytes.get(i..i + 2);
        let matched = match two {
            Some(b"!=") => Some((CmpOp::Ne, 2)),
            Some(b"<=") => Some((CmpOp::Le, 2)),
            Some(b">=") => Some((CmpOp::Ge, 2)),
            Some(b"==") => Some((CmpOp::Eq, 2)),
            _ => match bytes[i] {
                b'=' => Some((CmpOp::Eq, 1)),
                b'<' => Some((CmpOp::Lt, 1)),
                b'>' => Some((CmpOp::Gt, 1)),
                _ => None,
            },
        };
        if let Some((op, len)) = matched {
            return Some((s[..i].trim(), op, s[i + len..].trim()));
        }
        i += 1;
    }
    None
}

struct ExprParser<'a> {
    bytes: &'a [u8],
    pos: usize,
    next_history_idx: &'a mut usize,
}

fn parse_expr(input: &str, next_history_idx: &mut usize) -> Result<OperandExpr, String> {
    let mut parser = ExprParser {
        bytes: input.as_bytes(),
        pos: 0,
        next_history_idx,
    };
    let expr = parser.parse_add_sub()?;
    parser.skip_ws();
    if parser.pos < parser.bytes.len() {
        let rem = std::str::from_utf8(&parser.bytes[parser.pos..]).unwrap_or("");
        return Err(format!("unexpected trailing tokens '{rem}' in '{input}'"));
    }
    Ok(expr)
}

impl<'a> ExprParser<'a> {
    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos] == b' ' {
            // Note: DO NOT skip spaces immediately following `0x` (e.g. `0x 0010` means 16-bit!).
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn parse_add_sub(&mut self) -> Result<OperandExpr, String> {
        let mut lhs = self.parse_mul_div()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'+') => {
                    self.pos += 1;
                    let rhs = self.parse_mul_div()?;
                    lhs = OperandExpr::Binary {
                        op: ArithOp::Add,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    };
                }
                Some(b'-') => {
                    self.pos += 1;
                    let rhs = self.parse_mul_div()?;
                    lhs = OperandExpr::Binary {
                        op: ArithOp::Sub,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    };
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    fn parse_mul_div(&mut self) -> Result<OperandExpr, String> {
        let mut lhs = self.parse_bit_and()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'*') => {
                    self.pos += 1;
                    let rhs = self.parse_bit_and()?;
                    lhs = OperandExpr::Binary {
                        op: ArithOp::Mul,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    };
                }
                Some(b'/') => {
                    self.pos += 1;
                    let rhs = self.parse_bit_and()?;
                    lhs = OperandExpr::Binary {
                        op: ArithOp::Div,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    };
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    fn parse_bit_and(&mut self) -> Result<OperandExpr, String> {
        let mut lhs = self.parse_atom()?;
        loop {
            self.skip_ws();
            if self.peek() == Some(b'&') {
                self.pos += 1;
                let rhs = self.parse_atom()?;
                lhs = OperandExpr::Binary {
                    op: ArithOp::BitAnd,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                };
            } else {
                break;
            }
        }
        Ok(lhs)
    }

    fn parse_atom(&mut self) -> Result<OperandExpr, String> {
        self.skip_ws();
        if self.peek() == Some(b'-') {
            self.pos += 1;
            let inner = self.parse_atom()?;
            return Ok(OperandExpr::UnaryNeg(Box::new(inner)));
        }

        // Check memory reference prefixes (`d`, `p`, `b`, `~` followed by `0x`).
        let mut mode = MemValueMode::Current;
        let mut bcd = false;
        let mut invert = false;
        let saved_pos = self.pos;

        loop {
            match self.peek().map(|b| b.to_ascii_lowercase()) {
                Some(b'd') if !self.starts_with_0x() => {
                    mode = MemValueMode::Delta;
                    self.pos += 1;
                }
                Some(b'p') => {
                    mode = MemValueMode::Prior;
                    self.pos += 1;
                }
                Some(b'b') if self.looks_like_prefix_then_0x() => {
                    bcd = true;
                    self.pos += 1;
                }
                Some(b'~') => {
                    invert = true;
                    self.pos += 1;
                }
                _ => break,
            }
        }

        if self.starts_with_0x() {
            self.pos += 2;
            let size = match self.peek() {
                Some(b'L' | b'l') => {
                    self.pos += 1;
                    MemSize::Lower4
                }
                Some(b'U' | b'u') => {
                    self.pos += 1;
                    MemSize::Upper4
                }
                Some(b'H' | b'h') => {
                    self.pos += 1;
                    MemSize::Bits8
                }
                Some(b' ') => {
                    self.pos += 1;
                    MemSize::Bits16
                }
                Some(b'W' | b'w') => {
                    self.pos += 1;
                    MemSize::Bits24
                }
                Some(b'X' | b'x') => {
                    self.pos += 1;
                    MemSize::Bits32
                }
                Some(b'I' | b'i') => {
                    self.pos += 1;
                    MemSize::Bits16BE
                }
                Some(b'J' | b'j') => {
                    self.pos += 1;
                    MemSize::Bits24BE
                }
                Some(b'G' | b'g') => {
                    self.pos += 1;
                    MemSize::Bits32BE
                }
                Some(b'M' | b'm') => {
                    self.pos += 1;
                    MemSize::Bit(0)
                }
                Some(b'N' | b'n') => {
                    self.pos += 1;
                    MemSize::Bit(1)
                }
                Some(b'O' | b'o') => {
                    self.pos += 1;
                    MemSize::Bit(2)
                }
                Some(b'P' | b'p') => {
                    self.pos += 1;
                    MemSize::Bit(3)
                }
                Some(b'Q' | b'q') => {
                    self.pos += 1;
                    MemSize::Bit(4)
                }
                Some(b'R' | b'r') => {
                    self.pos += 1;
                    MemSize::Bit(5)
                }
                Some(b'S' | b's') => {
                    self.pos += 1;
                    MemSize::Bit(6)
                }
                Some(b'T' | b't') => {
                    self.pos += 1;
                    MemSize::Bit(7)
                }
                Some(b'K' | b'k') => {
                    self.pos += 1;
                    MemSize::BitCount
                }
                _ => MemSize::Bits16,
            };

            let addr_start = self.pos;
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_hexdigit() {
                self.pos += 1;
            }
            if self.pos == addr_start {
                return Err("missing hex address after 0x".to_string());
            }
            let hex_str = std::str::from_utf8(&self.bytes[addr_start..self.pos])
                .map_err(|_| "non-UTF8 hex address")?;
            let addr = u32::from_str_radix(hex_str, 16)
                .map_err(|_| format!("invalid hex address 0x{hex_str}"))?;

            let history_idx = *self.next_history_idx;
            *self.next_history_idx += 1;
            return Ok(OperandExpr::Mem {
                mem: MemRef {
                    addr,
                    size,
                    mode,
                    bcd,
                    invert,
                },
                history_idx,
            });
        }

        // Not a MemRef: rewind prefix attempts and parse literal constant (`h<hex>`, decimal, or `v<val>`).
        self.pos = saved_pos;
        if matches!(self.peek(), Some(b'h' | b'H')) {
            self.pos += 1;
            let start = self.pos;
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_hexdigit() {
                self.pos += 1;
            }
            if self.pos == start {
                return Err("missing hex literal after 'h'".to_string());
            }
            let hex_str = std::str::from_utf8(&self.bytes[start..self.pos])
                .map_err(|_| "non-UTF8 hex literal")?;
            let val = i64::from_str_radix(hex_str, 16)
                .map_err(|_| format!("invalid hex literal h{hex_str}"))?;
            return Ok(OperandExpr::Const(val));
        }

        if matches!(self.peek(), Some(b'v' | b'V')) {
            self.pos += 1;
        }

        let start = self.pos;
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        if self.pos == start {
            let rem = std::str::from_utf8(&self.bytes[self.pos..]).unwrap_or("");
            return Err(format!("expected operand near '{rem}'"));
        }
        let dec_str = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| "non-UTF8 integer literal")?;
        let val = dec_str
            .parse::<i64>()
            .map_err(|_| format!("invalid integer literal '{dec_str}'"))?;
        Ok(OperandExpr::Const(val))
    }

    fn starts_with_0x(&self) -> bool {
        self.bytes.get(self.pos) == Some(&b'0')
            && matches!(self.bytes.get(self.pos + 1), Some(b'x' | b'X'))
    }

    fn looks_like_prefix_then_0x(&self) -> bool {
        self.bytes.get(self.pos + 1) == Some(&b'0')
            && matches!(self.bytes.get(self.pos + 2), Some(b'x' | b'X'))
    }
}

/// Active or earned RetroAchievements entry tracked during gameplay.
#[derive(Clone, Debug)]
pub struct RuntimeAchievement {
    pub id: u64,
    pub title: String,
    pub description: String,
    pub points: u32,
    #[allow(dead_code)]
    pub flags: u32,
    #[allow(dead_code)]
    pub mem_addr: String,
    pub earned: bool,
    pub hardcore_earned: bool,
    pub measured_current: Option<i64>,
    pub measured_target: Option<i64>,
    pub parse_error: Option<String>,
    program: Option<TriggerProgram>,
}

/// Slide-in HUD notification queued when an achievement unlocks.
#[derive(Clone, Debug)]
pub struct UnlockToast {
    #[allow(dead_code)]
    pub id: u64,
    pub title: String,
    pub description: String,
    pub points: u32,
    pub hardcore: bool,
    pub created_at: Instant,
}

impl UnlockToast {
    /// Returns `[0.0..1.0]` normalized lifetime progress or `None` once expired.
    pub fn progress(&self) -> Option<f32> {
        let elapsed = self.created_at.elapsed();
        if elapsed >= TOAST_DURATION {
            None
        } else {
            Some((elapsed.as_secs_f32() / TOAST_DURATION.as_secs_f32()).clamp(0.0, 1.0))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RaConnectionState {
    LoggedOut,
    Authenticating,
    Authenticated,
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RaGameState {
    NoRom,
    LookingUpHash,
    UnrecognizedRom,
    Ready { game_id: u64, title: String },
    DemoMode { title: String },
    Error(String),
}

enum WorkerCommand {
    Login {
        username: String,
        password: String,
    },
    LoadGameByMd5 {
        username: String,
        token: String,
        md5: String,
    },
    AwardAchievement {
        username: String,
        token: String,
        achievement_id: u64,
        hardcore: bool,
        md5: String,
    },
    PingRichPresence {
        username: String,
        token: String,
        game_id: u64,
        message: String,
        md5: String,
    },
}

enum WorkerEvent {
    LoginSucceeded {
        username: String,
        token: String,
        score: u32,
        softcore_score: u32,
    },
    LoginFailed(String),
    GameNotFound {
        md5: String,
    },
    GameLoaded {
        md5: String,
        game_id: u64,
        title: String,
        rich_presence_script: String,
        achievements: Vec<PatchAchievement>,
    },
    GameLoadFailed {
        md5: String,
        error: String,
    },
    AwardResult {
        achievement_id: u64,
        success: bool,
        new_score: Option<u32>,
        error: Option<String>,
    },
    PingResult(Result<(), String>),
}

#[derive(Clone, Debug, Deserialize)]
struct ApiResponseEnvelope {
    #[serde(rename = "Success", default)]
    success: bool,
    #[serde(rename = "Error")]
    error: Option<String>,
    #[serde(rename = "User")]
    user: Option<String>,
    #[serde(rename = "Token")]
    token: Option<String>,
    #[serde(rename = "Score")]
    score: Option<u32>,
    #[serde(rename = "SoftcoreScore")]
    softcore_score: Option<u32>,
    #[serde(rename = "GameID")]
    game_id: Option<u64>,
    #[serde(rename = "PatchData")]
    patch_data: Option<PatchData>,
    #[serde(rename = "AchievementID")]
    achievement_id: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
struct PatchData {
    #[serde(rename = "ID", default)]
    id: u64,
    #[serde(rename = "Title", default)]
    title: String,
    #[serde(rename = "RichPresencePatch", default)]
    rich_presence_patch: String,
    #[serde(rename = "Achievements", default)]
    achievements: Vec<PatchAchievement>,
}

#[derive(Clone, Debug, Deserialize)]
struct PatchAchievement {
    #[serde(rename = "ID", default)]
    id: u64,
    #[serde(rename = "Title", default)]
    title: String,
    #[serde(rename = "Description", default)]
    description: String,
    #[serde(rename = "MemAddr", default)]
    mem_addr: String,
    #[serde(rename = "Points", default)]
    points: u32,
    #[serde(rename = "Flags", default = "default_core_flag")]
    flags: u32,
}

fn default_core_flag() -> u32 {
    3
}

struct RichPresenceScript {
    default_display: String,
}

impl RichPresenceScript {
    fn parse(raw: &str) -> Self {
        let mut in_display = false;
        let mut display = String::new();
        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.eq_ignore_ascii_case("Display:") {
                in_display = true;
                continue;
            }
            if in_display {
                if trimmed.is_empty() || trimmed.starts_with("//") {
                    continue;
                }
                if !trimmed.starts_with('?') {
                    display = trimmed.to_string();
                    break;
                }
            }
        }
        Self {
            default_display: display,
        }
    }

    fn format(&self, game_title: &str, hardcore: bool) -> String {
        if self.default_display.is_empty() || self.default_display.contains('@') {
            if hardcore {
                format!("Playing {game_title} [Hardcore]")
            } else {
                format!("Playing {game_title}")
            }
        } else {
            self.default_display.clone()
        }
    }
}

/// Manages RetroAchievements authentication, ROM identification, achievement evaluation,
/// Rich Presence pings, Hardcore enforcement state, and unlock toast banners.
pub struct RetroAchievementsManager {
    pub show_window: bool,
    pub connection_state: RaConnectionState,
    pub game_state: RaGameState,
    pub current_rom_md5: Option<String>,
    pub current_rom_title: String,
    pub achievements: Vec<RuntimeAchievement>,
    pub hardcore_score: u32,
    pub softcore_score: u32,
    pub last_status_note: Option<String>,
    pub unlock_queue: VecDeque<UnlockToast>,
    pub active_unlock_banner: Option<UnlockToast>,
    username_input: String,
    password_input: String,
    rich_presence: RichPresenceScript,
    last_rp_ping: Option<Instant>,
    cmd_tx: Option<Sender<WorkerCommand>>,
    event_rx: Receiver<WorkerEvent>,
    worker_handle: Option<JoinHandle<()>>,
}

impl Drop for RetroAchievementsManager {
    fn drop(&mut self) {
        self.cmd_tx.take();
        if let Some(handle) = self.worker_handle.take() {
            let _ = handle.join();
        }
    }
}

impl RetroAchievementsManager {
    pub fn new(config: &AppConfig) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCommand>();
        let (event_tx, event_rx) = mpsc::channel::<WorkerEvent>();

        let worker_handle = thread::Builder::new()
            .name("ra-api-worker".to_string())
            .spawn(move || run_worker_loop(cmd_rx, event_tx))
            .ok();

        let connection_state = if !config.ra_username.is_empty() && !config.ra_token.is_empty() {
            RaConnectionState::Authenticated
        } else {
            RaConnectionState::LoggedOut
        };

        Self {
            show_window: false,
            connection_state,
            game_state: RaGameState::NoRom,
            current_rom_md5: None,
            current_rom_title: String::new(),
            achievements: Vec::new(),
            hardcore_score: 0,
            softcore_score: 0,
            last_status_note: None,
            unlock_queue: VecDeque::new(),
            active_unlock_banner: None,
            username_input: config.ra_username.clone(),
            password_input: String::new(),
            rich_presence: RichPresenceScript {
                default_display: String::new(),
            },
            last_rp_ping: None,
            cmd_tx: Some(cmd_tx),
            event_rx,
            worker_handle,
        }
    }

    pub fn is_logged_in(&self, config: &AppConfig) -> bool {
        !config.ra_username.is_empty() && !config.ra_token.is_empty()
    }

    /// Returns true strictly when RA Hardcore mode should enforce gameplay restrictions:
    /// RA enabled + Hardcore enabled + logged in + active game loaded.
    pub fn is_hardcore_active(&self, config: &AppConfig) -> bool {
        config.ra_enabled
            && config.ra_hardcore
            && self.is_logged_in(config)
            && matches!(
                self.game_state,
                RaGameState::Ready { .. } | RaGameState::DemoMode { .. }
            )
    }

    /// Computes ROM MD5 and initiates async `gameid` + `patch` lookup when RA is enabled.
    pub fn on_rom_loaded(&mut self, config: &AppConfig, rom_bytes: &[u8], rom_title: &str) {
        let md5 = hash_rom_md5(rom_bytes);
        self.current_rom_md5 = Some(md5.clone());
        self.current_rom_title = rom_title.to_string();
        self.achievements.clear();
        self.last_rp_ping = None;
        self.last_status_note = None;
        self.rich_presence = RichPresenceScript {
            default_display: String::new(),
        };

        if config.ra_enabled && self.is_logged_in(config) {
            self.game_state = RaGameState::LookingUpHash;
            self.send_cmd(WorkerCommand::LoadGameByMd5 {
                username: config.ra_username.clone(),
                token: config.ra_token.clone(),
                md5,
            });
        } else {
            self.game_state = RaGameState::UnrecognizedRom;
        }
    }

    pub fn on_rom_closed(&mut self) {
        self.current_rom_md5 = None;
        self.current_rom_title.clear();
        self.achievements.clear();
        self.game_state = RaGameState::NoRom;
        self.last_rp_ping = None;
        self.unlock_queue.clear();
        self.active_unlock_banner = None;
    }

    /// Loads an offline 3-achievement sample set against the current GBA RAM for testing
    /// evaluation, Measured progress, hit counts, Hardcore locks, and HUD toast banners.
    pub fn load_sample_demo_set(&mut self) {
        let title = if self.current_rom_title.is_empty() {
            "Offline Demo Arena".to_string()
        } else {
            format!("{} (Offline Demo)", self.current_rom_title)
        };
        self.game_state = RaGameState::DemoMode { title };
        self.last_rp_ping = None;
        self.last_status_note = Some(
            "Loaded offline demo achievements (watches EWRAM 0x02000000 / 0x02000004 & IWRAM 0x03000008)."
                .to_string(),
        );

        let demo = vec![
            PatchAchievement {
                id: 900_001,
                title: "First Contact".to_string(),
                description: "Write any non-zero byte to EWRAM 0x02000000 (RA 0x000000)."
                    .to_string(),
                mem_addr: "0xH000000>0".to_string(),
                points: 5,
                flags: 3,
            },
            PatchAchievement {
                id: 900_002,
                title: "Collector's Streak".to_string(),
                description: "Reach value 5 at EWRAM 0x02000004 (Measured progress).".to_string(),
                mem_addr: "M:0xH000004=5".to_string(),
                points: 10,
                flags: 3,
            },
            PatchAchievement {
                id: 900_003,
                title: "Steady Hold".to_string(),
                description: "Keep IWRAM 0x03000008 == 1 for 3 consecutive frames.".to_string(),
                mem_addr: "0xH040008=1(3)_R:0xH040008!=1".to_string(),
                points: 25,
                flags: 3,
            },
        ];
        self.install_patch_achievements(demo);
    }

    fn install_patch_achievements(&mut self, patch_list: Vec<PatchAchievement>) {
        self.achievements = patch_list
            .into_iter()
            .filter(|a| a.flags == 3)
            .map(|a| match TriggerProgram::parse(&a.mem_addr) {
                Ok(prog) => RuntimeAchievement {
                    id: a.id,
                    title: a.title,
                    description: a.description,
                    points: a.points,
                    flags: a.flags,
                    mem_addr: a.mem_addr,
                    earned: false,
                    hardcore_earned: false,
                    measured_current: None,
                    measured_target: None,
                    parse_error: None,
                    program: Some(prog),
                },
                Err(err) => RuntimeAchievement {
                    id: a.id,
                    title: a.title,
                    description: a.description,
                    points: a.points,
                    flags: a.flags,
                    mem_addr: a.mem_addr,
                    earned: false,
                    hardcore_earned: false,
                    measured_current: None,
                    measured_target: None,
                    parse_error: Some(err),
                    program: None,
                },
            })
            .collect();
    }

    /// Evaluates all unearned achievements once per emulated frame and updates unlock/ping state.
    pub fn tick_frame(&mut self, core: &GbaCore, config: &AppConfig) {
        if !config.ra_enabled {
            return;
        }
        let hardcore = self.is_hardcore_active(config);
        let is_demo = matches!(self.game_state, RaGameState::DemoMode { .. });

        for ach in &mut self.achievements {
            if ach.earned {
                continue;
            }
            let Some(prog) = &mut ach.program else {
                continue;
            };
            let res = prog.evaluate_frame(core);
            ach.measured_current = res.measured_current;
            ach.measured_target = res.measured_target;

            if res.triggered {
                ach.earned = true;
                ach.hardcore_earned = hardcore;
                self.unlock_queue.push_back(UnlockToast {
                    id: ach.id,
                    title: ach.title.clone(),
                    description: ach.description.clone(),
                    points: ach.points,
                    hardcore,
                    created_at: Instant::now(),
                });

                if !is_demo
                    && !config.ra_username.is_empty()
                    && !config.ra_token.is_empty()
                    && let Some(md5) = &self.current_rom_md5
                    && let Some(tx) = &self.cmd_tx
                {
                    let _ = tx.send(WorkerCommand::AwardAchievement {
                        username: config.ra_username.clone(),
                        token: config.ra_token.clone(),
                        achievement_id: ach.id,
                        hardcore,
                        md5: md5.clone(),
                    });
                }
            }
        }

        // Rich Presence ping every ~2 minutes when logged in with a live recognized RA game.
        if config.ra_rich_presence
            && self.is_logged_in(config)
            && let RaGameState::Ready { game_id, title } = &self.game_state
        {
            let due = self
                .last_rp_ping
                .map(|t| t.elapsed() >= RICH_PRESENCE_INTERVAL)
                .unwrap_or(true);
            if due {
                self.last_rp_ping = Some(Instant::now());
                let msg = self.rich_presence.format(title, hardcore);
                if let Some(md5) = &self.current_rom_md5 {
                    self.send_cmd(WorkerCommand::PingRichPresence {
                        username: config.ra_username.clone(),
                        token: config.ra_token.clone(),
                        game_id: *game_id,
                        message: msg,
                        md5: md5.clone(),
                    });
                }
            }
        }
    }

    /// Polls background worker events and advances active HUD unlock banner timers.
    pub fn poll_events(&mut self, config: &mut AppConfig) -> bool {
        let mut config_dirty = false;
        while let Ok(ev) = self.event_rx.try_recv() {
            match ev {
                WorkerEvent::LoginSucceeded {
                    username,
                    token,
                    score,
                    softcore_score,
                } => {
                    config.ra_username = username.clone();
                    config.ra_token = token;
                    config.ra_enabled = true;
                    config_dirty = true;
                    self.username_input = username;
                    self.password_input.clear();
                    self.hardcore_score = score;
                    self.softcore_score = softcore_score;
                    self.connection_state = RaConnectionState::Authenticated;
                    self.last_status_note = Some("Signed in to RetroAchievements.".to_string());

                    if let Some(md5) = self.current_rom_md5.clone() {
                        self.game_state = RaGameState::LookingUpHash;
                        self.send_cmd(WorkerCommand::LoadGameByMd5 {
                            username: config.ra_username.clone(),
                            token: config.ra_token.clone(),
                            md5,
                        });
                    }
                }
                WorkerEvent::LoginFailed(err) => {
                    self.password_input.clear();
                    self.connection_state = RaConnectionState::Error(err);
                }
                WorkerEvent::GameNotFound { md5 } => {
                    if self.current_rom_md5.as_deref() == Some(&md5) {
                        self.game_state = RaGameState::UnrecognizedRom;
                    }
                }
                WorkerEvent::GameLoaded {
                    md5,
                    game_id,
                    title,
                    rich_presence_script,
                    achievements,
                } => {
                    if self.current_rom_md5.as_deref() == Some(&md5) {
                        self.rich_presence = RichPresenceScript::parse(&rich_presence_script);
                        self.install_patch_achievements(achievements);
                        self.game_state = RaGameState::Ready { game_id, title };
                        self.last_rp_ping = None;
                    }
                }
                WorkerEvent::GameLoadFailed { md5, error } => {
                    if self.current_rom_md5.as_deref() == Some(&md5) {
                        self.game_state = RaGameState::Error(error);
                    }
                }
                WorkerEvent::AwardResult {
                    achievement_id,
                    success,
                    new_score,
                    error,
                } => {
                    if let Some(score) = new_score {
                        self.hardcore_score = score;
                    }
                    if !success {
                        self.last_status_note = Some(format!(
                            "Award sync note for #{achievement_id}: {}",
                            error.unwrap_or_else(|| "request rejected".to_string())
                        ));
                    }
                }
                WorkerEvent::PingResult(Err(err)) => {
                    log::debug!("RetroAchievements ping skipped/failed: {err}");
                }
                WorkerEvent::PingResult(Ok(())) => {}
            }
        }

        // Advance HUD toast queue cleanly.
        if let Some(active) = &self.active_unlock_banner
            && active.progress().is_none()
        {
            self.active_unlock_banner = None;
        }
        if self.active_unlock_banner.is_none()
            && let Some(mut next) = self.unlock_queue.pop_front()
        {
            next.created_at = Instant::now();
            self.active_unlock_banner = Some(next);
        }

        config_dirty
    }

    pub fn current_unlock_banner(&self) -> Option<&UnlockToast> {
        self.active_unlock_banner
            .as_ref()
            .filter(|b| b.progress().is_some())
    }

    fn send_cmd(&self, cmd: WorkerCommand) {
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(cmd);
        }
    }

    /// Renders the `Tools -> RetroAchievements...` floating window.
    pub fn show_retroachievements_window(
        &mut self,
        ctx: &egui::Context,
        config: &mut AppConfig,
    ) -> bool {
        if !self.show_window {
            return false;
        }

        let mut open = self.show_window;
        let mut config_changed = false;

        egui::Window::new("RetroAchievements")
            .open(&mut open)
            .default_width(540.0)
            .default_height(560.0)
            .min_width(440.0)
            .min_height(380.0)
            .resizable(true)
            .show(ctx, |ui| {
                config_changed = self.draw_window_contents(ui, config);
            });

        self.show_window = open;
        config_changed
    }

    fn draw_window_contents(&mut self, ui: &mut egui::Ui, config: &mut AppConfig) -> bool {
        let mut config_changed = false;

        let card_bg = Color32::from_rgb(18, 21, 28);
        let elevated_bg = Color32::from_rgb(24, 28, 37);
        let hairline = Color32::from_rgb(42, 47, 60);
        let accent_teal = Color32::from_rgb(0, 210, 175);
        let accent_amber = Color32::from_rgb(245, 158, 11);
        let accent_emerald = Color32::from_rgb(16, 185, 129);
        let text_muted = Color32::from_rgb(142, 150, 168);

        let hardcore_locked = self.is_hardcore_active(config);

        // Top controls & Hardcore strip
        egui::Frame::new()
            .fill(elevated_bg)
            .stroke(Stroke::new(1.0_f32, hairline))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.checkbox(&mut config.ra_enabled, "Enable RetroAchievements").changed() {
                        config_changed = true;
                        if !config.ra_enabled {
                            self.achievements.clear();
                        }
                    }

                    if ui
                        .checkbox(&mut config.ra_hardcore, "Hardcore Mode")
                        .on_hover_text(
                            "Disables Rewind, Cheats, Save/Load State, Lua memory writes, and Debugger stepping while a game is active.",
                        )
                        .changed()
                    {
                        config_changed = true;
                    }

                    if ui
                        .checkbox(&mut config.ra_rich_presence, "Rich Presence")
                        .changed()
                    {
                        config_changed = true;
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (badge_text, badge_col) = if hardcore_locked {
                            ("HARDCORE LOCK", accent_amber)
                        } else if config.ra_hardcore {
                            ("HARDCORE ARMED", text_muted)
                        } else {
                            ("SOFTCORE", accent_teal)
                        };
                        egui::Frame::new()
                            .fill(Color32::from_rgb(16, 19, 25))
                            .stroke(Stroke::new(1.0_f32, badge_col))
                            .corner_radius(3.0)
                            .inner_margin(egui::Margin::symmetric(6, 2))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(badge_text)
                                        .size(10.5)
                                        .strong()
                                        .color(badge_col),
                                );
                            });
                    });
                });

                if hardcore_locked {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Hardcore Active: Rewind, Cheats, Manual Load/Save State, Lua pokes, and Debugger stepping are disabled.",
                        )
                        .small()
                        .color(accent_amber),
                    );
                }
            });

        ui.add_space(6.0);

        // Account / Login Card
        egui::Frame::new()
            .fill(card_bg)
            .stroke(Stroke::new(1.0_f32, hairline))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                if self.is_logged_in(config) {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("Account: {}", config.ra_username))
                                .strong()
                                .color(Color32::WHITE),
                        );
                        ui.label(
                            RichText::new("Token saved (password never stored)")
                                .small()
                                .color(accent_emerald),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Sign Out").clicked() {
                                config.ra_username.clear();
                                config.ra_token.clear();
                                config_changed = true;
                                self.password_input.clear();
                                self.connection_state = RaConnectionState::LoggedOut;
                                self.achievements.clear();
                                if !matches!(self.game_state, RaGameState::NoRom) {
                                    self.game_state = RaGameState::UnrecognizedRom;
                                }
                            }
                        });
                    });
                    if self.hardcore_score > 0 || self.softcore_score > 0 {
                        ui.label(
                            RichText::new(format!(
                                "Hardcore Points: {}   |   Softcore Points: {}",
                                self.hardcore_score, self.softcore_score
                            ))
                            .small()
                            .color(text_muted),
                        );
                    }
                } else {
                    ui.label(RichText::new("RetroAchievements Sign In").strong());
                    ui.add_space(4.0);
                    egui::Grid::new("ra_login_grid")
                        .num_columns(2)
                        .spacing([8.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Username:");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.username_input)
                                    .desired_width(220.0)
                                    .hint_text("RA Username"),
                            );
                            ui.end_row();

                            ui.label("Password:");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.password_input)
                                    .password(true)
                                    .desired_width(220.0)
                                    .hint_text("Password (never saved)"),
                            );
                            ui.end_row();
                        });

                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let busy =
                            matches!(self.connection_state, RaConnectionState::Authenticating);
                        let can_submit = !busy
                            && !self.username_input.trim().is_empty()
                            && !self.password_input.is_empty();
                        if ui
                            .add_enabled(can_submit, egui::Button::new("Sign In"))
                            .clicked()
                        {
                            self.connection_state = RaConnectionState::Authenticating;
                            self.send_cmd(WorkerCommand::Login {
                                username: self.username_input.trim().to_string(),
                                password: self.password_input.clone(),
                            });
                        }
                        if busy {
                            ui.spinner();
                            ui.label(RichText::new("Signing in...").small().color(text_muted));
                        } else if let RaConnectionState::Error(err) = &self.connection_state {
                            ui.label(
                                RichText::new(err)
                                    .small()
                                    .color(Color32::from_rgb(248, 113, 113)),
                            );
                        }
                    });
                }
            });

        ui.add_space(6.0);

        // Current Game + Offline Demo Test Strip
        egui::Frame::new()
            .fill(card_bg)
            .stroke(Stroke::new(1.0_f32, hairline))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    match &self.game_state {
                        RaGameState::NoRom => {
                            ui.label(
                                RichText::new(
                                    "No ROM loaded. Open a GBA ROM or test offline below.",
                                )
                                .small()
                                .color(text_muted),
                            );
                        }
                        RaGameState::LookingUpHash => {
                            ui.spinner();
                            ui.label(
                                RichText::new("Looking up ROM MD5 on RetroAchievements...")
                                    .small()
                                    .color(text_muted),
                            );
                        }
                        RaGameState::UnrecognizedRom => {
                            ui.label(
                                RichText::new("ROM not matched on RA (or offline).")
                                    .small()
                                    .color(text_muted),
                            );
                        }
                        RaGameState::Ready { game_id, title } => {
                            ui.label(RichText::new(title).strong().color(accent_teal));
                            ui.label(
                                RichText::new(format!("Game ID: #{game_id}"))
                                    .small()
                                    .monospace()
                                    .color(text_muted),
                            );
                        }
                        RaGameState::DemoMode { title } => {
                            ui.label(RichText::new(title).strong().color(accent_emerald));
                        }
                        RaGameState::Error(err) => {
                            ui.label(
                                RichText::new(format!("Lookup error: {err}"))
                                    .small()
                                    .color(Color32::from_rgb(248, 113, 113)),
                            );
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Load Sample Demo Set (Offline Test)").clicked() {
                            self.load_sample_demo_set();
                        }
                    });
                });

                if let Some(md5) = &self.current_rom_md5 {
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(format!("ROM MD5: {md5}"))
                            .monospace()
                            .size(11.0)
                            .color(text_muted),
                    );
                }

                if let Some(note) = &self.last_status_note {
                    ui.add_space(2.0);
                    ui.label(RichText::new(note).small().color(text_muted));
                }
            });

        ui.add_space(6.0);

        // Achievements List
        let total = self.achievements.len();
        let earned_count = self.achievements.iter().filter(|a| a.earned).count();
        let total_pts: u32 = self.achievements.iter().map(|a| a.points).sum();
        let earned_pts: u32 = self
            .achievements
            .iter()
            .filter(|a| a.earned)
            .map(|a| a.points)
            .sum();

        ui.horizontal(|ui| {
            ui.label(RichText::new("Achievements").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!(
                        "{earned_count} / {total} unlocked  ({earned_pts} / {total_pts} pts)"
                    ))
                    .small()
                    .monospace()
                    .color(text_muted),
                );
            });
        });

        ui.add_space(4.0);

        egui::ScrollArea::vertical()
            .id_salt("ra_achievements_scroll")
            .max_height(260.0)
            .show(ui, |ui| {
                if self.achievements.is_empty() {
                    egui::Frame::new()
                        .fill(card_bg)
                        .stroke(Stroke::new(1.0_f32, hairline))
                        .corner_radius(4.0)
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(
                                    "No achievements loaded. Sign in with a recognized ROM or click \"Load Sample Demo Set (Offline Test)\".",
                                )
                                .small()
                                .italics()
                                .color(text_muted),
                            );
                        });
                    return;
                }

                for ach in &self.achievements {
                    let border_color = if ach.earned {
                        if ach.hardcore_earned {
                            accent_amber
                        } else {
                            accent_emerald
                        }
                    } else {
                        hairline
                    };

                    egui::Frame::new()
                        .fill(if ach.earned {
                            Color32::from_rgb(20, 29, 27)
                        } else {
                            card_bg
                        })
                        .stroke(Stroke::new(1.0_f32, border_color))
                        .corner_radius(4.0)
                        .inner_margin(egui::Margin::symmetric(8, 6))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let state_tag = if ach.earned {
                                    if ach.hardcore_earned {
                                        "EARNED (HC)"
                                    } else {
                                        "EARNED"
                                    }
                                } else {
                                    "LOCKED"
                                };
                                let tag_col = if ach.earned {
                                    border_color
                                } else {
                                    text_muted
                                };
                                ui.label(
                                    RichText::new(format!("[{state_tag}]"))
                                        .size(10.5)
                                        .strong()
                                        .color(tag_col),
                                );
                                ui.label(
                                    RichText::new(&ach.title).strong().color(if ach.earned {
                                        Color32::WHITE
                                    } else {
                                        Color32::from_rgb(210, 216, 228)
                                    }),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new(format!("{} pts", ach.points))
                                                .small()
                                                .monospace()
                                                .color(accent_amber),
                                        );
                                    },
                                );
                            });

                            ui.label(RichText::new(&ach.description).small().color(text_muted));

                            if !ach.earned {
                                if let (Some(cur), Some(target)) =
                                    (ach.measured_current, ach.measured_target)
                                {
                                    if target > 0 {
                                        let ratio = (cur as f32 / target as f32).clamp(0.0, 1.0);
                                        ui.add_space(2.0);
                                        ui.add(
                                            egui::ProgressBar::new(ratio)
                                                .text(format!("{cur} / {target}"))
                                                .desired_height(14.0),
                                        );
                                    }
                                } else if let Some(err) = &ach.parse_error {
                                    ui.label(
                                        RichText::new(format!("Unsupported trigger: {err}"))
                                            .size(10.5)
                                            .color(Color32::from_rgb(248, 113, 113)),
                                    );
                                }
                            }
                        });
                    ui.add_space(4.0);
                }
            });

        config_changed
    }
}

/// Draws the slide-in Achievement Unlocked HUD Toast Banner cleanly onto `viewport_rect`
/// without purple gradients or glowing orbs (obsidian surface + 1px emerald/amber border).
pub fn draw_unlock_toast_banner(painter: &egui::Painter, viewport_rect: Rect, toast: &UnlockToast) {
    let Some(t) = toast.progress() else {
        return;
    };

    // Smooth ease-out slide-in (first 12%) and ease-in slide-out (final 14%).
    let slide = if t < 0.12 {
        let p = t / 0.12;
        1.0 - (1.0 - p).powi(3)
    } else if t > 0.86 {
        let p = (1.0 - t) / 0.14;
        1.0 - (1.0 - p).powi(2)
    } else {
        1.0
    };
    let alpha_mul = slide.clamp(0.0, 1.0);

    let card_w = 290.0_f32.min((viewport_rect.width() - 24.0).max(180.0));
    let card_h = 62.0_f32;
    let margin = 12.0_f32;
    let x_target = viewport_rect.right() - card_w - margin;
    let x_start = viewport_rect.right() + 12.0;
    let card_x = x_start + (x_target - x_start) * slide;
    let card_y = viewport_rect.top() + margin;

    let card_rect = Rect::from_min_size(Pos2::new(card_x, card_y), Vec2::new(card_w, card_h));

    let border_rgb = if toast.hardcore {
        (245, 158, 11) // amber
    } else {
        (16, 185, 129) // emerald
    };

    let bg = Color32::from_rgba_unmultiplied(16, 19, 26, (238.0 * alpha_mul) as u8);
    let border = Color32::from_rgba_unmultiplied(
        border_rgb.0,
        border_rgb.1,
        border_rgb.2,
        (235.0 * alpha_mul) as u8,
    );
    let title_col = Color32::from_rgba_unmultiplied(244, 247, 252, (255.0 * alpha_mul) as u8);
    let desc_col = Color32::from_rgba_unmultiplied(164, 174, 192, (255.0 * alpha_mul) as u8);

    painter.rect_filled(card_rect, CornerRadius::same(4), bg);
    painter.rect_stroke(
        card_rect,
        CornerRadius::same(4),
        Stroke::new(1.0_f32, border),
        egui::StrokeKind::Inside,
    );

    // Crisp left accent stripe (3px) instead of glow effects.
    let stripe = Rect::from_min_size(card_rect.min, Vec2::new(3.0, card_h));
    painter.rect_filled(stripe, CornerRadius::same(2), border);

    let text_x = card_rect.left() + 12.0;
    let tag = if toast.hardcore {
        format!("ACHIEVEMENT UNLOCKED  •  +{} PTS (HC)", toast.points)
    } else {
        format!("ACHIEVEMENT UNLOCKED  •  +{} PTS", toast.points)
    };
    painter.text(
        Pos2::new(text_x, card_rect.top() + 8.0),
        Align2::LEFT_TOP,
        tag,
        FontId::proportional(10.0),
        border,
    );
    painter.text(
        Pos2::new(text_x, card_rect.top() + 23.0),
        Align2::LEFT_TOP,
        truncate_chars(&toast.title, 34),
        FontId::proportional(13.0),
        title_col,
    );
    painter.text(
        Pos2::new(text_x, card_rect.top() + 41.0),
        Align2::LEFT_TOP,
        truncate_chars(&toast.description, 44),
        FontId::proportional(11.0),
        desc_col,
    );
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

fn run_worker_loop(rx: Receiver<WorkerCommand>, tx: Sender<WorkerEvent>) {
    while let Ok(cmd) = rx.recv() {
        match cmd {
            WorkerCommand::Login { username, password } => {
                let ev = match perform_login(&username, &password) {
                    Ok((user, token, score, softcore_score)) => WorkerEvent::LoginSucceeded {
                        username: user,
                        token,
                        score,
                        softcore_score,
                    },
                    Err(err) => WorkerEvent::LoginFailed(err),
                };
                if tx.send(ev).is_err() {
                    break;
                }
            }
            WorkerCommand::LoadGameByMd5 {
                username,
                token,
                md5,
            } => {
                let ev = perform_load_game(&username, &token, &md5);
                if tx.send(ev).is_err() {
                    break;
                }
            }
            WorkerCommand::AwardAchievement {
                username,
                token,
                achievement_id,
                hardcore,
                md5,
            } => {
                let ev = perform_award(&username, &token, achievement_id, hardcore, &md5);
                if tx.send(ev).is_err() {
                    break;
                }
            }
            WorkerCommand::PingRichPresence {
                username,
                token,
                game_id,
                message,
                md5,
            } => {
                let res = perform_ping(&username, &token, game_id, &message, &md5);
                if tx.send(WorkerEvent::PingResult(res)).is_err() {
                    break;
                }
            }
        }
    }
}

fn api_post(params: &[(&str, &str)]) -> Result<ApiResponseEnvelope, String> {
    let resp = ureq::post(RA_API_URL)
        .set("User-Agent", RA_USER_AGENT)
        .timeout(Duration::from_secs(12))
        .send_form(params)
        .map_err(|err| format!("Network error: {err}"))?;
    resp.into_json::<ApiResponseEnvelope>()
        .map_err(|err| format!("Invalid RA JSON response: {err}"))
}

fn api_get(params: &[(&str, &str)]) -> Result<ApiResponseEnvelope, String> {
    let mut req = ureq::get(RA_API_URL)
        .set("User-Agent", RA_USER_AGENT)
        .timeout(Duration::from_secs(12));
    for (k, v) in params {
        req = req.query(k, v);
    }
    let resp = req.call().map_err(|err| format!("Network error: {err}"))?;
    resp.into_json::<ApiResponseEnvelope>()
        .map_err(|err| format!("Invalid RA JSON response: {err}"))
}

fn perform_login(username: &str, password: &str) -> Result<(String, String, u32, u32), String> {
    let env = api_post(&[("r", "login2"), ("u", username), ("p", password)])?;
    if !env.success {
        return Err(env
            .error
            .unwrap_or_else(|| "RetroAchievements login rejected".to_string()));
    }
    let token = env
        .token
        .filter(|t| !t.is_empty())
        .ok_or_else(|| "RA login succeeded without Token".to_string())?;
    let user = env.user.unwrap_or_else(|| username.to_string());
    Ok((
        user,
        token,
        env.score.unwrap_or(0),
        env.softcore_score.unwrap_or(0),
    ))
}

fn perform_load_game(username: &str, token: &str, md5: &str) -> WorkerEvent {
    let game_env = match api_get(&[("r", "gameid"), ("m", md5)]) {
        Ok(env) => env,
        Err(err) => {
            return WorkerEvent::GameLoadFailed {
                md5: md5.to_string(),
                error: err,
            };
        }
    };
    if !game_env.success {
        return WorkerEvent::GameLoadFailed {
            md5: md5.to_string(),
            error: game_env
                .error
                .unwrap_or_else(|| "gameid lookup failed".to_string()),
        };
    }

    let game_id = game_env.game_id.unwrap_or(0);
    if game_id == 0 {
        return WorkerEvent::GameNotFound {
            md5: md5.to_string(),
        };
    }

    let gid_str = game_id.to_string();
    let patch_env = match api_get(&[
        ("r", "patch"),
        ("u", username),
        ("t", token),
        ("g", &gid_str),
    ]) {
        Ok(env) => env,
        Err(err) => {
            return WorkerEvent::GameLoadFailed {
                md5: md5.to_string(),
                error: err,
            };
        }
    };

    if !patch_env.success {
        return WorkerEvent::GameLoadFailed {
            md5: md5.to_string(),
            error: patch_env
                .error
                .unwrap_or_else(|| "patch download failed".to_string()),
        };
    }

    let Some(patch) = patch_env.patch_data else {
        return WorkerEvent::GameLoadFailed {
            md5: md5.to_string(),
            error: "patch response missing PatchData".to_string(),
        };
    };

    WorkerEvent::GameLoaded {
        md5: md5.to_string(),
        game_id: if patch.id > 0 { patch.id } else { game_id },
        title: patch.title,
        rich_presence_script: patch.rich_presence_patch,
        achievements: patch.achievements,
    }
}

fn perform_award(
    username: &str,
    token: &str,
    achievement_id: u64,
    hardcore: bool,
    md5: &str,
) -> WorkerEvent {
    let ach_str = achievement_id.to_string();
    let hc_str = if hardcore { "1" } else { "0" };
    let sig_input = format!("{achievement_id}{username}{hc_str}");
    let sig = hash_rom_md5(sig_input.as_bytes());

    match api_get(&[
        ("r", "awardachievement"),
        ("u", username),
        ("t", token),
        ("a", &ach_str),
        ("h", hc_str),
        ("m", md5),
        ("v", &sig),
    ]) {
        Ok(env) => WorkerEvent::AwardResult {
            achievement_id: env.achievement_id.unwrap_or(achievement_id),
            success: env.success,
            new_score: env.score,
            error: env.error,
        },
        Err(err) => WorkerEvent::AwardResult {
            achievement_id,
            success: false,
            new_score: None,
            error: Some(err),
        },
    }
}

fn perform_ping(
    username: &str,
    token: &str,
    game_id: u64,
    message: &str,
    md5: &str,
) -> Result<(), String> {
    let gid_str = game_id.to_string();
    let env = api_post(&[
        ("r", "ping"),
        ("u", username),
        ("t", token),
        ("g", &gid_str),
        ("m", message),
        ("x", md5),
    ])?;
    if env.success {
        Ok(())
    } else {
        Err(env.error.unwrap_or_else(|| "ping failed".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_core() -> GbaCore {
        let mut rom = vec![0u8; 0x200];
        rom[0xA0..0xAC].copy_from_slice(b"RATESTROM\0\0\0");
        rom[0xAC..0xB0].copy_from_slice(b"ARTE");
        GbaCore::new(None, rom)
    }

    #[test]
    fn rom_md5_hashing_matches_rfc1321_and_gba_sizes() {
        assert_eq!(hash_rom_md5(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hash_rom_md5(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        let fake_gba = vec![0xAAu8; 1024];
        let digest = hash_rom_md5(&fake_gba);
        assert_eq!(digest.len(), 32);
        assert!(digest.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
    }

    #[test]
    fn mem_addr_sizes_comparisons_and_bits() {
        let mut core = test_core();
        // EWRAM 0x0200_0010 -> RA 0x000010, IWRAM 0x0300_0020 -> RA 0x040020
        core.poke8(0x0200_0010, 0xA5); // binary 1010_0101 -> bits 0,2,5,7 set (4 bits)
        core.poke16(0x0200_0012, 1024);
        core.poke32(0x0300_0020, 0x1234_5678);

        let mut p_eq =
            TriggerProgram::parse("0xH000010=165_0x 000012>500_0xX040020=305419896").unwrap();
        assert!(p_eq.evaluate_frame(&core).triggered);

        let mut p_nib = TriggerProgram::parse("0xL000010=5_0xU000010=10_0xK000010=4").unwrap();
        assert!(p_nib.evaluate_frame(&core).triggered);

        // Bit0..Bit7: 0xA5 = bits 0,2,5,7 are 1; 1,3,4,6 are 0.
        let mut p_bits = TriggerProgram::parse(
            "0xM000010=1_0xN000010=0_0xO000010=1_0xP000010=0_0xQ000010=0_0xR000010=1_0xS000010=0_0xT000010=1",
        )
        .unwrap();
        assert!(p_bits.evaluate_frame(&core).triggered);
    }

    #[test]
    fn mem_addr_delta_and_prior_across_frames() {
        let mut core = test_core();
        let mut prog = TriggerProgram::parse("d0xH000010=1_0xH000010=2").unwrap();

        core.poke8(0x0200_0010, 1);
        assert!(!prog.evaluate_frame(&core).triggered);

        core.poke8(0x0200_0010, 2);
        assert!(prog.evaluate_frame(&core).triggered);

        // On next frame delta becomes 2, so d0x == 1 is now false.
        assert!(!prog.evaluate_frame(&core).triggered);

        // Verify Prior (`p`) retains the last differing value even across idle frames.
        let mut prior_prog = TriggerProgram::parse("p0xH000010=5_0xH000010=9").unwrap();
        core.poke8(0x0200_0010, 5);
        assert!(!prior_prog.evaluate_frame(&core).triggered);
        core.poke8(0x0200_0010, 9);
        assert!(prior_prog.evaluate_frame(&core).triggered);
        // Holding at 9 keeps prior=5 across subsequent frames!
        assert!(prior_prog.evaluate_frame(&core).triggered);
    }

    #[test]
    fn mem_addr_andnext_ornext_addsource_pauseif_resetif_and_hitcounts() {
        let mut core = test_core();

        // HitCounts (3) + ResetIf
        let mut hit_prog = TriggerProgram::parse("0xH000010=5(3)_R:0xH000011=1").unwrap();
        core.poke8(0x0200_0010, 5);
        assert!(!hit_prog.evaluate_frame(&core).triggered); // hits = 1
        assert!(!hit_prog.evaluate_frame(&core).triggered); // hits = 2
        // Fire ResetIf -> clears hits back to 0.
        core.poke8(0x0200_0011, 1);
        let reset_res = hit_prog.evaluate_frame(&core);
        assert!(!reset_res.triggered);
        assert!(reset_res.reset_fired);
        // Must take 3 more frames after reset.
        core.poke8(0x0200_0011, 0);
        assert!(!hit_prog.evaluate_frame(&core).triggered); // 1
        assert!(!hit_prog.evaluate_frame(&core).triggered); // 2
        assert!(hit_prog.evaluate_frame(&core).triggered); // 3!

        // PauseIf freezes hit accumulation without resetting existing hits.
        let mut pause_prog = TriggerProgram::parse("0xH000010=7(2)_P:0xH000012=1").unwrap();
        core.poke8(0x0200_0010, 7);
        assert!(!pause_prog.evaluate_frame(&core).triggered); // hits = 1
        core.poke8(0x0200_0012, 1);
        assert!(!pause_prog.evaluate_frame(&core).triggered); // paused! hits stay 1
        core.poke8(0x0200_0012, 0);
        assert!(pause_prog.evaluate_frame(&core).triggered); // hits = 2 -> unlocked!

        // AndNext & OrNext
        let mut logic_prog =
            TriggerProgram::parse("N:0xH000020=1_O:0xH000021=2_0xH000022=3").unwrap();
        core.poke8(0x0200_0020, 1);
        core.poke8(0x0200_0021, 0);
        core.poke8(0x0200_0022, 3);
        assert!(logic_prog.evaluate_frame(&core).triggered);
        core.poke8(0x0200_0022, 0);
        assert!(!logic_prog.evaluate_frame(&core).triggered);

        // AddSource + SubSource + scaling & mask
        let mut add_prog = TriggerProgram::parse("A:0xH000030*2_B:0xH000031_0xH000032=10").unwrap();
        core.poke8(0x0200_0030, 4); // * 2 = 8
        core.poke8(0x0200_0031, 3); // - 3 = 5
        core.poke8(0x0200_0032, 5); // + 5 = 10
        assert!(add_prog.evaluate_frame(&core).triggered);

        // Alt groups (S)
        let mut alt_prog = TriggerProgram::parse("0xH000040=1S0xH000041=2S0xH000042=3").unwrap();
        core.poke8(0x0200_0040, 1);
        core.poke8(0x0200_0041, 0);
        core.poke8(0x0200_0042, 0);
        assert!(!alt_prog.evaluate_frame(&core).triggered);
        core.poke8(0x0200_0042, 3);
        assert!(alt_prog.evaluate_frame(&core).triggered);
    }

    #[test]
    fn demo_set_achievement_progression_across_frames() {
        let mut core = test_core();
        let cfg = AppConfig::default();
        let mut mgr = RetroAchievementsManager::new(&cfg);
        mgr.load_sample_demo_set();
        assert_eq!(mgr.achievements.len(), 3);

        // Frame 1: trigger First Contact & advance Measured on Collector's Streak (2/5).
        core.poke8(0x0200_0000, 1);
        core.poke8(0x0200_0004, 2);
        core.poke8(0x0300_0008, 1);
        mgr.tick_frame(&core, &cfg);
        assert!(mgr.achievements[0].earned);
        assert!(!mgr.achievements[1].earned);
        assert_eq!(mgr.achievements[1].measured_current, Some(2));
        assert_eq!(mgr.achievements[1].measured_target, Some(5));
        assert!(!mgr.achievements[2].earned);

        // Frame 2 & 3: complete Collector's Streak + Steady Hold (3 frames total at 0x03000008 == 1).
        core.poke8(0x0200_0004, 5);
        mgr.tick_frame(&core, &cfg);
        assert!(mgr.achievements[1].earned);
        assert!(!mgr.achievements[2].earned);

        mgr.tick_frame(&core, &cfg);
        assert!(mgr.achievements[2].earned);
        assert_eq!(mgr.unlock_queue.len(), 3);
    }

    #[test]
    fn hardcore_activation_requires_enabled_logged_in_and_active_game() {
        let mut cfg = AppConfig {
            ra_enabled: true,
            ra_hardcore: true,
            ..Default::default()
        };
        let mut mgr = RetroAchievementsManager::new(&cfg);

        // Not logged in -> not active
        mgr.load_sample_demo_set();
        assert!(!mgr.is_hardcore_active(&cfg));

        // Logged in + active game -> active
        cfg.ra_username = "tester".to_string();
        cfg.ra_token = "tok123".to_string();
        assert!(mgr.is_hardcore_active(&cfg));

        // Close ROM -> not active
        mgr.on_rom_closed();
        assert!(!mgr.is_hardcore_active(&cfg));
    }

    #[test]
    fn mem_addr_addhits_subhits_mask_div_and_raw_addresses() {
        let mut core = test_core();

        // Raw GBA address 0x02000050 + bitwise mask & + integer division / + Trigger T:
        core.poke8(0x0200_0050, 0x3A); // 0x3A & 15 == 10; 10 / 2 == 5
        let mut mask_prog =
            TriggerProgram::parse("0xH02000050&15=10_T:0xH02000050&h0F/2=5").unwrap();
        assert!(mask_prog.evaluate_frame(&core).triggered);

        // AddHits (C:) + SubHits (D:) feeding target (3)
        let mut hits_prog =
            TriggerProgram::parse("C:0xH000060=1_D:0xH000061=1_0xH000062=1(3)").unwrap();
        core.poke8(0x0200_0060, 1); // +1 hit
        core.poke8(0x0200_0061, 0);
        core.poke8(0x0200_0062, 1); // +1 hit => effective 2
        assert!(!hits_prog.evaluate_frame(&core).triggered);

        core.poke8(0x0200_0060, 0);
        core.poke8(0x0200_0061, 1); // +1 subhit => effective stays balanced
        core.poke8(0x0200_0062, 1);
        assert!(!hits_prog.evaluate_frame(&core).triggered);

        core.poke8(0x0200_0060, 1); // +1 addhit => 2
        core.poke8(0x0200_0061, 0); // subhit stays 1
        core.poke8(0x0200_0062, 1); // 3rd hit: 2 + 3 - 1 = 4 >= 3!
        assert!(hits_prog.evaluate_frame(&core).triggered);
    }
}
