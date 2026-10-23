//! The CSR file: machine and supervisor CSRs, counters, trap entry/return, and
//! interrupt selection. FP CSRs are delegated to `cpu::fp`; PMP CSRs to `cpu::pmp`.

use super::{fp, Cpu, Exception, Interrupt, Priv, Trap};
use crate::bus::Bus;
use crate::irq::HART_IRQ_MASK;

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
pub const TSELECT: u16 = 0x7a0;
pub const TDATA1: u16 = 0x7a1;
pub const TDATA2: u16 = 0x7a2;
pub const TDATA3: u16 = 0x7a3;
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

/// Delegable exceptions: everything except ecall-from-M (11) and the reserved codes.
const MEDELEG_WMASK: u64 = 0xb3ff;
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
        if self.mcountinhibit & 1 == 0 {
            self.mcycle = self.mcycle.wrapping_add(1);
        }
        if self.mcountinhibit & 4 == 0 {
            self.minstret = self.minstret.wrapping_add(1);
        }
    }

    #[inline]
    pub fn satp_mode(&self) -> u64 {
        self.satp >> 60
    }
}

/// `mip` as software sees it: software-writable bits OR device lines.
#[inline]
pub fn mip(cpu: &Cpu, bus: &Bus) -> u64 {
    cpu.csr.mip | (bus.wires.hart_irq & HART_IRQ_MASK)
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
        MVENDORID | MARCHID | MIMPID | MHARTID | MCONFIGPTR => 0,
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
        TSELECT | TDATA1 | TDATA2 | TDATA3 => 0,
        MCYCLE => c.mcycle,
        MINSTRET => c.minstret,
        MHPMCOUNTER3..=MHPMCOUNTER31 => 0,
        _ => return Err(illegal),
    })
}

/// Write a CSR as an instruction in the current mode would. Read-only CSRs and
/// insufficient privilege raise illegal instruction.
pub fn write(cpu: &mut Cpu, addr: u16, value: u64, insn: u32) -> Result<(), Exception> {
    let illegal = Exception::illegal(insn);
    if (addr >> 8) & 3 > cpu.mode as u16 || addr >> 10 == 3 {
        return Err(illegal);
    }
    let c = &mut cpu.csr;
    match addr {
        FFLAGS | FRM | FCSR => {
            // fp::write_csr logs fflags/frm itself, the way Spike does (no fcsr entry).
            fp::write_csr(cpu, addr, value).map_err(|_| illegal)?;
            return Ok(());
        }
        SSTATUS => c.mstatus = (c.mstatus & !SSTATUS_WMASK) | (value & SSTATUS_WMASK),
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
            if (v ^ c.mstatus) & (MSTATUS_MPRV | MSTATUS_SUM | MSTATUS_MXR | MSTATUS_MPP) != 0 {
                cpu.mmu.flush();
            }
            cpu.csr.mstatus = v;
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
        TSELECT | TDATA1 | TDATA2 | TDATA3 => {}
        // A write to a counter takes precedence over the increment of the same instruction.
        MCYCLE => c.mcycle = if c.mcountinhibit & 1 == 0 { value.wrapping_sub(1) } else { value },
        MINSTRET => c.minstret = if c.mcountinhibit & 4 == 0 { value.wrapping_sub(1) } else { value },
        MHPMCOUNTER3..=MHPMCOUNTER31 => {}
        _ => return Err(illegal),
    }
    if cpu.tracing() {
        let logged = log_value(cpu, addr);
        cpu.log_csr_write(logged.0, logged.1);
    }
    Ok(())
}

/// The CSR and value to record in the commit log after a write to `addr`. Supervisor
/// views of machine registers are logged as the machine register, like Spike.
fn log_value(cpu: &Cpu, addr: u16) -> (u16, u64) {
    let c = &cpu.csr;
    match addr {
        SSTATUS | MSTATUS => (MSTATUS, c.read_mstatus()),
        SIE => (MIE, c.mie),
        SIP => (MIP, c.mip),
        MCYCLE => (MCYCLE, c.mcycle.wrapping_add(1)),
        MINSTRET => (MINSTRET, c.minstret.wrapping_add(1)),
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
    cpu.reservation = None;
    cpu.mmu.flush();
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
    cpu.reservation = None;
    cpu.mmu.flush();
}

/// Name used in Spike commit logs (`c768_mstatus`).
pub fn name(addr: u16) -> &'static str {
    match addr {
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
        MCYCLE => "mcycle",
        MINSTRET => "minstret",
        0x3a0 => "pmpcfg0",
        0x3a2 => "pmpcfg2",
        0x3b0 => "pmpaddr0",
        0x3b1 => "pmpaddr1",
        0x3b2 => "pmpaddr2",
        0x3b3 => "pmpaddr3",
        0x3b4 => "pmpaddr4",
        0x3b5 => "pmpaddr5",
        0x3b6 => "pmpaddr6",
        0x3b7 => "pmpaddr7",
        0x3b8 => "pmpaddr8",
        0x3b9 => "pmpaddr9",
        0x3ba => "pmpaddr10",
        0x3bb => "pmpaddr11",
        0x3bc => "pmpaddr12",
        0x3bd => "pmpaddr13",
        0x3be => "pmpaddr14",
        0x3bf => "pmpaddr15",
        _ => "unknown",
    }
}
