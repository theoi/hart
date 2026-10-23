//! The RV64 hart: architectural state, the run loop, memory access, and trap entry.

pub mod csr;
pub mod decode;
pub mod exec;
pub mod fp;
pub mod mmu;
pub mod pmp;
pub mod trap;

use crate::bus::Bus;
use crate::trace::{Commit, MemWrite, RegWrite, TraceSink};
pub use mmu::Access;
pub use trap::{Exception, ExceptionCause, Interrupt, Trap};

/// Privilege mode. The discriminant is the 2-bit encoding used in `mstatus.MPP`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Priv {
    User = 0,
    Supervisor = 1,
    Machine = 3,
}

impl Priv {
    /// Decode a 2-bit mode field. The reserved value 2 maps to User (hart's WARL choice
    /// for `mstatus.MPP` never stores 2, so this only matters for robustness).
    #[inline]
    pub fn from_bits(bits: u64) -> Priv {
        match bits & 3 {
            3 => Priv::Machine,
            1 => Priv::Supervisor,
            _ => Priv::User,
        }
    }
}

/// What one call to [`Cpu::step`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepResult {
    /// An instruction retired.
    Retired,
    /// An exception or interrupt was taken (no instruction retired).
    Trapped(Trap),
    /// The hart is stalled in WFI with no enabled interrupt pending.
    Waiting,
}

/// Per-profile CPU settings.
#[derive(Clone, Debug, Default)]
pub struct CpuConfig {
    pub mvendorid: u64,
    pub marchid: u64,
    pub mimpid: u64,
    /// Spike-compatible interrupt polling for lockstep runs. Interrupts are taken only
    /// when [`Cpu::irq_poll`] is set (the board sets it at chunk start; the CPU sets it
    /// after every Zicsr instruction, MRET, and SRET) and immediately before every Zicsr
    /// instruction. When false, interrupts are polled before every instruction.
    pub lockstep_irq: bool,
    /// Reset PMP entry 0 to NAPOT over all memory with R/W/X, as Spike does. When false,
    /// all PMP entries reset to off (the spec default).
    pub pmp_reset_all_access: bool,
}

impl CpuConfig {
    /// Settings matching Spike, for differential testing.
    pub fn spike() -> Self {
        CpuConfig { mvendorid: 0, marchid: 5, mimpid: 0, lockstep_irq: true, pmp_reset_all_access: true }
    }
}

pub struct Cpu {
    /// Integer registers. `x[0]` is kept at zero.
    pub x: [u64; 32],
    pub pc: u64,
    pub mode: Priv,
    pub csr: csr::CsrFile,
    /// F/D state, owned by the FPU module.
    pub fp: fp::FpState,
    pub mmu: mmu::Mmu,
    pub pmp: pmp::Pmp,
    /// LR/SC reservation: the physical address of the reserved doubleword-aligned granule.
    pub reservation: Option<u64>,
    /// True while stalled in WFI.
    pub waiting: bool,
    /// True if the instruction retired by the last `step` was WFI.
    pub wfi_retired: bool,
    /// Lockstep mode only: poll interrupts at the next step (see [`CpuConfig::lockstep_irq`]).
    pub irq_poll: bool,
    pub config: CpuConfig,
    tracer: Option<Box<dyn TraceSink>>,
    commit: Commit,
}

impl Cpu {
    /// A hart in its reset state: M-mode, `pc = reset_pc`, everything else zero.
    pub fn new(reset_pc: u64) -> Self {
        Cpu {
            x: [0; 32],
            pc: reset_pc,
            mode: Priv::Machine,
            csr: csr::CsrFile::new(),
            fp: fp::FpState::default(),
            mmu: mmu::Mmu::new(),
            pmp: pmp::Pmp::new(),
            reservation: None,
            waiting: false,
            wfi_retired: false,
            irq_poll: false,
            config: CpuConfig::default(),
            tracer: None,
            commit: Commit::default(),
        }
    }

    /// A hart in its reset state with the given profile settings.
    pub fn with_config(reset_pc: u64, config: CpuConfig) -> Self {
        let mut cpu = Cpu::new(reset_pc);
        cpu.set_config(config);
        cpu
    }

    /// Change the profile settings, applying their reset-time effects (PMP reset state).
    /// Call before the hart starts running.
    pub fn set_config(&mut self, config: CpuConfig) {
        if config.pmp_reset_all_access {
            self.pmp.reset_all_access();
        } else {
            self.pmp = pmp::Pmp::new();
        }
        self.mmu.flush();
        self.config = config;
    }

    /// Install or remove a commit-log sink. Tracing slows execution.
    pub fn set_tracer(&mut self, tracer: Option<Box<dyn TraceSink>>) {
        self.tracer = tracer;
    }

    pub fn tracer_mut(&mut self) -> Option<&mut (dyn TraceSink + 'static)> {
        self.tracer.as_deref_mut()
    }

    #[inline]
    fn tracing(&self) -> bool {
        self.tracer.is_some()
    }

    // ---- register access (used by exec and the FP module) ----

    #[inline]
    pub fn x(&self, r: u32) -> u64 {
        self.x[r as usize & 31]
    }

    #[inline(always)]
    pub fn set_x(&mut self, rd: u32, value: u64) {
        if rd != 0 {
            self.x[rd as usize & 31] = value;
            if self.tracing() {
                self.commit.regs.push(RegWrite::X(rd as u8, value));
            }
        }
    }

    #[inline]
    pub fn f(&self, r: u32) -> u64 {
        self.fp.f[r as usize & 31]
    }

    #[inline]
    pub fn set_f(&mut self, rd: u32, bits: u64) {
        self.fp.f[rd as usize & 31] = bits;
        if self.tracing() {
            self.commit.regs.push(RegWrite::F(rd as u8, bits));
        }
    }

    /// Record a CSR write in the commit log (no-op unless tracing).
    #[inline]
    pub fn log_csr_write(&mut self, addr: u16, value: u64) {
        if self.tracing() {
            self.commit.regs.push(RegWrite::Csr(addr, value));
        }
    }

    /// `mstatus.FS`: 0 Off, 1 Initial, 2 Clean, 3 Dirty.
    #[inline]
    pub fn mstatus_fs(&self) -> u8 {
        ((self.csr.mstatus >> csr::MSTATUS_FS_SHIFT) & 3) as u8
    }

    #[inline]
    pub fn set_mstatus_fs(&mut self, fs: u8) {
        let old = self.csr.mstatus;
        self.csr.mstatus = (old & !csr::MSTATUS_FS) | (((fs & 3) as u64) << csr::MSTATUS_FS_SHIFT);
        if self.csr.mstatus != old && self.tracing() {
            let v = self.csr.read_mstatus();
            self.commit.regs.push(RegWrite::Csr(csr::MSTATUS, v));
        }
    }

    // ---- memory access ----

    /// Translated, PMP-checked load of 1, 2, 4, or 8 bytes, zero-extended. Misaligned
    /// addresses are supported, including accesses that cross a page boundary.
    #[inline(always)]
    pub fn load(&mut self, bus: &mut Bus, vaddr: u64, size: u8) -> Result<u64, Exception> {
        let pgoff = vaddr & 0xfff;
        if pgoff + size as u64 <= 0x1000 {
            if let Some(off) = self.mmu.lookup(vaddr, Access::Load) {
                let v = bus.ram.read_at((off + pgoff) as usize, size);
                self.log_load(vaddr);
                return Ok(v);
            }
        }
        self.load_slow(bus, vaddr, size)
    }

    #[inline(never)]
    fn load_slow(&mut self, bus: &mut Bus, vaddr: u64, size: u8) -> Result<u64, Exception> {
        let v = if (vaddr & 0xfff) + size as u64 <= 0x1000 {
            let paddr = self.translate(bus, vaddr, size, Access::Load)?;
            bus.read(paddr, size).map_err(|_| Exception::new(ExceptionCause::LoadAccessFault, vaddr))?
        } else {
            self.load_split(bus, vaddr, size)?
        };
        self.log_load(vaddr);
        Ok(v)
    }

    /// Translated, PMP-checked store of the low `size` bytes of `value`. Misaligned
    /// addresses are supported, including accesses that cross a page boundary.
    #[inline(always)]
    pub fn store(&mut self, bus: &mut Bus, vaddr: u64, size: u8, value: u64) -> Result<(), Exception> {
        let pgoff = vaddr & 0xfff;
        if pgoff + size as u64 <= 0x1000 {
            if let Some(off) = self.mmu.lookup(vaddr, Access::Store) {
                bus.ram.write_at((off + pgoff) as usize, size, value);
                self.log_store(vaddr, value, size);
                return Ok(());
            }
        }
        self.store_slow(bus, vaddr, size, value)
    }

    #[inline(never)]
    fn store_slow(&mut self, bus: &mut Bus, vaddr: u64, size: u8, value: u64) -> Result<(), Exception> {
        if (vaddr & 0xfff) + size as u64 <= 0x1000 {
            let paddr = self.translate(bus, vaddr, size, Access::Store)?;
            bus.write(paddr, size, value).map_err(|_| Exception::new(ExceptionCause::StoreAccessFault, vaddr))?;
        } else {
            self.store_split(bus, vaddr, size, value)?;
        }
        self.log_store(vaddr, value, size);
        Ok(())
    }

    /// A load that crosses a page boundary: translate both parts before reading.
    #[cold]
    fn load_split(&mut self, bus: &mut Bus, vaddr: u64, size: u8) -> Result<u64, Exception> {
        let n1 = (0x1000 - (vaddr & 0xfff)) as u8;
        let v2 = vaddr.wrapping_add(n1 as u64);
        let p1 = self.translate(bus, vaddr, n1, Access::Load)?;
        let p2 = self.translate(bus, v2, size - n1, Access::Load)?;
        let mut v = 0u64;
        for k in 0..size {
            let (pa, va) = if k < n1 { (p1 + k as u64, vaddr) } else { (p2 + (k - n1) as u64, v2) };
            let byte = bus.read(pa, 1).map_err(|_| Exception::new(ExceptionCause::LoadAccessFault, va))?;
            v |= byte << (8 * k);
        }
        Ok(v)
    }

    #[cold]
    fn store_split(&mut self, bus: &mut Bus, vaddr: u64, size: u8, value: u64) -> Result<(), Exception> {
        let n1 = (0x1000 - (vaddr & 0xfff)) as u8;
        let v2 = vaddr.wrapping_add(n1 as u64);
        let p1 = self.translate(bus, vaddr, n1, Access::Store)?;
        let p2 = self.translate(bus, v2, size - n1, Access::Store)?;
        for k in 0..size {
            let (pa, va) = if k < n1 { (p1 + k as u64, vaddr) } else { (p2 + (k - n1) as u64, v2) };
            bus.write(pa, 1, value >> (8 * k)).map_err(|_| Exception::new(ExceptionCause::StoreAccessFault, va))?;
        }
        Ok(())
    }

    /// Record a load address (virtual, as Spike logs it) in the commit log.
    #[inline]
    pub fn log_load(&mut self, vaddr: u64) {
        if self.tracing() {
            self.commit.loads.push(vaddr);
        }
    }

    /// Record a store (virtual address, data, size) in the commit log.
    #[inline]
    pub fn log_store(&mut self, vaddr: u64, value: u64, size: u8) {
        if self.tracing() {
            let value = if size == 8 { value } else { value & ((1u64 << (8 * size)) - 1) };
            self.commit.stores.push(MemWrite { addr: vaddr, value, size });
        }
    }

    /// Drop any cached decoded instructions (FENCE.I).
    #[inline]
    pub fn flush_icache(&mut self) {}

    /// Virtual-to-physical translation for a data or fetch access (Sv39 + PMP).
    pub fn translate(&mut self, bus: &mut Bus, vaddr: u64, size: u8, access: Access) -> Result<u64, Exception> {
        mmu::translate(self, bus, vaddr, size, access)
    }

    // ---- execution ----

    /// Execute up to `max` steps (instructions or traps). Stops early when the hart is in
    /// WFI with nothing pending. Returns the number of instructions retired.
    ///
    /// In the normal profile, interrupts are polled on entry and then only where
    /// interrupt state can change: after Zicsr instructions, MRET, SRET, WFI, traps, and
    /// accesses that leave the TLB fast path (which includes every device access).
    /// Device lines change only during device accesses or between calls to `run`, so
    /// this is exact.
    pub fn run(&mut self, bus: &mut Bus, max: u64) -> u64 {
        mmu::update_ctx(self);
        self.wfi_retired = false;
        if !self.config.lockstep_irq && self.tracer.is_none() {
            return self.run_fast(bus, max);
        }
        if !self.config.lockstep_irq {
            self.irq_poll = true;
        }
        let mut retired = 0;
        for _ in 0..max {
            match self.step_impl(bus, false) {
                StepResult::Retired => retired += 1,
                StepResult::Trapped(_) => {}
                StepResult::Waiting => break,
            }
        }
        retired
    }

    /// The normal-profile run loop without tracing. Executes straight-line code in an
    /// inner loop that reuses the current fetch page's translation and batches the
    /// counter updates; it returns to the outer loop (which polls interrupts) after
    /// serializing instructions, SFENCE.VMA, traps, and device accesses.
    fn run_fast(&mut self, bus: &mut Bus, max: u64) -> u64 {
        self.irq_poll = true;
        let mut steps = 0u64;
        let mut retired = 0u64;
        while steps < max {
            if self.irq_poll {
                self.irq_poll = false;
                if self.poll_interrupts(bus).is_some() {
                    steps += 1;
                    continue;
                }
            }
            if self.waiting {
                break;
            }
            let mut cur_tag = u64::MAX;
            let mut cur_off = 0u64;
            let mut pending = 0u64;
            while steps < max {
                steps += 1;
                let pc = self.pc;
                let tag = (pc >> 12) | self.mmu.fetch_ctx;
                let cached = if tag == cur_tag {
                    let off = (cur_off + (pc & 0xfff)) as usize;
                    match bus.ram.cached_code(off) {
                        Some(slot) => Some((slot.insn, slot.raw)),
                        None => exec::fill_code(bus, pc, off),
                    }
                } else {
                    None
                };
                let (insn, raw) = match cached {
                    Some(x) => x,
                    None => match exec::fetch_decoded(self, bus) {
                        Ok(x) => {
                            if let Some(off) = self.mmu.lookup(pc, Access::Fetch) {
                                cur_tag = tag;
                                cur_off = off;
                            }
                            x
                        }
                        Err(e) => {
                            self.trap_exception(e, pc);
                            break;
                        }
                    },
                };
                if insn.op.is_serializing() {
                    self.csr.add_retired(pending);
                    retired += pending;
                    pending = 0;
                    if let StepResult::Retired = self.step_serializing(bus, insn, raw) {
                        retired += 1;
                    }
                    break;
                }
                if let Err(e) = exec::execute(self, bus, insn, raw) {
                    self.trap_exception(e, pc);
                    break;
                }
                pending += 1;
                // A device access or SFENCE.VMA: re-poll and re-translate.
                if self.irq_poll || insn.op == decode::Op::SfenceVma {
                    break;
                }
            }
            self.csr.add_retired(pending);
            retired += pending;
        }
        retired
    }

    /// Check for interrupts: wake from WFI on any locally enabled pending interrupt
    /// (even if globally masked), and take the highest-priority enabled one.
    #[inline]
    fn poll_interrupts(&mut self, bus: &Bus) -> Option<Trap> {
        if csr::mip(self, bus) & self.csr.mie == 0 {
            return None;
        }
        self.waiting = false;
        let irq = self.pending_interrupt(bus)?;
        let trap = Trap::Interrupt(irq);
        self.take_trap(trap, self.pc);
        Some(trap)
    }

    /// Take a pending interrupt or execute one instruction. In the normal profile this
    /// always polls interrupts first; in the lockstep profile it polls only when
    /// `irq_poll` is set.
    pub fn step(&mut self, bus: &mut Bus) -> StepResult {
        mmu::update_ctx(self);
        let always = !self.config.lockstep_irq;
        self.step_impl(bus, always)
    }

    #[inline(always)]
    fn step_impl(&mut self, bus: &mut Bus, always_poll: bool) -> StepResult {
        self.wfi_retired = false;
        if always_poll || self.irq_poll {
            self.irq_poll = false;
            if let Some(trap) = self.poll_interrupts(bus) {
                return StepResult::Trapped(trap);
            }
        }
        if self.waiting {
            return StepResult::Waiting;
        }
        let pc = self.pc;
        let (insn, raw) = match exec::fetch_decoded(self, bus) {
            Ok(x) => x,
            Err(e) => return self.trap_exception(e, pc),
        };
        if insn.op.is_serializing() {
            return self.step_serializing(bus, insn, raw);
        }
        if self.tracing() {
            self.commit.clear();
        }
        let mode = self.mode;
        if let Err(e) = exec::execute(self, bus, insn, raw) {
            return self.trap_exception(e, pc);
        }
        self.csr.retire();
        if self.tracing() {
            self.emit_commit(mode, pc, raw);
        }
        StepResult::Retired
    }

    /// Execute MRET, SRET, WFI, or a Zicsr instruction, then arrange to re-poll interrupts.
    #[inline(never)]
    fn step_serializing(&mut self, bus: &mut Bus, insn: decode::Insn, raw: u32) -> StepResult {
        let pc = self.pc;
        let lockstep = self.config.lockstep_irq;
        // Spike serializes before every Zicsr instruction, which polls interrupts.
        if lockstep && insn.op.is_csr() {
            if let Some(trap) = self.poll_interrupts(bus) {
                return StepResult::Trapped(trap);
            }
        }
        if self.tracing() {
            self.commit.clear();
        }
        let mode = self.mode;
        if let Err(e) = exec::execute_serializing(self, bus, insn, raw) {
            return self.trap_exception(e, pc);
        }
        self.csr.retire();
        if insn.op == decode::Op::Wfi {
            self.wfi_retired = true;
            // A WFI with an interrupt already pending must not stall.
            if !lockstep {
                self.irq_poll = true;
            }
        } else {
            self.irq_poll = true;
        }
        if self.tracing() {
            self.emit_commit(mode, pc, raw);
        }
        StepResult::Retired
    }

    #[cold]
    fn trap_exception(&mut self, e: Exception, pc: u64) -> StepResult {
        let trap = Trap::Exception(e);
        self.take_trap(trap, pc);
        if !self.config.lockstep_irq {
            self.irq_poll = true;
        }
        StepResult::Trapped(trap)
    }

    #[cold]
    fn emit_commit(&mut self, mode: Priv, pc: u64, raw: u32) {
        self.commit.mode = mode as u8;
        self.commit.pc = pc;
        self.commit.insn = raw;
        self.commit.len = if raw & 3 == 3 { 4 } else { 2 };
        let commit = core::mem::take(&mut self.commit);
        if let Some(t) = self.tracer.as_mut() {
            t.commit(&commit);
        }
        self.commit = commit;
    }

    /// The highest-priority interrupt that is pending, enabled, and not masked by the
    /// current mode, if any.
    pub fn pending_interrupt(&self, bus: &Bus) -> Option<Interrupt> {
        csr::pending_interrupt(self, bus)
    }

    /// Enter the trap handler for `trap`, with `epc` as the faulting/resume pc.
    pub fn take_trap(&mut self, trap: Trap, epc: u64) {
        let from = self.mode;
        csr::take_trap(self, trap, epc);
        if let Some(t) = self.tracer.as_mut() {
            t.trap(&trap, epc, from);
        }
    }
}
