//! Optional basic-block JIT compiler for GBA ARM7TDMI.
//!
//! Gated behind the `jit` feature. Supports x86_64 and aarch64 host architectures via Cranelift.
//! Enforces W^X executable memory protection and falls back gracefully to the interpreter
//! if JIT initialization or memory allocation fails.

use crate::bus::SysBus;
use crate::cpu::{ARM_LUT, Arm7tdmi, THUMB_LUT};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockKey {
    pub pc: u32,
    pub is_thumb: bool,
}

pub type JitCompiledFn = unsafe extern "C" fn(cpu: *mut Arm7tdmi, bus: *mut SysBus) -> u64;

pub struct JitBlock {
    pub key: BlockKey,
    pub func_ptr: JitCompiledFn,
    pub end_pc: u32,
    pub cycle_count: u64,
    pub instr_count: usize,
    pub pages: Vec<u32>,
}

#[derive(Default)]
pub struct JitEngine {
    pub available: bool,
    pub enabled: bool,
    blocks: HashMap<BlockKey, JitBlock>,
    page_to_blocks: HashMap<u32, Vec<BlockKey>>,
    #[cfg(feature = "jit")]
    module: Option<cranelift_jit::JITModule>,
    #[cfg(feature = "jit")]
    ctx: Option<cranelift_codegen::Context>,
    #[cfg(feature = "jit")]
    func_ctx: Option<cranelift_frontend::FunctionBuilderContext>,
    #[allow(dead_code)]
    block_counter: u64,
}

impl JitEngine {
    pub fn new() -> Self {
        #[cfg(feature = "jit")]
        {
            let mut engine = Self::init_cranelift();
            engine.enabled = false; // Interpreter stays the default and the fallback!
            engine
        }
        #[cfg(not(feature = "jit"))]
        {
            Self {
                available: false,
                enabled: false,
                blocks: HashMap::new(),
                page_to_blocks: HashMap::new(),
                block_counter: 0,
            }
        }
    }

    #[cfg(feature = "jit")]
    fn init_cranelift() -> Self {
        use cranelift_codegen::settings::{self, Configurable};
        use cranelift_jit::{JITBuilder, JITModule};
        use cranelift_module::Module;

        let mut flag_builder = settings::builder();
        // Optimize for speed on host
        let _ = flag_builder.set("opt_level", "speed");

        let isa_builder = cranelift_native::builder();
        let isa = match isa_builder {
            Ok(b) => match b.finish(settings::Flags::new(flag_builder)) {
                Ok(isa) => isa,
                Err(e) => {
                    log::warn!(
                        "JIT: Failed to create target ISA: {e}; falling back to interpreter"
                    );
                    return Self::disabled();
                }
            },
            Err(e) => {
                log::warn!(
                    "JIT: Host architecture unsupported by Cranelift: {e}; falling back to interpreter"
                );
                return Self::disabled();
            }
        };

        let jit_builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
        let module = JITModule::new(jit_builder);

        let ctx = module.make_context();
        let func_ctx = cranelift_frontend::FunctionBuilderContext::new();

        log::info!("JIT: Cranelift basic block JIT compiler initialized successfully");

        Self {
            available: true,
            enabled: true,
            blocks: HashMap::new(),
            page_to_blocks: HashMap::new(),
            module: Some(module),
            ctx: Some(ctx),
            func_ctx: Some(func_ctx),
            block_counter: 0,
        }
    }

    pub fn disabled() -> Self {
        #[cfg(feature = "jit")]
        {
            Self {
                available: false,
                enabled: false,
                blocks: HashMap::new(),
                page_to_blocks: HashMap::new(),
                module: None,
                ctx: None,
                func_ctx: None,
                block_counter: 0,
            }
        }
        #[cfg(not(feature = "jit"))]
        {
            Self {
                available: false,
                enabled: false,
                blocks: HashMap::new(),
                page_to_blocks: HashMap::new(),
                block_counter: 0,
            }
        }
    }

    #[inline]
    pub fn get_block(&self, key: &BlockKey) -> Option<&JitBlock> {
        if !self.enabled || !self.available {
            return None;
        }
        self.blocks.get(key)
    }

    pub fn invalidate_page(&mut self, page: u32) {
        if let Some(keys) = self.page_to_blocks.remove(&page) {
            for key in keys {
                self.blocks.remove(&key);
            }
        }
    }

    pub fn clear(&mut self) {
        self.blocks.clear();
        self.page_to_blocks.clear();
    }

    pub fn compile_block(
        &mut self,
        cpu: &mut Arm7tdmi,
        bus: &mut SysBus,
        key: BlockKey,
    ) -> Option<&JitBlock> {
        if !self.available || !self.enabled {
            return None;
        }

        // Trace instructions up to block boundary
        let (instrs, total_cycles, pages) = Self::trace_block(cpu, bus, key)?;
        if instrs.is_empty() {
            return None;
        }

        let func_ptr = self.codegen_block(&instrs, key.is_thumb)?;

        let block = JitBlock {
            key,
            func_ptr,
            end_pc: cpu.gpr[15],
            cycle_count: total_cycles,
            instr_count: instrs.len(),
            pages: pages.clone(),
        };

        for page in pages {
            self.page_to_blocks.entry(page).or_default().push(key);
        }

        self.blocks.insert(key, block);
        self.blocks.get(&key)
    }

    fn trace_block(
        cpu: &Arm7tdmi,
        bus: &mut SysBus,
        key: BlockKey,
    ) -> Option<(Vec<u32>, u64, Vec<u32>)> {
        let mut cur_pc = key.pc;
        let mut instrs = Vec::new();
        let mut total_cycles = 0u64;
        let mut pages = Vec::new();
        let max_instrs = 32;

        for _ in 0..max_instrs {
            let page = cur_pc >> 14;
            if !pages.contains(&page) {
                pages.push(page);
            }

            if key.is_thumb {
                let h = bus.debug_read_16(cur_pc);
                instrs.push(h as u32);
                cur_pc = cur_pc.wrapping_add(2);
                total_cycles += 1; // Base cycle

                if is_thumb_block_boundary(h) {
                    break;
                }
            } else {
                let w = bus.debug_read_32(cur_pc);
                instrs.push(w);
                cur_pc = cur_pc.wrapping_add(4);
                total_cycles += 1; // Base cycle

                if is_arm_block_boundary(w) {
                    break;
                }
            }
        }

        let _ = cpu;
        Some((instrs, total_cycles, pages))
    }

    #[cfg(feature = "jit")]
    fn codegen_block(&mut self, instrs: &[u32], is_thumb: bool) -> Option<JitCompiledFn> {
        use cranelift_codegen::ir::{AbiParam, InstBuilder, types};
        use cranelift_frontend::FunctionBuilder;
        use cranelift_module::{FuncId, Linkage, Module};

        let module = self.module.as_mut()?;
        let ctx = self.ctx.as_mut()?;
        let func_ctx = self.func_ctx.as_mut()?;

        self.block_counter += 1;
        let func_name = format!("jit_block_{}", self.block_counter);

        // Signature: unsafe extern "C" fn(cpu: *mut Arm7tdmi, bus: *mut SysBus) -> u64
        let mut sig = module.make_signature();
        sig.params.push(AbiParam::new(types::I64)); // cpu
        sig.params.push(AbiParam::new(types::I64)); // bus
        sig.returns.push(AbiParam::new(types::I64)); // cycles

        let func_id: FuncId = match module.declare_function(&func_name, Linkage::Local, &sig) {
            Ok(id) => id,
            Err(e) => {
                log::warn!("JIT: Failed to declare function {func_name}: {e}");
                return None;
            }
        };

        ctx.func.signature = sig;
        ctx.func.name = cranelift_codegen::ir::UserFuncName::user(0, func_id.as_u32());

        let frontend_config = module.isa().frontend_config();
        {
            let mut builder = FunctionBuilder::new(&mut ctx.func, func_ctx);
            let entry_block = builder.create_block();
            builder.append_block_params_for_function_params(entry_block);
            builder.switch_to_block(entry_block);
            builder.seal_block(entry_block);

            let cpu_val = builder.block_params(entry_block)[0];
            let bus_val = builder.block_params(entry_block)[1];

            // Store instruction vector pointer and dispatch helper
            // For Cranelift codegen, we emit calls to the interpreter handlers for each instruction
            // in the basic block, unrolling the sequence and chaining them cleanly in host machine code.
            let helper_fn_ptr = if is_thumb {
                run_thumb_instr as *const () as usize
            } else {
                run_arm_instr as *const () as usize
            };

            let helper_imm = builder.ins().iconst(types::I64, helper_fn_ptr as i64);
            let mut total_cyc_val = builder.ins().iconst(types::I64, 0);

            let mut call_sig = module.make_signature();
            call_sig.params.push(AbiParam::new(types::I64)); // cpu
            call_sig.params.push(AbiParam::new(types::I64)); // bus
            call_sig.params.push(AbiParam::new(types::I32)); // instr
            call_sig.returns.push(AbiParam::new(types::I64)); // cycles
            let sig_ref = builder.import_signature(call_sig);

            for &instr in instrs {
                if let Some(cyc) = try_lower_ir(&mut builder, cpu_val, instr, is_thumb) {
                    total_cyc_val = builder.ins().iadd_imm(total_cyc_val, cyc);
                } else {
                    let instr_val = builder.ins().iconst(types::I32, instr as i64);
                    let call_inst = builder.ins().call_indirect(
                        sig_ref,
                        helper_imm,
                        &[cpu_val, bus_val, instr_val],
                    );
                    let step_cyc = builder.inst_results(call_inst)[0];
                    total_cyc_val = builder.ins().iadd(total_cyc_val, step_cyc);
                }
            }

            builder.ins().return_(&[total_cyc_val]);
            builder.finalize(frontend_config);
        }

        if let Err(e) = module.define_function(func_id, ctx) {
            log::warn!("JIT: Failed to define function {func_name}: {e}");
            module.clear_context(ctx);
            return None;
        }

        module.clear_context(ctx);

        if let Err(e) = module.finalize_definitions() {
            log::warn!("JIT: Failed to finalize function {func_name} (W^X protect failure): {e}");
            return None;
        }

        let code_ptr = module.get_finalized_function(func_id);
        // SAFETY: Cranelift finalized the function into executable memory with valid C ABI.
        let compiled_fn: JitCompiledFn = unsafe { std::mem::transmute(code_ptr) };
        Some(compiled_fn)
    }

    #[cfg(not(feature = "jit"))]
    fn codegen_block(&mut self, _instrs: &[u32], _is_thumb: bool) -> Option<JitCompiledFn> {
        None
    }
}

#[cfg(feature = "jit")]
fn try_lower_ir(
    builder: &mut cranelift_frontend::FunctionBuilder,
    cpu_val: cranelift_codegen::ir::Value,
    instr: u32,
    is_thumb: bool,
) -> Option<i64> {
    use cranelift_codegen::ir::{InstBuilder, MemFlags, Value, types};

    let load_gpr = |builder: &mut cranelift_frontend::FunctionBuilder, reg: usize| -> Value {
        let offset = (reg * 4) as i32;
        builder
            .ins()
            .load(types::I32, MemFlags::trusted(), cpu_val, offset)
    };

    let store_gpr = |builder: &mut cranelift_frontend::FunctionBuilder, reg: usize, val: Value| {
        let offset = (reg * 4) as i32;
        builder
            .ins()
            .store(MemFlags::trusted(), val, cpu_val, offset);
    };

    let advance_pc = |builder: &mut cranelift_frontend::FunctionBuilder, delta: i64| {
        let offset = 60i32; // gpr[15]
        let old_pc = builder
            .ins()
            .load(types::I32, MemFlags::trusted(), cpu_val, offset);
        let new_pc = builder.ins().iadd_imm(old_pc, delta);
        builder
            .ins()
            .store(MemFlags::trusted(), new_pc, cpu_val, offset);
    };

    if is_thumb {
        let h = instr as u16;
        // Format 5: Hi register ops (ADD Rd, Rs / MOV Rd, Rs where Rd != 15 and Rs != 15)
        if (h & 0xFC00) == 0x4400 {
            let op = (h >> 8) & 3;
            let h1 = (h >> 7) & 1;
            let h2 = (h >> 6) & 1;
            let rd = ((h1 << 3) | (h & 7)) as usize;
            let rs = ((h2 << 3) | ((h >> 3) & 7)) as usize;
            if rd < 15 && rs < 15 {
                match op {
                    0 => {
                        // ADD Rd, Rs
                        let v_d = load_gpr(builder, rd);
                        let v_s = load_gpr(builder, rs);
                        let res = builder.ins().iadd(v_d, v_s);
                        store_gpr(builder, rd, res);
                        advance_pc(builder, 2);
                        return Some(1);
                    }
                    2 => {
                        // MOV Rd, Rs
                        let v_s = load_gpr(builder, rs);
                        store_gpr(builder, rd, v_s);
                        advance_pc(builder, 2);
                        return Some(1);
                    }
                    _ => {}
                }
            }
        }
        // Format 13: ADD SP, #imm / SUB SP, #imm
        if (h & 0xFF00) == 0xB000 {
            let s_bit = (h >> 7) & 1;
            let imm = ((h & 0x7F) as i64) * 4;
            let sp = load_gpr(builder, 13);
            let res = if s_bit == 0 {
                builder.ins().iadd_imm(sp, imm)
            } else {
                builder.ins().iadd_imm(sp, -imm)
            };
            store_gpr(builder, 13, res);
            advance_pc(builder, 2);
            return Some(1);
        }
    } else {
        // ARM: check condition AL (0xE)
        let cond = (instr >> 28) & 0xF;
        if cond != 0xE {
            return None;
        }

        // Data processing: bits 27..26 == 00
        if (instr & 0x0C00_0000) == 0 {
            let s_bit = (instr & 0x0010_0000) != 0;
            let i_bit = (instr & 0x0200_0000) != 0;
            let opcode = (instr >> 21) & 0xF;
            let rn = ((instr >> 16) & 0xF) as usize;
            let rd = ((instr >> 12) & 0xF) as usize;

            // Only lower if destination is not PC and S bit is 0 (flags unchanged)
            if !s_bit && rd < 15 {
                let op2: Value = if i_bit {
                    let rot = ((instr >> 8) & 0xF) * 2;
                    let imm8 = (instr & 0xFF) as u32;
                    let imm_val = imm8.rotate_right(rot);
                    builder.ins().iconst(types::I32, imm_val as i64)
                } else {
                    // Register with no shift (bits 11..4 == 0)
                    if (instr & 0xFF0) == 0 {
                        let rm = (instr & 0xF) as usize;
                        if rm >= 15 {
                            return None;
                        }
                        load_gpr(builder, rm)
                    } else {
                        return None;
                    }
                };

                let res: Value = match opcode {
                    0x0 => {
                        // AND
                        if rn >= 15 {
                            return None;
                        }
                        let v_n = load_gpr(builder, rn);
                        builder.ins().band(v_n, op2)
                    }
                    0x1 => {
                        // EOR
                        if rn >= 15 {
                            return None;
                        }
                        let v_n = load_gpr(builder, rn);
                        builder.ins().bxor(v_n, op2)
                    }
                    0x2 => {
                        // SUB
                        if rn >= 15 {
                            return None;
                        }
                        let v_n = load_gpr(builder, rn);
                        builder.ins().isub(v_n, op2)
                    }
                    0x4 => {
                        // ADD
                        if rn >= 15 {
                            return None;
                        }
                        let v_n = load_gpr(builder, rn);
                        builder.ins().iadd(v_n, op2)
                    }
                    0xC => {
                        // ORR
                        if rn >= 15 {
                            return None;
                        }
                        let v_n = load_gpr(builder, rn);
                        builder.ins().bor(v_n, op2)
                    }
                    0xD => {
                        // MOV
                        op2
                    }
                    0xE => {
                        // BIC (Rn & ~op2)
                        if rn >= 15 {
                            return None;
                        }
                        let v_n = load_gpr(builder, rn);
                        let not_op2 = builder.ins().bnot(op2);
                        builder.ins().band(v_n, not_op2)
                    }
                    _ => return None,
                };

                store_gpr(builder, rd, res);
                advance_pc(builder, 4);
                return Some(1);
            }
        }
    }

    None
}

// C-ABI helper entrypoints for JIT execution
/// # Safety
/// `cpu_ptr` and `bus_ptr` must be valid, aligned, non-null pointers to live `Arm7tdmi` and `SysBus` instances.
pub unsafe extern "C" fn run_arm_instr(
    cpu_ptr: *mut Arm7tdmi,
    bus_ptr: *mut SysBus,
    instr: u32,
) -> u64 {
    let cpu = unsafe { &mut *cpu_ptr };
    let bus = unsafe { &mut *bus_ptr };

    if cpu.halted || cpu.stopped || bus.hw.halt_requested || bus.hw.stop_requested {
        return 0;
    }

    let cond = (instr >> 28) & 0xF;
    if cond != 0xE && !cpu.check_cond(cond) {
        cpu.gpr[15] = cpu.gpr[15].wrapping_add(4);
        return 1;
    }
    let lut_idx = (((instr >> 16) & 0xFF0) | ((instr >> 4) & 0x00F)) as usize;
    let c = ARM_LUT[lut_idx](cpu, bus, instr);
    if !cpu.pipeline_flushed {
        cpu.gpr[15] = cpu.gpr[15].wrapping_add(4);
    }
    c.max(1)
}

/// # Safety
/// `cpu_ptr` and `bus_ptr` must be valid, aligned, non-null pointers to live `Arm7tdmi` and `SysBus` instances.
pub unsafe extern "C" fn run_thumb_instr(
    cpu_ptr: *mut Arm7tdmi,
    bus_ptr: *mut SysBus,
    instr: u32,
) -> u64 {
    let cpu = unsafe { &mut *cpu_ptr };
    let bus = unsafe { &mut *bus_ptr };

    if cpu.halted || cpu.stopped || bus.hw.halt_requested || bus.hw.stop_requested {
        return 0;
    }

    let lut_idx = ((instr >> 6) & 0x3FF) as usize;
    let c = THUMB_LUT[lut_idx](cpu, bus, instr as u16);
    if !cpu.pipeline_flushed {
        cpu.gpr[15] = cpu.gpr[15].wrapping_add(2);
    }
    c.max(1)
}

#[inline]
fn is_arm_block_boundary(instr: u32) -> bool {
    let cond = (instr >> 28) & 0xF;
    if cond == 0xF {
        return true; // Undefined / coprocessor
    }
    // B / BL: bits 27..25 == 101
    if (instr & 0x0E00_0000) == 0x0A00_0000 {
        return true;
    }
    // BX: 0x012F_FF10
    if (instr & 0x0FFF_FFF0) == 0x012F_FF10 {
        return true;
    }
    // SWI: bits 27..24 == 1111
    if (instr & 0x0F00_0000) == 0x0F00_0000 {
        return true;
    }
    // Data processing with Rd == 15
    let is_data_proc = (instr & 0x0C00_0000) == 0;
    let rd = (instr >> 12) & 0xF;
    if is_data_proc && rd == 15 {
        return true;
    }
    // LDR with Rd == 15
    let is_ldr = (instr & 0x0C10_0000) == 0x0410_0000;
    if is_ldr && rd == 15 {
        return true;
    }
    // LDM with PC in rlist: bit 15 of rlist set
    let is_ldm = (instr & 0x0E10_0000) == 0x0810_0000;
    if is_ldm && (instr & (1 << 15)) != 0 {
        return true;
    }
    // MSR CPSR (mode change)
    let is_msr_cpsr = (instr & 0x0FB0_0000) == 0x0120_0000;
    if is_msr_cpsr {
        return true;
    }
    false
}

#[inline]
fn is_thumb_block_boundary(instr: u16) -> bool {
    let top8 = (instr >> 8) as u8;
    // B cond (0xD0..=0xDF, except 0xDF which is SWI)
    if (0xD0..=0xDE).contains(&top8) {
        return true;
    }
    // SWI (0xDF)
    if top8 == 0xDF {
        return true;
    }
    // Unconditional branch (0xE0..=0xE7)
    if (0xE0..=0xE7).contains(&top8) {
        return true;
    }
    // BL prefix / suffix (0xF0..=0xFF)
    if (0xF0..=0xFF).contains(&top8) {
        return true;
    }
    // BX / BLX (top 10 bits: 01000111xx)
    if (instr & 0xFF80) == 0x4700 {
        return true;
    }
    // High register operations with Rd == PC (top 10 bits: 010001xx00 where Rd/Hd == 15)
    if (instr & 0xFC00) == 0x4400 && (instr & 0x0007) == 7 && (instr & 0x0080) != 0 {
        return true; // ADD/MOV with PC as destination
    }
    // POP {..., PC} (top 8 bits: 0xBD)
    if (instr & 0xFF00) == 0xBD00 {
        return true;
    }
    false
}
