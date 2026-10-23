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
            tracer: None,
            commit: Commit::default(),
        }
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

    #[inline]
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
    /// addresses are supported.
    pub fn load(&mut self, bus: &mut Bus, vaddr: u64, size: u8) -> Result<u64, Exception> {
        let paddr = self.translate(bus, vaddr, size, Access::Load)?;
        let v = bus.read(paddr, size).map_err(|_| Exception::new(ExceptionCause::LoadAccessFault, vaddr))?;
        if self.tracing() {
            self.commit.loads.push(paddr);
        }
        Ok(v)
    }

    /// Translated, PMP-checked store of the low `size` bytes of `value`. Misaligned
    /// addresses are supported.
    pub fn store(&mut self, bus: &mut Bus, vaddr: u64, size: u8, value: u64) -> Result<(), Exception> {
        let paddr = self.translate(bus, vaddr, size, Access::Store)?;
        bus.write(paddr, size, value).map_err(|_| Exception::new(ExceptionCause::StoreAccessFault, vaddr))?;
        if self.tracing() {
            self.commit.stores.push(MemWrite { addr: paddr, value, size });
        }
        Ok(())
    }

    /// Virtual-to-physical translation for a data or fetch access (Sv39 + PMP).
    pub fn translate(&mut self, bus: &mut Bus, vaddr: u64, size: u8, access: Access) -> Result<u64, Exception> {
        mmu::translate(self, bus, vaddr, size, access)
    }

    // ---- execution ----

    /// Execute up to `max` instructions. Stops early when the hart enters WFI with nothing
    /// pending. Returns the number of instructions retired.
    pub fn run(&mut self, bus: &mut Bus, max: u64) -> u64 {
        let mut retired = 0;
        let mut steps = 0;
        while steps < max {
            steps += 1;
            match self.step(bus) {
                StepResult::Retired => retired += 1,
                StepResult::Trapped(_) => {}
                StepResult::Waiting => break,
            }
        }
        retired
    }

    /// Take a pending interrupt or execute one instruction.
    pub fn step(&mut self, bus: &mut Bus) -> StepResult {
        if let Some(irq) = self.pending_interrupt(bus) {
            self.waiting = false;
            let trap = Trap::Interrupt(irq);
            self.take_trap(trap, self.pc);
            return StepResult::Trapped(trap);
        }
        if self.waiting {
            return StepResult::Waiting;
        }
        let pc = self.pc;
        let mode = self.mode;
        if self.tracing() {
            self.commit.clear();
        }
        match exec::step(self, bus) {
            Ok(insn) => {
                self.csr.retire();
                if self.tracing() {
                    self.commit.mode = mode as u8;
                    self.commit.pc = pc;
                    self.commit.insn = insn;
                    self.commit.len = if insn & 3 == 3 { 4 } else { 2 };
                    let commit = core::mem::take(&mut self.commit);
                    if let Some(t) = self.tracer.as_mut() {
                        t.commit(&commit);
                    }
                    self.commit = commit;
                }
                StepResult::Retired
            }
            Err(e) => {
                let trap = Trap::Exception(e);
                self.take_trap(trap, pc);
                StepResult::Trapped(trap)
            }
        }
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
