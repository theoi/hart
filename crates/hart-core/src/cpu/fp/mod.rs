//! Floating-point seam (F and D). Owned by the FPU builder.
//!
//! The decoder sends every FP instruction here in 32-bit form: LOAD-FP (0x07),
//! STORE-FP (0x27), OP-FP (0x53), and MADD/MSUB/NMSUB/NMADD (0x43/0x47/0x4b/0x4f).
//! Compressed FP loads and stores (C.FLD, C.FSD, C.FLDSP, C.FSDSP) are expanded to the
//! equivalent 32-bit FLD/FSD first, so this module never sees 16-bit encodings.
//!
//! Hooks the CPU provides for this module:
//! - `cpu.mstatus_fs()` / `cpu.set_mstatus_fs(fs)`: the FS field (0 Off, 1 Initial,
//!   2 Clean, 3 Dirty). `mstatus.SD` is derived from it automatically.
//! - `cpu.set_f(rd, bits)`: write an f register (logged by the tracer).
//! - `cpu.load(bus, vaddr, size)` / `cpu.store(bus, vaddr, size, value)`: translated,
//!   PMP-checked memory access with misaligned support and tracing.
//! - `cpu.x(r)` / `cpu.set_x(rd, v)` for integer operands and results.
//! - `cpu.log_csr_write(addr, value)` when an instruction updates fflags or mstatus.

use crate::bus::Bus;
use crate::cpu::{Cpu, Exception};

pub const FS_OFF: u8 = 0;
pub const FS_INITIAL: u8 = 1;
pub const FS_CLEAN: u8 = 2;
pub const FS_DIRTY: u8 = 3;

#[derive(Clone, Debug, Default)]
pub struct FpState {
    /// f0..f31 as raw bits. Single-precision values are NaN-boxed.
    pub f: [u64; 32],
    /// Dynamic rounding mode (`fcsr.frm`, 3 bits).
    pub frm: u8,
    /// Accrued exception flags (`fcsr.fflags`, 5 bits).
    pub fflags: u8,
}

/// Execute one FP instruction (32-bit encoding). On `Ok` the CPU advances `pc`; this
/// function must not change `pc`. It must raise an illegal-instruction exception when
/// `mstatus.FS` is Off or the encoding is invalid.
pub fn execute(cpu: &mut Cpu, bus: &mut Bus, insn: u32) -> Result<(), Exception> {
    let _ = (cpu, bus);
    Err(Exception::illegal(insn))
}

/// Read `fflags` (0x001), `frm` (0x002), or `fcsr` (0x003). `Err` means illegal instruction.
pub fn read_csr(cpu: &Cpu, addr: u16) -> Result<u64, ()> {
    if cpu.mstatus_fs() == FS_OFF {
        return Err(());
    }
    let fp = &cpu.fp;
    Ok(match addr {
        0x001 => fp.fflags as u64,
        0x002 => fp.frm as u64,
        0x003 => ((fp.frm as u64) << 5) | fp.fflags as u64,
        _ => return Err(()),
    })
}

/// Write `fflags`, `frm`, or `fcsr`. `Err` means illegal instruction.
pub fn write_csr(cpu: &mut Cpu, addr: u16, value: u64) -> Result<(), ()> {
    if cpu.mstatus_fs() == FS_OFF {
        return Err(());
    }
    match addr {
        0x001 => cpu.fp.fflags = (value & 0x1f) as u8,
        0x002 => cpu.fp.frm = (value & 0x7) as u8,
        0x003 => {
            cpu.fp.fflags = (value & 0x1f) as u8;
            cpu.fp.frm = ((value >> 5) & 0x7) as u8;
        }
        _ => return Err(()),
    }
    cpu.set_mstatus_fs(FS_DIRTY);
    Ok(())
}
