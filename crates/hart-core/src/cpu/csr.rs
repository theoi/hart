//! The CSR file: machine and supervisor CSRs, counters, trap entry/return, and
//! interrupt selection. FP CSRs are delegated to `cpu::fp`; PMP CSRs to `cpu::pmp`.

use super::{fp, Cpu, Exception, Interrupt, Priv, Trap};
use crate::bus::Bus;
use crate::irq::{bits, HART_IRQ_MASK};
use std::borrow::Cow;

// ---- addresses ----
pub const FFLAGS: u16 = 0x001;
pub const FRM: u16 = 0x002;
pub const FCSR: u16 = 0x003;

pub const CYCLE: u16 = 0xc00;
pub const TIME: u16 = 0xc01;
pub const INSTRET: u16 = 0xc02;
pub const HPMCOUNTER3: u16 = 0xc03;
pub const HPMCOUNTER31: u16 = 0xc1f;

pub const SSTATUS: u16 = 0x100;
pub const SIE: u16 = 0x104;
pub const STVEC: u16 = 0x105;
pub const SCOUNTEREN: u16 = 0x106;
pub const SENVCFG: u16 = 0x10a;
pub const SSCRATCH: u16 = 0x140;
pub const SEPC: u16 = 0x141;
pub const SCAUSE: u16 = 0x142;
pub const STVAL: u16 = 0x143;
pub const SIP: u16 = 0x144;
pub const SATP: u16 = 0x180;

pub const MVENDORID: u16 = 0xf11;
pub const MARCHID: u16 = 0xf12;
pub const MIMPID: u16 = 0xf13;
pub const MHARTID: u16 = 0xf14;
pub const MCONFIGPTR: u16 = 0xf15;
pub const MSTATUS: u16 = 0x300;
pub const MISA: u16 = 0x301;
pub const MEDELEG: u16 = 0x302;
pub const MIDELEG: u16 = 0x303;
pub const MIE: u16 = 0x304;
pub const MTVEC: u16 = 0x305;
pub const MCOUNTEREN: u16 = 0x306;
pub const MENVCFG: u16 = 0x30a;
pub const MCOUNTINHIBIT: u16 = 0x320;
pub const MHPMEVENT3: u16 = 0x323;
pub const MHPMEVENT31: u16 = 0x33f;
pub const MSCRATCH: u16 = 0x340;
pub const MEPC: u16 = 0x341;
pub const MCAUSE: u16 = 0x342;
pub const MTVAL: u16 = 0x343;
pub const MIP: u16 = 0x344;
pub const PMPCFG0: u16 = 0x3a0;
pub const PMPCFG15: u16 = 0x3af;
pub const PMPADDR0: u16 = 0x3b0;
pub const PMPADDR63: u16 = 0x3ef;
pub const SCONTEXT: u16 = 0x5a8;
pub const HCONTEXT: u16 = 0x6a8;
pub const TSELECT: u16 = 0x7a0;
pub const TDATA1: u16 = 0x7a1;
pub const TDATA2: u16 = 0x7a2;
pub const TDATA3: u16 = 0x7a3;
pub const TINFO: u16 = 0x7a4;
pub const MCONTEXT: u16 = 0x7a8;
pub const MCYCLE: u16 = 0xb00;
pub const MINSTRET: u16 = 0xb02;
pub const MHPMCOUNTER3: u16 = 0xb03;
pub const MHPMCOUNTER31: u16 = 0xb1f;

// ---- mstatus fields ----
pub const MSTATUS_SIE: u64 = 1 << 1;
pub const MSTATUS_MIE: u64 = 1 << 3;
pub const MSTATUS_SPIE: u64 = 1 << 5;
pub const MSTATUS_MPIE: u64 = 1 << 7;
pub const MSTATUS_SPP: u64 = 1 << 8;
pub const MSTATUS_MPP_SHIFT: u64 = 11;
pub const MSTATUS_MPP: u64 = 3 << MSTATUS_MPP_SHIFT;
pub const MSTATUS_FS_SHIFT: u64 = 13;
pub const MSTATUS_FS: u64 = 3 << MSTATUS_FS_SHIFT;
pub const MSTATUS_XS: u64 = 3 << 15;
pub const MSTATUS_MPRV: u64 = 1 << 17;
pub const MSTATUS_SUM: u64 = 1 << 18;
pub const MSTATUS_MXR: u64 = 1 << 19;
pub const MSTATUS_TVM: u64 = 1 << 20;
pub const MSTATUS_TW: u64 = 1 << 21;
pub const MSTATUS_TSR: u64 = 1 << 22;
pub const MSTATUS_UXL: u64 = 3 << 32;
pub const MSTATUS_SXL: u64 = 3 << 34;
pub const MSTATUS_SD: u64 = 1 << 63;
/// UXL = SXL = 2 (64-bit), read-only.
const MSTATUS_XL64: u64 = (2 << 32) | (2 << 34);

const MSTATUS_WMASK: u64 = MSTATUS_SIE
    | MSTATUS_MIE
    | MSTATUS_SPIE
    | MSTATUS_MPIE
    | MSTATUS_SPP
    | MSTATUS_MPP
    | MSTATUS_FS
    | MSTATUS_MPRV
    | MSTATUS_SUM
    | MSTATUS_MXR
    | MSTATUS_TVM
    | MSTATUS_TW
    | MSTATUS_TSR;
const SSTATUS_RMASK: u64 = MSTATUS_SIE
    | MSTATUS_SPIE
    | MSTATUS_SPP
    | MSTATUS_FS
    | MSTATUS_XS
    | MSTATUS_SUM
    | MSTATUS_MXR
    | MSTATUS_UXL
    | MSTATUS_SD;
const SSTATUS_WMASK: u64 = MSTATUS_SIE | MSTATUS_SPIE | MSTATUS_SPP | MSTATUS_FS | MSTATUS_SUM | MSTATUS_MXR;

/// RV64 with A, C, D, F, I, M, S, U.
pub const MISA_VALUE: u64 =
    (2 << 62) | (1 << 0) | (1 << 2) | (1 << 3) | (1 << 5) | (1 << 8) | (1 << 12) | (1 << 18) | (1 << 20);

/// Delegable exceptions, matching Spike: codes 1-9, 12, 13, 15, and 19. Bit 0
/// (misaligned fetch) is read-only zero because C makes it impossible; ecall from M (11)
/// cannot be delegated.
const MEDELEG_WMASK: u64 = 0x8b3fe;
/// Debug-spec context registers (present without triggers, as in Spike).
const SCONTEXT_WMASK: u64 = 0xffff_ffff;
const MCONTEXT_WMASK: u64 = 0x1fff;
/// Delegable interrupts: the supervisor ones.
const MIDELEG_WMASK: u64 = (1 << 1) | (1 << 5) | (1 << 9);
const MIE_WMASK: u64 = 0xaaa;
/// mip bits that M-mode software may write: SSIP, STIP, SEIP.
const MIP_WMASK: u64 = (1 << 1) | (1 << 5) | (1 << 9);
pub const SATP_MODE_BARE: u64 = 0;
pub const SATP_MODE_SV39: u64 = 8;
const SATP_ASID_PPN_MASK: u64 = (0xffff << 44) | ((1 << 44) - 1);
/// envcfg.FIOM is the only implemented field.
const ENVCFG_WMASK: u64 = 1;

#[derive(Clone, Debug, Default)]
pub struct CsrFile {
    /// mstatus without SD and the read-only XL fields (see [`CsrFile::read_mstatus`]).
    pub mstatus: u64,
    pub medeleg: u64,
    pub mideleg: u64,
    pub mie: u64,
    /// Software-writable mip bits (SSIP, STIP, SEIP). Device lines come from `Wires`.
    pub mip: u64,
    pub mtvec: u64,
    pub mcounteren: u64,
    pub mcountinhibit: u64,
    pub menvcfg: u64,
    pub mscratch: u64,
    pub mepc: u64,
    pub mcause: u64,
    pub mtval: u64,
    pub mcycle: u64,
    pub minstret: u64,
    pub stvec: u64,
    pub scounteren: u64,
    pub senvcfg: u64,
    pub sscratch: u64,
    pub sepc: u64,
    pub scause: u64,
    pub stval: u64,
    pub satp: u64,
    pub scontext: u64,
    pub mcontext: u64,
}

impl CsrFile {
    pub fn new() -> Self {
        CsrFile::default()
    }

    #[inline]
    pub fn read_mstatus(&self) -> u64 {
        let sd = if self.mstatus & MSTATUS_FS == MSTATUS_FS { MSTATUS_SD } else { 0 };
        self.mstatus | MSTATUS_XL64 | sd
    }

    /// Count one retired instruction (`mcycle` advances with `minstret`).
    #[inline]
    pub fn retire(&mut self) {
        self.retire_with_inhibit(self.mcountinhibit);
    }

    /// Count one retired instruction under `inhibit`, the `mcountinhibit` value from
    /// before the instruction executed: an instruction that writes `mcountinhibit` is
    /// counted according to the old value, as in Spike and Sail.
    #[inline]
    pub fn retire_with_inhibit(&mut self, inhibit: u64) {
        if inhibit & 1 == 0 {
            self.mcycle = self.mcycle.wrapping_add(1);
        }
        if inhibit & 4 == 0 {
            self.minstret = self.minstret.wrapping_add(1);
        }
    }

    /// Count `n` retired instructions at once.
    #[inline]
    pub fn add_retired(&mut self, n: u64) {
        if self.mcountinhibit & 1 == 0 {
            self.mcycle = self.mcycle.wrapping_add(n);
        }
        if self.mcountinhibit & 4 == 0 {
            self.minstret = self.minstret.wrapping_add(n);
        }
    }

    #[inline]
    pub fn satp_mode(&self) -> u64 {
        self.satp >> 60
    }
}

/// `mip` as software sees it: software-writable bits OR device lines OR the test-only
/// interrupt lines.
#[inline]
pub fn mip(cpu: &Cpu, bus: &Bus) -> u64 {
    cpu.csr.mip | hw_mip(bus)
}

/// The `mip` bits driven by devices (including the test-only interrupt device).
#[inline]
fn hw_mip(bus: &Bus) -> u64 {
    (bus.wires.hart_irq & HART_IRQ_MASK) | (bus.wires.test_irq & (HART_IRQ_MASK | bits::SSIP))
}

fn counter_allowed(cpu: &Cpu, bit: u32) -> bool {
    match cpu.mode {
        Priv::Machine => true,
        Priv::Supervisor => cpu.csr.mcounteren >> bit & 1 != 0,
        Priv::User => cpu.csr.mcounteren >> bit & 1 != 0 && cpu.csr.scounteren >> bit & 1 != 0,
    }
}

/// Read a CSR as an instruction in the current mode would. `insn` is used for the
/// illegal-instruction `tval`.
pub fn read(cpu: &Cpu, bus: &Bus, addr: u16, insn: u32) -> Result<u64, Exception> {
    let illegal = Exception::illegal(insn);
    if (addr >> 8) & 3 > cpu.mode as u16 {
        return Err(illegal);
    }
    let c = &cpu.csr;
    Ok(match addr {
        FFLAGS | FRM | FCSR => fp::read_csr(cpu, addr).map_err(|_| illegal)?,
        CYCLE | TIME | INSTRET | HPMCOUNTER3..=HPMCOUNTER31 => {
            let bit = (addr - CYCLE) as u32;
            if !counter_allowed(cpu, bit) {
                return Err(illegal);
            }
            match addr {
                CYCLE => c.mcycle,
                TIME => bus.wires.mtime,
                INSTRET => c.minstret,
                _ => 0,
            }
        }
        SSTATUS => c.read_mstatus() & SSTATUS_RMASK,
        SIE => c.mie & c.mideleg,
        STVEC => c.stvec,
        SCOUNTEREN => c.scounteren,
        SENVCFG => c.senvcfg,
        SSCRATCH => c.sscratch,
        SEPC => c.sepc,
        SCAUSE => c.scause,
        STVAL => c.stval,
        SIP => mip(cpu, bus) & c.mideleg,
        SATP => {
            if cpu.mode == Priv::Supervisor && c.mstatus & MSTATUS_TVM != 0 {
                return Err(illegal);
            }
            c.satp
        }
        SCONTEXT => c.scontext,
        MVENDORID => cpu.config.mvendorid,
        MARCHID => cpu.config.marchid,
        MIMPID => cpu.config.mimpid,
        MHARTID | MCONFIGPTR => 0,
        MSTATUS => c.read_mstatus(),
        MISA => MISA_VALUE,
        MEDELEG => c.medeleg,
        MIDELEG => c.mideleg,
        MIE => c.mie,
        MTVEC => c.mtvec,
        MCOUNTEREN => c.mcounteren,
        MENVCFG => c.menvcfg,
        MCOUNTINHIBIT => c.mcountinhibit,
        MHPMEVENT3..=MHPMEVENT31 => 0,
        MSCRATCH => c.mscratch,
        MEPC => c.mepc,
        MCAUSE => c.mcause,
        MTVAL => c.mtval,
        MIP => mip(cpu, bus),
        PMPCFG0..=PMPCFG15 | PMPADDR0..=PMPADDR63 => cpu.pmp.read_csr(addr).ok_or(illegal)?,
        TSELECT | TDATA1 | TDATA2 | TDATA3 | TINFO => 0,
        MCONTEXT => c.mcontext,
        MCYCLE => c.mcycle,
        MINSTRET => c.minstret,
        MHPMCOUNTER3..=MHPMCOUNTER31 => 0,
        _ => return Err(illegal),
    })
}

/// Write a CSR as an instruction in the current mode would. Read-only CSRs and
/// insufficient privilege raise illegal instruction.
pub fn write(cpu: &mut Cpu, bus: &Bus, addr: u16, value: u64, insn: u32) -> Result<(), Exception> {
    let illegal = Exception::illegal(insn);
    if (addr >> 8) & 3 > cpu.mode as u16 || addr >> 10 == 3 {
        return Err(illegal);
    }
    let old_mip = cpu.csr.mip;
    let c = &mut cpu.csr;
    match addr {
        FFLAGS | FRM | FCSR => {
            // fp::write_csr logs fflags/frm itself, the way Spike does (no fcsr entry).
            fp::write_csr(cpu, addr, value).map_err(|_| illegal)?;
            return Ok(());
        }
        SSTATUS => {
            c.mstatus = (c.mstatus & !SSTATUS_WMASK) | (value & SSTATUS_WMASK);
            super::mmu::update_ctx(cpu);
        }
        SIE => c.mie = (c.mie & !c.mideleg) | (value & c.mideleg & MIE_WMASK),
        STVEC => c.stvec = value & !2,
        SCOUNTEREN => c.scounteren = value & 0xffff_ffff,
        SENVCFG => c.senvcfg = value & ENVCFG_WMASK,
        SSCRATCH => c.sscratch = value,
        SEPC => c.sepc = value & !1,
        SCAUSE => c.scause = value,
        STVAL => c.stval = value,
        SIP => {
            let m = c.mideleg & (1 << 1);
            c.mip = (c.mip & !m) | (value & m);
        }
        SATP => {
            if cpu.mode == Priv::Supervisor && c.mstatus & MSTATUS_TVM != 0 {
                return Err(illegal);
            }
            let mode = value >> 60;
            if mode == SATP_MODE_BARE || mode == SATP_MODE_SV39 {
                c.satp = (mode << 60) | (value & SATP_ASID_PPN_MASK);
                cpu.mmu.flush();
            }
        }
        MSTATUS => {
            let mut v = (c.mstatus & !MSTATUS_WMASK) | (value & MSTATUS_WMASK);
            if (v & MSTATUS_MPP) >> MSTATUS_MPP_SHIFT == 2 {
                v &= !MSTATUS_MPP;
            }
            cpu.csr.mstatus = v;
            super::mmu::update_ctx(cpu);
        }
        MISA => {}
        MEDELEG => c.medeleg = value & MEDELEG_WMASK,
        MIDELEG => c.mideleg = value & MIDELEG_WMASK,
        MIE => c.mie = value & MIE_WMASK,
        MTVEC => c.mtvec = value & !2,
        MCOUNTEREN => c.mcounteren = value & 0xffff_ffff,
        MENVCFG => c.menvcfg = value & ENVCFG_WMASK,
        MCOUNTINHIBIT => c.mcountinhibit = value & 0xffff_fffd,
        MHPMEVENT3..=MHPMEVENT31 => {}
        MSCRATCH => c.mscratch = value,
        MEPC => c.mepc = value & !1,
        MCAUSE => c.mcause = value,
        MTVAL => c.mtval = value,
        MIP => c.mip = (c.mip & !MIP_WMASK) | (value & MIP_WMASK),
        PMPCFG0..=PMPCFG15 | PMPADDR0..=PMPADDR63 => {
            if !cpu.pmp.write_csr(addr, value) {
                return Err(illegal);
            }
            cpu.mmu.flush();
        }
        SCONTEXT => c.scontext = value & SCONTEXT_WMASK,
        MCONTEXT => c.mcontext = value & MCONTEXT_WMASK,
        TSELECT => {}
        // Constant registers: writes are ignored and, as in Spike, not logged.
        TDATA1 | TDATA2 | TDATA3 | TINFO | MHPMCOUNTER3..=MHPMCOUNTER31 => return Ok(()),
        // A write to a counter takes precedence over the increment of the same instruction.
        MCYCLE => c.mcycle = if c.mcountinhibit & 1 == 0 { value.wrapping_sub(1) } else { value },
        MINSTRET => c.minstret = if c.mcountinhibit & 4 == 0 { value.wrapping_sub(1) } else { value },
        _ => return Err(illegal),
    }
    if cpu.tracing() {
        log_write(cpu, bus, addr, old_mip);
    }
    Ok(())
}

const MVIP: u16 = 0x309;

/// Record the commit-log entries Spike produces for a write to `addr`. `old_mip` is
/// the software-writable `mip` state from before the write.
fn log_write(cpu: &mut Cpu, bus: &Bus, addr: u16, old_mip: u64) {
    let c = &cpu.csr;
    match addr {
        SSTATUS | MSTATUS => {
            let v = c.read_mstatus();
            cpu.log_csr_write(MSTATUS, v);
        }
        SIE => {
            let (sie, mie) = (c.mie & c.mideleg, c.mie);
            cpu.log_csr_write(SIE, sie);
            cpu.log_csr_write(MIE, mie);
        }
        SIP | MIP => {
            // Spike keeps the software SEIP bit in mvip and logs mvip before it updates
            // mip, so the logged mvip has the new SEIP but the old SSIP and STIP. Its
            // logged mip is the full read value, software SEIP included.
            let mvip = (c.mip & bits::SEIP) | (old_mip & (bits::SSIP | bits::STIP));
            let mip = mip(cpu, bus);
            cpu.log_csr_write(MVIP, mvip);
            cpu.log_csr_write(MIP, mip);
        }
        _ => {
            let (a, v) = log_value(cpu, addr);
            cpu.log_csr_write(a, v);
        }
    }
}

/// The value Spike logs after a write to `addr` (single-entry cases).
fn log_value(cpu: &Cpu, addr: u16) -> (u16, u64) {
    let c = &cpu.csr;
    match addr {
        MCYCLE => (MCYCLE, if c.mcountinhibit & 1 == 0 { c.mcycle.wrapping_add(1) } else { c.mcycle }),
        // Spike's mcontext is a proxy that logs under the hcontext name.
        MCONTEXT => (HCONTEXT, c.mcontext),
        SCONTEXT => (SCONTEXT, c.scontext),
        MINSTRET => (MINSTRET, if c.mcountinhibit & 4 == 0 { c.minstret.wrapping_add(1) } else { c.minstret }),
        _ => {
            let v = match addr {
                STVEC => c.stvec,
                SCOUNTEREN => c.scounteren,
                SENVCFG => c.senvcfg,
                SSCRATCH => c.sscratch,
                SEPC => c.sepc,
                SCAUSE => c.scause,
                STVAL => c.stval,
                SATP => c.satp,
                MEDELEG => c.medeleg,
                MIDELEG => c.mideleg,
                MIE => c.mie,
                MTVEC => c.mtvec,
                MCOUNTEREN => c.mcounteren,
                MENVCFG => c.menvcfg,
                MCOUNTINHIBIT => c.mcountinhibit,
                MSCRATCH => c.mscratch,
                MEPC => c.mepc,
                MCAUSE => c.mcause,
                MTVAL => c.mtval,
                MIP => c.mip,
                PMPCFG0..=PMPCFG15 | PMPADDR0..=PMPADDR63 => cpu.pmp.read_csr(addr).unwrap_or(0),
                MISA => MISA_VALUE,
                _ => 0,
            };
            (addr, v)
        }
    }
}

/// Select the interrupt to take now, if any. Priority: M-level before S-level, then
/// MEI, MSI, MTI, SEI, SSI, STI.
pub fn pending_interrupt(cpu: &Cpu, bus: &Bus) -> Option<Interrupt> {
    let pending = mip(cpu, bus) & cpu.csr.mie;
    if pending == 0 {
        return None;
    }
    let mstatus = cpu.csr.mstatus;
    let m_enabled = cpu.mode < Priv::Machine || mstatus & MSTATUS_MIE != 0;
    let mut enabled = if m_enabled { pending & !cpu.csr.mideleg } else { 0 };
    if enabled == 0 {
        let s_enabled = cpu.mode < Priv::Supervisor || (cpu.mode == Priv::Supervisor && mstatus & MSTATUS_SIE != 0);
        if s_enabled {
            enabled = pending & cpu.csr.mideleg;
        }
    }
    if enabled == 0 {
        return None;
    }
    use Interrupt::*;
    [MachineExternal, MachineSoftware, MachineTimer, SupervisorExternal, SupervisorSoftware, SupervisorTimer]
        .into_iter()
        .find(|i| enabled & i.mask() != 0)
}

/// Trap entry: pick the target mode from the delegation registers and update state.
pub fn take_trap(cpu: &mut Cpu, trap: Trap, epc: u64) {
    let (deleg, code, is_irq) = match trap {
        Trap::Exception(e) => (cpu.csr.medeleg, e.code(), false),
        Trap::Interrupt(i) => (cpu.csr.mideleg, i.code(), true),
    };
    let from = cpu.mode;
    let c = &mut cpu.csr;
    if from <= Priv::Supervisor && (deleg >> code) & 1 != 0 {
        c.sepc = epc;
        c.scause = trap.cause();
        c.stval = trap.tval();
        let sie = c.mstatus & MSTATUS_SIE != 0;
        c.mstatus &= !(MSTATUS_SPIE | MSTATUS_SIE | MSTATUS_SPP);
        if sie {
            c.mstatus |= MSTATUS_SPIE;
        }
        if from == Priv::Supervisor {
            c.mstatus |= MSTATUS_SPP;
        }
        let base = c.stvec & !3;
        cpu.pc = if is_irq && c.stvec & 1 != 0 { base + 4 * code } else { base };
        cpu.mode = Priv::Supervisor;
    } else {
        c.mepc = epc;
        c.mcause = trap.cause();
        c.mtval = trap.tval();
        let mie = c.mstatus & MSTATUS_MIE != 0;
        c.mstatus &= !(MSTATUS_MPIE | MSTATUS_MIE | MSTATUS_MPP);
        if mie {
            c.mstatus |= MSTATUS_MPIE;
        }
        c.mstatus |= (from as u64) << MSTATUS_MPP_SHIFT;
        let base = c.mtvec & !3;
        cpu.pc = if is_irq && c.mtvec & 1 != 0 { base + 4 * code } else { base };
        cpu.mode = Priv::Machine;
    }
    cpu.waiting = false;
    super::mmu::update_ctx(cpu);
}

/// MRET. Caller has checked privilege.
pub fn mret(cpu: &mut Cpu) {
    let c = &mut cpu.csr;
    let mpp = Priv::from_bits(c.mstatus >> MSTATUS_MPP_SHIFT);
    let mpie = c.mstatus & MSTATUS_MPIE != 0;
    c.mstatus &= !(MSTATUS_MIE | MSTATUS_MPP);
    c.mstatus |= MSTATUS_MPIE;
    if mpie {
        c.mstatus |= MSTATUS_MIE;
    }
    if mpp != Priv::Machine {
        c.mstatus &= !MSTATUS_MPRV;
    }
    cpu.pc = c.mepc;
    cpu.mode = mpp;
    super::mmu::update_ctx(cpu);
    let v = cpu.csr.read_mstatus();
    cpu.log_csr_write(MSTATUS, v);
}

/// SRET. Caller has checked privilege and TSR.
pub fn sret(cpu: &mut Cpu) {
    let c = &mut cpu.csr;
    let spp = if c.mstatus & MSTATUS_SPP != 0 { Priv::Supervisor } else { Priv::User };
    let spie = c.mstatus & MSTATUS_SPIE != 0;
    c.mstatus &= !(MSTATUS_SIE | MSTATUS_SPP | MSTATUS_MPRV);
    c.mstatus |= MSTATUS_SPIE;
    if spie {
        c.mstatus |= MSTATUS_SIE;
    }
    cpu.pc = c.sepc;
    cpu.mode = spp;
    super::mmu::update_ctx(cpu);
    let v = cpu.csr.read_mstatus();
    cpu.log_csr_write(MSTATUS, v);
}

/// Name used in Spike commit logs (`c768_mstatus`).
pub fn name(addr: u16) -> Cow<'static, str> {
    let fixed = match addr {
        FFLAGS => "fflags",
        FRM => "frm",
        FCSR => "fcsr",
        CYCLE => "cycle",
        TIME => "time",
        INSTRET => "instret",
        SSTATUS => "sstatus",
        SIE => "sie",
        STVEC => "stvec",
        SCOUNTEREN => "scounteren",
        SENVCFG => "senvcfg",
        SSCRATCH => "sscratch",
        SEPC => "sepc",
        SCAUSE => "scause",
        STVAL => "stval",
        SIP => "sip",
        SATP => "satp",
        SCONTEXT => "scontext",
        HCONTEXT => "hcontext",
        MVENDORID => "mvendorid",
        MARCHID => "marchid",
        MIMPID => "mimpid",
        MHARTID => "mhartid",
        MCONFIGPTR => "mconfigptr",
        MSTATUS => "mstatus",
        MISA => "misa",
        MEDELEG => "medeleg",
        MIDELEG => "mideleg",
        MIE => "mie",
        MTVEC => "mtvec",
        MCOUNTEREN => "mcounteren",
        MENVCFG => "menvcfg",
        MVIP => "mvip",
        MCOUNTINHIBIT => "mcountinhibit",
        MSCRATCH => "mscratch",
        MEPC => "mepc",
        MCAUSE => "mcause",
        MTVAL => "mtval",
        MIP => "mip",
        TSELECT => "tselect",
        TDATA1 => "tdata1",
        TDATA2 => "tdata2",
        TDATA3 => "tdata3",
        TINFO => "tinfo",
        MCONTEXT => "mcontext",
        MCYCLE => "mcycle",
        MINSTRET => "minstret",
        HPMCOUNTER3..=HPMCOUNTER31 => return Cow::Owned(format!("hpmcounter{}", addr - CYCLE)),
        MHPMEVENT3..=MHPMEVENT31 => return Cow::Owned(format!("mhpmevent{}", addr - MCOUNTINHIBIT)),
        MHPMCOUNTER3..=MHPMCOUNTER31 => return Cow::Owned(format!("mhpmcounter{}", addr - MCYCLE)),
        PMPCFG0..=PMPCFG15 => return Cow::Owned(format!("pmpcfg{}", addr - PMPCFG0)),
        PMPADDR0..=PMPADDR63 => return Cow::Owned(format!("pmpaddr{}", addr - PMPADDR0)),
        _ => "unknown",
    };
    Cow::Borrowed(fixed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::CpuConfig;
    use crate::trace::{Commit, RegWrite, TraceSink};

    struct Discard;
    impl TraceSink for Discard {
        fn commit(&mut self, _: &Commit) {}
    }

    fn take_csr_log(cpu: &mut Cpu) -> Vec<(u16, u64)> {
        let log = cpu
            .commit
            .regs
            .iter()
            .filter_map(|r| match *r {
                RegWrite::Csr(a, v) => Some((a, v)),
                _ => None,
            })
            .collect();
        cpu.commit.clear();
        log
    }

    #[test]
    fn spike_profile_csr_details() {
        let mut cpu = Cpu::with_config(0x8000_0000, CpuConfig::spike());
        cpu.set_tracer(Some(Box::new(Discard)));
        let bus = Bus::new(0x8000_0000, 0x1000);
        assert_eq!(read(&cpu, &bus, PMPCFG0, 0), Ok(0x1f));
        assert_eq!(read(&cpu, &bus, PMPADDR0, 0), Ok(0x003f_ffff_ffff_ffff));
        // OpenSBI writes 0xb109; misaligned fetch (bit 0) is not delegable with C.
        write(&mut cpu, &bus, MEDELEG, 0xb109, 0).unwrap();
        assert_eq!(take_csr_log(&mut cpu), [(MEDELEG, 0xb108)]);
        assert_eq!(read(&cpu, &bus, TINFO, 0), Ok(0));
        write(&mut cpu, &bus, MHPMCOUNTER3, 5, 0).unwrap();
        write(&mut cpu, &bus, TDATA1, 5, 0).unwrap();
        write(&mut cpu, &bus, TINFO, 5, 0).unwrap();
        assert_eq!(take_csr_log(&mut cpu), []);
        assert_eq!(read(&cpu, &bus, MHPMCOUNTER3, 0), Ok(0));
        write(&mut cpu, &bus, MCONTEXT, !0, 0).unwrap();
        write(&mut cpu, &bus, SCONTEXT, !0, 0).unwrap();
        assert_eq!(take_csr_log(&mut cpu), [(HCONTEXT, 0x1fff), (SCONTEXT, 0xffff_ffff)]);
    }

    #[test]
    fn mip_write_logs_like_spike() {
        let mut cpu = Cpu::with_config(0x8000_0000, CpuConfig::spike());
        cpu.set_tracer(Some(Box::new(Discard)));
        let bus = Bus::new(0x8000_0000, 0x1000);
        write(&mut cpu, &bus, MIP, bits::SEIP | bits::SSIP, 0).unwrap();
        // mvip is logged before mip is updated: new SEIP, old SSIP.
        assert_eq!(take_csr_log(&mut cpu), [(MVIP, bits::SEIP), (MIP, bits::SEIP | bits::SSIP)]);
        assert_eq!(read(&cpu, &bus, MIP, 0), Ok(bits::SEIP | bits::SSIP));
        write(&mut cpu, &bus, MIP, 0, 0).unwrap();
        assert_eq!(take_csr_log(&mut cpu), [(MVIP, bits::SSIP), (MIP, 0)]);
    }

    #[test]
    fn reserved_tvec_modes_clear_bit_1() {
        let mut cpu = Cpu::new(0x8000_0000);
        let bus = Bus::new(0x8000_0000, 0x1000);
        write(&mut cpu, &bus, MTVEC, 0x8000_0001, 0).unwrap();
        write(&mut cpu, &bus, MTVEC, 0x8000_1002, 0).unwrap();
        assert_eq!(read(&cpu, &bus, MTVEC, 0), Ok(0x8000_1000));
        write(&mut cpu, &bus, STVEC, 0x207, 0).unwrap();
        assert_eq!(read(&cpu, &bus, STVEC, 0), Ok(0x205));
    }

    #[test]
    fn default_profile_pmp_resets_off() {
        let cpu = Cpu::new(0x8000_0000);
        let bus = Bus::new(0x8000_0000, 0x1000);
        assert_eq!(read(&cpu, &bus, PMPCFG0, 0), Ok(0));
        assert_eq!(read(&cpu, &bus, PMPADDR0, 0), Ok(0));
    }

    #[test]
    fn numbered_names_match_spike() {
        assert_eq!(name(0x3c0), "pmpaddr16");
        assert_eq!(name(0x3ef), "pmpaddr63");
        assert_eq!(name(0x3ae), "pmpcfg14");
        assert_eq!(name(0xb03), "mhpmcounter3");
        assert_eq!(name(0xb1f), "mhpmcounter31");
        assert_eq!(name(0x33f), "mhpmevent31");
        assert_eq!(name(0xc03), "hpmcounter3");
        assert_eq!(name(0x7a4), "tinfo");
        assert_eq!(name(0x6a8), "hcontext");
    }
}
