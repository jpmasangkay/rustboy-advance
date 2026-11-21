//! Optional basic-block JIT compiler for GBA ARM7TDMI.
//!
//! Gated behind the `jit` feature. Supports x86_64 and aarch64 host architectures via Cranelift.
//! Enforces W^X executable memory protection and falls back gracefully to the interpreter
//! if JIT initialization or memory allocation fails.

use alloc::vec::Vec;

use crate::bus::SysBus;
use crate::cpu::Arm7tdmi;

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

#[cfg(not(feature = "jit"))]
#[derive(Default)]
pub struct JitEngine {
    pub available: bool,
    pub enabled: bool,
}

#[cfg(not(feature = "jit"))]
impl JitEngine {
    pub fn new() -> Self {
        Self::disabled()
    }

    pub fn disabled() -> Self {
        Self {
            available: false,
            enabled: false,
        }
    }

    #[inline]
    pub fn get_block(&self, _key: &BlockKey) -> Option<&JitBlock> {
        None
    }

    #[inline]
    pub fn invalidate_page(&mut self, _page: u32) {}

    #[inline]
    pub fn clear(&mut self) {}

    #[inline]
    pub fn compile_block(
        &mut self,
        _cpu: &mut Arm7tdmi,
        _bus: &mut SysBus,
        _key: BlockKey,
    ) -> Option<&JitBlock> {
        None
    }
}

#[cfg(feature = "jit")]
mod cranelift_backend {
    use super::*;
    use alloc::format;
    use std::collections::HashMap;

    const MAX_JIT_BLOCKS: usize = 4096;
    const JIT_EXIT_BIT: u64 = 1u64 << 63;

    type TracedBlock = (Vec<(u32, u32, u64)>, u32, u64, Vec<u32>);

    #[derive(Default)]
    pub struct JitEngine {
        pub available: bool,
        pub enabled: bool,
        blocks: HashMap<BlockKey, JitBlock>,
        page_to_blocks: HashMap<u32, Vec<BlockKey>>,
        module: Option<cranelift_jit::JITModule>,
        ctx: Option<cranelift_codegen::Context>,
        func_ctx: Option<cranelift_frontend::FunctionBuilderContext>,
        block_counter: u64,
    }

    impl JitEngine {
        pub fn new() -> Self {
            let mut engine = Self::init_cranelift();
            engine.enabled = false; // Interpreter stays the default and the fallback!
            engine
        }

        fn init_cranelift() -> Self {
            use cranelift_codegen::settings::{self, Configurable};
            use cranelift_jit::{JITBuilder, JITModule};
            use cranelift_module::Module;

            let mut flag_builder = settings::builder();
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

        #[inline]
        pub fn get_block(&self, key: &BlockKey) -> Option<&JitBlock> {
            if !self.enabled || !self.available {
                return None;
            }
            self.blocks.get(key)
        }

        pub fn invalidate_page(&mut self, page: u32) {
            let canon = SysBus::canonical_code_page(page);
            if let Some(keys) = self.page_to_blocks.remove(&canon) {
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
            _cpu: &mut Arm7tdmi,
            bus: &mut SysBus,
            key: BlockKey,
        ) -> Option<&JitBlock> {
            if !self.available || !self.enabled || self.block_counter as usize >= MAX_JIT_BLOCKS {
                return None;
            }

            let region = (key.pc >> 24) & 0xFF;
            if !matches!(region, 0x02 | 0x03 | 0x08..=0x0D) {
                return None;
            }

            let (traced, end_pc, total_cycles, pages) = Self::trace_block(bus, key)?;
            if traced.is_empty() {
                return None;
            }

            let allow_ir = !bus.prefetch.enabled;
            let func_ptr = self.codegen_block(&traced, bus, key.is_thumb, allow_ir)?;

            let block = JitBlock {
                key,
                func_ptr,
                end_pc,
                cycle_count: total_cycles,
                instr_count: traced.len(),
                pages: pages.clone(),
            };

            for page in pages {
                self.page_to_blocks.entry(page).or_default().push(key);
            }

            self.blocks.insert(key, block);
            self.blocks.get(&key)
        }

        fn trace_block(bus: &SysBus, key: BlockKey) -> Option<TracedBlock> {
            let mut cur_pc = key.pc;
            let mut traced = Vec::new();
            let mut total_cycles = 0u64;
            let mut pages = Vec::new();
            let max_instrs = 32;
            let step = if key.is_thumb { 2u32 } else { 4u32 };
            let width = if key.is_thumb { 1usize } else { 2usize };

            for _ in 0..max_instrs {
                let region = (cur_pc >> 24) & 0xFF;
                if !matches!(region, 0x02 | 0x03 | 0x08..=0x0D) {
                    break;
                }
                // Stop if pipeline fetch at cur_pc + 2*step leaves fastmem/ROM or crosses 128 KiB ROM boundary.
                let fetch_pc = cur_pc.wrapping_add(step * 2);
                let fetch_region = (fetch_pc >> 24) & 0xFF;
                if !matches!(fetch_region, 0x02 | 0x03 | 0x08..=0x0D) {
                    break;
                }
                let page = SysBus::canonical_code_page((cur_pc >> 14) & 0x3FFF);
                if !pages.contains(&page) {
                    pages.push(page);
                }
                let fetch_page = SysBus::canonical_code_page((fetch_pc >> 14) & 0x3FFF);
                if !pages.contains(&fetch_page) {
                    pages.push(fetch_page);
                }

                let is_rom = (0x08..=0x0D).contains(&fetch_region);
                let fetch_cyc = if is_rom && (fetch_pc & 0x1FFFF) == 0 {
                    bus.waitstate_table[fetch_region as usize][width][0] as u64
                } else {
                    bus.waitstate_table[fetch_region as usize][width][1] as u64
                };

                if key.is_thumb {
                    let h = bus.debug_read_16(cur_pc);
                    traced.push((cur_pc, h as u32, fetch_cyc));
                    cur_pc = cur_pc.wrapping_add(2);
                    // Conservative upper bound so `step_jit` never executes a block across a
                    // pending scheduler event (e.g. PpuHBlank / TimerOverflow).
                    let worst_extra: u64 = if (h & 0xF000) == 0xC000 || (h & 0xF600) == 0xB400 {
                        // LDMIA / STMIA / PUSH / POP: up to 9 registers + bus waitstates
                        160
                    } else if (h & 0xFC00) == 0x4400 || (h & 0xFF00) == 0xB000 {
                        // Pure ALU / SP adjust
                        4
                    } else {
                        // Loads, stores, branches, multiplies, SWIs
                        32
                    };
                    total_cycles += fetch_cyc.max(1) + worst_extra;
                    if is_thumb_block_boundary(h) {
                        break;
                    }
                } else {
                    let w = bus.debug_read_32(cur_pc);
                    traced.push((cur_pc, w, fetch_cyc));
                    cur_pc = cur_pc.wrapping_add(4);
                    let worst_extra: u64 = if (w & 0x0E00_0000) == 0x0800_0000 {
                        // LDM / STM: up to 16 registers + bus waitstates
                        280
                    } else if (w & 0x0E00_0000) == 0x0000_0000 && (w & 0x0000_0090) != 0x0000_0090 {
                        // Data processing / shifts
                        4
                    } else {
                        // Loads, stores, halfword transfers, branches, multiplies, SWIs
                        32
                    };
                    total_cycles += fetch_cyc.max(1) + worst_extra;
                    if is_arm_block_boundary(w) {
                        break;
                    }
                }
            }

            Some((traced, cur_pc, total_cycles, pages))
        }

        fn codegen_block(
            &mut self,
            traced: &[(u32, u32, u64)],
            bus: &SysBus,
            is_thumb: bool,
            allow_ir: bool,
        ) -> Option<JitCompiledFn> {
            use cranelift_codegen::ir::condcodes::IntCC;
            use cranelift_codegen::ir::{AbiParam, InstBuilder, types};
            use cranelift_frontend::FunctionBuilder;
            use cranelift_module::{FuncId, Linkage, Module};

            let module = self.module.as_mut()?;
            let ctx = self.ctx.as_mut()?;
            let func_ctx = self.func_ctx.as_mut()?;

            self.block_counter += 1;
            let func_name = format!("jit_block_{}", self.block_counter);
            let ptr_ty = module.isa().pointer_type();

            let mut sig = module.make_signature();
            sig.params.push(AbiParam::new(ptr_ty)); // cpu
            sig.params.push(AbiParam::new(ptr_ty)); // bus
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
                let exit_block = builder.create_block();
                builder.append_block_params_for_function_params(entry_block);
                builder.append_block_param(exit_block, types::I64);

                builder.switch_to_block(entry_block);
                builder.seal_block(entry_block);

                let cpu_val = builder.block_params(entry_block)[0];
                let bus_val = builder.block_params(entry_block)[1];

                let step_fn_ptr = run_step_checked as *const () as usize;
                let step_imm = builder.ins().iconst(ptr_ty, step_fn_ptr as i64);

                let mut step_sig = module.make_signature();
                step_sig.params.push(AbiParam::new(ptr_ty)); // cpu
                step_sig.params.push(AbiParam::new(ptr_ty)); // bus
                step_sig.returns.push(AbiParam::new(types::I64)); // cycles | exit_bit
                let step_sig_ref = builder.import_signature(step_sig);

                let sync_fn_ptr = sync_pipeline_after_ir as *const () as usize;
                let sync_imm = builder.ins().iconst(ptr_ty, sync_fn_ptr as i64);

                let mut sync_sig = module.make_signature();
                sync_sig.params.push(AbiParam::new(ptr_ty)); // cpu
                sync_sig.params.push(AbiParam::new(ptr_ty)); // bus
                sync_sig.params.push(AbiParam::new(types::I32)); // p0
                sync_sig.params.push(AbiParam::new(types::I32)); // p1
                sync_sig.params.push(AbiParam::new(types::I32)); // open_bus_latch
                let sync_sig_ref = builder.import_signature(sync_sig);

                let mut total_cyc_val = builder.ins().iconst(types::I64, 0);
                let step_bytes = if is_thumb { 2u32 } else { 4u32 };
                let mut ir_dirty_end_pc: Option<u32> = None;

                let emit_sync = |builder: &mut FunctionBuilder,
                                 end_pc: u32,
                                 cpu_val,
                                 bus_val,
                                 sync_sig_ref,
                                 sync_imm| {
                    let (p0, p1, latch) = if is_thumb {
                        let h0 = bus.debug_read_16(end_pc) as u32;
                        let h1 = bus.debug_read_16(end_pc.wrapping_add(2)) as u32;
                        (h0, h1, h1 | (h1 << 16))
                    } else {
                        let w0 = bus.debug_read_32(end_pc);
                        let w1 = bus.debug_read_32(end_pc.wrapping_add(4));
                        (w0, w1, w1)
                    };
                    let p0_v = builder.ins().iconst(types::I32, p0 as i64);
                    let p1_v = builder.ins().iconst(types::I32, p1 as i64);
                    let latch_v = builder.ins().iconst(types::I32, latch as i64);
                    builder.ins().call_indirect(
                        sync_sig_ref,
                        sync_imm,
                        &[cpu_val, bus_val, p0_v, p1_v, latch_v],
                    );
                };

                for (idx, &(pc, instr, fetch_cyc)) in traced.iter().enumerate() {
                    let is_last = idx + 1 == traced.len();
                    if allow_ir && try_lower_ir(&mut builder, cpu_val, instr, is_thumb) {
                        total_cyc_val = builder.ins().iadd_imm_s(total_cyc_val, fetch_cyc as i64);
                        ir_dirty_end_pc = Some(pc.wrapping_add(step_bytes));
                    } else {
                        if let Some(end_pc) = ir_dirty_end_pc.take() {
                            emit_sync(
                                &mut builder,
                                end_pc,
                                cpu_val,
                                bus_val,
                                sync_sig_ref,
                                sync_imm,
                            );
                        }
                        let call_inst = builder.ins().call_indirect(
                            step_sig_ref,
                            step_imm,
                            &[cpu_val, bus_val],
                        );
                        let raw_ret = builder.inst_results(call_inst)[0];
                        let mask = builder.ins().iconst(types::I64, (!JIT_EXIT_BIT) as i64);
                        let step_cyc = builder.ins().band(raw_ret, mask);
                        total_cyc_val = builder.ins().iadd(total_cyc_val, step_cyc);

                        if !is_last {
                            let exit_flag = builder.ins().band_imm_s(raw_ret, JIT_EXIT_BIT as i64);
                            let should_exit =
                                builder.ins().icmp_imm_s(IntCC::NotEqual, exit_flag, 0);
                            let cont_block = builder.create_block();
                            builder.ins().brif(
                                should_exit,
                                exit_block,
                                &[total_cyc_val.into()],
                                cont_block,
                                &[],
                            );
                            builder.switch_to_block(cont_block);
                            builder.seal_block(cont_block);
                        }
                    }
                }

                if let Some(end_pc) = ir_dirty_end_pc.take() {
                    emit_sync(
                        &mut builder,
                        end_pc,
                        cpu_val,
                        bus_val,
                        sync_sig_ref,
                        sync_imm,
                    );
                }

                builder.ins().jump(exit_block, &[total_cyc_val.into()]);
                builder.switch_to_block(exit_block);
                builder.seal_block(exit_block);
                let final_cyc = builder.block_params(exit_block)[0];
                builder.ins().return_(&[final_cyc]);
                builder.finalize(frontend_config);
            }

            if let Err(e) = module.define_function(func_id, ctx) {
                log::warn!("JIT: Failed to define function {func_name}: {e}");
                module.clear_context(ctx);
                return None;
            }

            module.clear_context(ctx);

            if let Err(e) = module.finalize_definitions() {
                log::warn!(
                    "JIT: Failed to finalize function {func_name} (W^X protect failure): {e}"
                );
                return None;
            }

            let code_ptr = module.get_finalized_function(func_id);
            // SAFETY: Cranelift finalized the function into executable memory with valid C ABI.
            let compiled_fn: JitCompiledFn = unsafe { core::mem::transmute(code_ptr) };
            Some(compiled_fn)
        }
    }

    fn try_lower_ir(
        builder: &mut cranelift_frontend::FunctionBuilder,
        cpu_val: cranelift_codegen::ir::Value,
        instr: u32,
        is_thumb: bool,
    ) -> bool {
        use cranelift_codegen::ir::{InstBuilder, MemFlagsData, Value, types};

        let trusted_flags = MemFlagsData::trusted();

        let load_gpr = |builder: &mut cranelift_frontend::FunctionBuilder, reg: usize| -> Value {
            let offset = (reg * 4) as i32;
            builder
                .ins()
                .load(types::I32, trusted_flags, cpu_val, offset)
        };

        let store_gpr =
            |builder: &mut cranelift_frontend::FunctionBuilder, reg: usize, val: Value| {
                let offset = (reg * 4) as i32;
                builder.ins().store(trusted_flags, val, cpu_val, offset);
            };

        let advance_pc = |builder: &mut cranelift_frontend::FunctionBuilder, delta: i64| {
            let offset = 60i32; // gpr[15]
            let old_pc = builder
                .ins()
                .load(types::I32, trusted_flags, cpu_val, offset);
            let new_pc = builder.ins().iadd_imm_s(old_pc, delta);
            builder.ins().store(trusted_flags, new_pc, cpu_val, offset);
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
                            let v_d = load_gpr(builder, rd);
                            let v_s = load_gpr(builder, rs);
                            let res = builder.ins().iadd(v_d, v_s);
                            store_gpr(builder, rd, res);
                            advance_pc(builder, 2);
                            return true;
                        }
                        2 => {
                            let v_s = load_gpr(builder, rs);
                            store_gpr(builder, rd, v_s);
                            advance_pc(builder, 2);
                            return true;
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
                    builder.ins().iadd_imm_s(sp, imm)
                } else {
                    builder.ins().iadd_imm_s(sp, -imm)
                };
                store_gpr(builder, 13, res);
                advance_pc(builder, 2);
                return true;
            }
        } else {
            let cond = (instr >> 28) & 0xF;
            if cond != 0xE {
                return false;
            }
            if (instr & 0x0C00_0000) == 0 {
                let s_bit = (instr & 0x0010_0000) != 0;
                let i_bit = (instr & 0x0200_0000) != 0;
                let opcode = (instr >> 21) & 0xF;
                let rn = ((instr >> 16) & 0xF) as usize;
                let rd = ((instr >> 12) & 0xF) as usize;

                if !s_bit && rd < 15 {
                    let op2: Value = if i_bit {
                        let rot = ((instr >> 8) & 0xF) * 2;
                        let imm8 = instr & 0xFF;
                        let imm_val = imm8.rotate_right(rot);
                        builder.ins().iconst(types::I32, imm_val as i64)
                    } else if (instr & 0xFF0) == 0 {
                        let rm = (instr & 0xF) as usize;
                        if rm >= 15 {
                            return false;
                        }
                        load_gpr(builder, rm)
                    } else {
                        return false;
                    };

                    let res: Value = match opcode {
                        0x0 => {
                            if rn >= 15 {
                                return false;
                            }
                            let v_n = load_gpr(builder, rn);
                            builder.ins().band(v_n, op2)
                        }
                        0x1 => {
                            if rn >= 15 {
                                return false;
                            }
                            let v_n = load_gpr(builder, rn);
                            builder.ins().bxor(v_n, op2)
                        }
                        0x2 => {
                            if rn >= 15 {
                                return false;
                            }
                            let v_n = load_gpr(builder, rn);
                            builder.ins().isub(v_n, op2)
                        }
                        0x4 => {
                            if rn >= 15 {
                                return false;
                            }
                            let v_n = load_gpr(builder, rn);
                            builder.ins().iadd(v_n, op2)
                        }
                        0xC => {
                            if rn >= 15 {
                                return false;
                            }
                            let v_n = load_gpr(builder, rn);
                            builder.ins().bor(v_n, op2)
                        }
                        0xD => op2,
                        0xE => {
                            if rn >= 15 {
                                return false;
                            }
                            let v_n = load_gpr(builder, rn);
                            let not_op2 = builder.ins().bnot(op2);
                            builder.ins().band(v_n, not_op2)
                        }
                        _ => return false,
                    };

                    store_gpr(builder, rd, res);
                    advance_pc(builder, 4);
                    return true;
                }
            }
        }

        false
    }

    unsafe extern "C" fn sync_pipeline_after_ir(
        cpu_ptr: *mut Arm7tdmi,
        bus_ptr: *mut SysBus,
        p0: u32,
        p1: u32,
        open_bus_latch: u32,
    ) {
        let cpu = unsafe { &mut *cpu_ptr };
        let bus = unsafe { &mut *bus_ptr };
        cpu.pipeline[0] = p0;
        cpu.pipeline[1] = p1;
        cpu.pipeline_flushed = false;
        cpu.flush_cycles = 0;
        bus.open_bus_latch = open_bus_latch;
    }

    unsafe extern "C" fn run_step_checked(cpu_ptr: *mut Arm7tdmi, bus_ptr: *mut SysBus) -> u64 {
        let cpu = unsafe { &mut *cpu_ptr };
        let bus = unsafe { &mut *bus_ptr };

        if cpu.halted
            || cpu.stopped
            || bus.hw.halt_requested
            || bus.hw.stop_requested
            || bus.hw.active_dma.is_some()
        {
            return JIT_EXIT_BIT;
        }

        bus.debug_pc = cpu.current_exec_pc();
        let c = cpu.step(bus).max(1);
        let need_exit = cpu.pipeline_flushed
            || cpu.halted
            || cpu.stopped
            || bus.hw.halt_requested
            || bus.hw.stop_requested
            || bus.hw.active_dma.is_some()
            || bus.invalidate_all_jit
            || !bus.dirty_code_pages.is_empty()
            || bus.force_next_fetch_nseq
            || bus.irq_sync_delay > 0
            || (bus.hw.ime != 0 && (bus.hw.ie & bus.hw.if_flag) != 0 && !cpu.is_irq_disabled());
        if need_exit { c | JIT_EXIT_BIT } else { c }
    }

    #[inline]
    fn is_arm_block_boundary(instr: u32) -> bool {
        let cond = (instr >> 28) & 0xF;
        if cond == 0xF {
            return true;
        }
        // B / BL
        if (instr & 0x0E00_0000) == 0x0A00_0000 {
            return true;
        }
        // BX
        if (instr & 0x0FFF_FFF0) == 0x012F_FF10 {
            return true;
        }
        // SWI & Coprocessor/Undefined
        if (instr & 0x0C00_0000) == 0x0C00_0000 {
            return true;
        }
        // Media/Undefined (bits 27..25 == 011, bit 4 == 1)
        if (instr & 0x0E00_0010) == 0x0600_0010 {
            return true;
        }
        let rd = (instr >> 12) & 0xF;
        // Data processing with Rd == 15, or MRS/MSR/unallocated S=0 opcodes 0x8..0xB
        if (instr & 0x0C00_0000) == 0 {
            if rd == 15 {
                return true;
            }
            // MSR CPSR (register or immediate) and unallocated S=0 0x8..=0xB
            if (instr & 0x0D90_0000) == 0x0100_0000 {
                return true;
            }
            // Undefined halfword store L=0, sh in {2,3}
            if (instr & 0x0210_0090) == 0x0000_0090 && (instr & 0x0000_0060) >= 0x40 {
                return true;
            }
        }
        // LDR with Rd == 15
        if (instr & 0x0C10_0000) == 0x0410_0000 && rd == 15 {
            return true;
        }
        // LDM with PC in rlist or empty rlist (which loads PC)
        if (instr & 0x0E10_0000) == 0x0810_0000
            && ((instr & (1 << 15)) != 0 || (instr & 0xFFFF) == 0)
        {
            return true;
        }
        false
    }

    #[inline]
    fn is_thumb_block_boundary(instr: u16) -> bool {
        let top8 = (instr >> 8) as u8;
        if (0xD0..=0xFF).contains(&top8) {
            // B cond, Undefined (0xDE), SWI (0xDF), B uncond, BL prefix/suffix
            return true;
        }
        // BX / BLX
        if (instr & 0xFF80) == 0x4700 {
            return true;
        }
        // High register ops with Rd == PC
        if (instr & 0xFC00) == 0x4400 && (instr & 0x0007) == 7 && (instr & 0x0080) != 0 {
            return true;
        }
        // POP {..., PC} or empty POP {} (which loads PC)
        if (instr & 0xFF00) == 0xBD00 || instr == 0xBC00 {
            return true;
        }
        // Empty LDMIA Rb!, {} (which loads PC)
        if (instr & 0xF8FF) == 0xC800 {
            return true;
        }
        false
    }
}

#[cfg(feature = "jit")]
pub use cranelift_backend::JitEngine;
