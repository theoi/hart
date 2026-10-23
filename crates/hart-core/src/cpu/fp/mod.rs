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
//!
//! State-update rules (matching Spike):
//! - Any FP instruction, including loads, stores, and moves, is illegal while FS is Off.
//! - Writing an f register, or accruing a nonzero set of exception flags, sets FS to
//!   Dirty. Instructions that only write an integer register and raise no flags (for
//!   example `fmv.x.d`, `fclass`, or an exact `fcvt.w.d`) and FP stores leave FS alone.
//! - Accrued flags are ORed into `fflags`, which is logged (with its new accrued value)
//!   whenever an instruction raises any flag, even if the value does not change.
//! - `set_mstatus_fs` logs `mstatus` when FS actually changes, so the first f-register or
//!   fflags write while FS is not Dirty logs `mstatus` with FS = 3 and SD = 1.
//! - Every instruction with an `rm` field (arithmetic, FMA, square root, and all `fcvt`
//!   forms, including the exact ones) is illegal when `rm` is 5 or 6, or when `rm` is 7
//!   (DYN) and `frm` holds 5, 6, or 7.

use crate::bus::Bus;
use crate::cpu::{csr, Cpu, Exception};
use crate::softfloat::{self as sf, Flags, RoundingMode};

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

const OP_LOAD_FP: u32 = 0x07;
const OP_STORE_FP: u32 = 0x27;
const OP_MADD: u32 = 0x43;
const OP_MSUB: u32 = 0x47;
const OP_NMSUB: u32 = 0x4b;
const OP_NMADD: u32 = 0x4f;
const OP_FP: u32 = 0x53;

const FMT_S: u32 = 0;
const FMT_D: u32 = 1;

#[inline(always)]
fn rd(insn: u32) -> u32 {
    (insn >> 7) & 31
}

#[inline(always)]
fn rs1(insn: u32) -> u32 {
    (insn >> 15) & 31
}

#[inline(always)]
fn rs2(insn: u32) -> u32 {
    (insn >> 20) & 31
}

#[inline(always)]
fn rs3(insn: u32) -> u32 {
    insn >> 27
}

#[inline(always)]
fn funct3(insn: u32) -> u32 {
    (insn >> 12) & 7
}

#[inline(always)]
fn sext32(v: u32) -> u64 {
    v as i32 as i64 as u64
}

/// Resolve the instruction's `rm` field, consulting `frm` for DYN.
#[inline]
fn rounding_mode(cpu: &Cpu, insn: u32) -> Result<RoundingMode, Exception> {
    let rm = match funct3(insn) {
        7 => cpu.fp.frm as u32,
        rm => rm,
    };
    RoundingMode::from_bits(rm).ok_or(Exception::illegal(insn))
}

/// Write an f register and mark the FP state dirty.
#[inline]
fn write_f(cpu: &mut Cpu, rd: u32, bits: u64) {
    cpu.set_f(rd, bits);
    cpu.set_mstatus_fs(FS_DIRTY);
}

#[inline]
fn write_f32(cpu: &mut Cpu, rd: u32, bits: u32) {
    write_f(cpu, rd, sf::nan_box_f32(bits));
}

/// Accrue exception flags raised by one instruction.
#[inline]
fn accrue(cpu: &mut Cpu, fl: Flags) {
    if !fl.is_empty() {
        cpu.fp.fflags |= fl.bits();
        cpu.set_mstatus_fs(FS_DIRTY);
        let v = cpu.fp.fflags as u64;
        cpu.log_csr_write(csr::FFLAGS, v);
    }
}

#[inline]
fn f32_operand(cpu: &Cpu, r: u32) -> u32 {
    sf::unbox_f32(cpu.f(r))
}

/// Execute one FP instruction (32-bit encoding). On `Ok` the CPU advances `pc`; this
/// function must not change `pc`. It raises an illegal-instruction exception when
/// `mstatus.FS` is Off or the encoding is invalid.
pub fn execute(cpu: &mut Cpu, bus: &mut Bus, insn: u32) -> Result<(), Exception> {
    let illegal = Exception::illegal(insn);
    if cpu.mstatus_fs() == FS_OFF {
        return Err(illegal);
    }
    match insn & 0x7f {
        OP_LOAD_FP => {
            let addr = cpu.x(rs1(insn)).wrapping_add(((insn as i32) >> 20) as i64 as u64);
            match funct3(insn) {
                2 => {
                    let v = cpu.load(bus, addr, 4)?;
                    write_f32(cpu, rd(insn), v as u32);
                }
                3 => {
                    let v = cpu.load(bus, addr, 8)?;
                    write_f(cpu, rd(insn), v);
                }
                _ => return Err(illegal),
            }
            Ok(())
        }
        OP_STORE_FP => {
            let imm = (((insn as i32) >> 25) << 5) as i64 as u64 | ((insn >> 7) & 31) as u64;
            let addr = cpu.x(rs1(insn)).wrapping_add(imm);
            let value = cpu.f(rs2(insn));
            match funct3(insn) {
                2 => cpu.store(bus, addr, 4, value & 0xffff_ffff),
                3 => cpu.store(bus, addr, 8, value),
                _ => Err(illegal),
            }
        }
        OP_MADD | OP_MSUB | OP_NMSUB | OP_NMADD => execute_fma(cpu, insn),
        OP_FP => execute_op_fp(cpu, insn),
        _ => Err(illegal),
    }
}

fn execute_fma(cpu: &mut Cpu, insn: u32) -> Result<(), Exception> {
    let fmt = (insn >> 25) & 3;
    if fmt > FMT_D {
        return Err(Exception::illegal(insn));
    }
    let rm = rounding_mode(cpu, insn)?;
    let mut fl = Flags::empty();
    let op = insn & 0x7f;
    if fmt == FMT_S {
        let (a, b, c) = (f32_operand(cpu, rs1(insn)), f32_operand(cpu, rs2(insn)), f32_operand(cpu, rs3(insn)));
        let r = match op {
            OP_MADD => sf::f32_fmadd(a, b, c, rm, &mut fl),
            OP_MSUB => sf::f32_fmsub(a, b, c, rm, &mut fl),
            OP_NMSUB => sf::f32_fnmsub(a, b, c, rm, &mut fl),
            _ => sf::f32_fnmadd(a, b, c, rm, &mut fl),
        };
        write_f32(cpu, rd(insn), r);
    } else {
        let (a, b, c) = (cpu.f(rs1(insn)), cpu.f(rs2(insn)), cpu.f(rs3(insn)));
        let r = match op {
            OP_MADD => sf::f64_fmadd(a, b, c, rm, &mut fl),
            OP_MSUB => sf::f64_fmsub(a, b, c, rm, &mut fl),
            OP_NMSUB => sf::f64_fnmsub(a, b, c, rm, &mut fl),
            _ => sf::f64_fnmadd(a, b, c, rm, &mut fl),
        };
        write_f(cpu, rd(insn), r);
    }
    accrue(cpu, fl);
    Ok(())
}

fn execute_op_fp(cpu: &mut Cpu, insn: u32) -> Result<(), Exception> {
    let illegal = Exception::illegal(insn);
    let funct7 = insn >> 25;
    let fmt = funct7 & 3;
    if fmt > FMT_D {
        return Err(illegal);
    }
    let single = fmt == FMT_S;
    let (rd, rs1, rs2, f3) = (rd(insn), rs1(insn), rs2(insn), funct3(insn));
    let mut fl = Flags::empty();
    match funct7 >> 2 {
        // FADD, FSUB, FMUL, FDIV
        0x00..=0x03 => {
            let rm = rounding_mode(cpu, insn)?;
            let op = funct7 >> 2;
            if single {
                let (a, b) = (f32_operand(cpu, rs1), f32_operand(cpu, rs2));
                let r = match op {
                    0 => sf::f32_add(a, b, rm, &mut fl),
                    1 => sf::f32_sub(a, b, rm, &mut fl),
                    2 => sf::f32_mul(a, b, rm, &mut fl),
                    _ => sf::f32_div(a, b, rm, &mut fl),
                };
                write_f32(cpu, rd, r);
            } else {
                let (a, b) = (cpu.f(rs1), cpu.f(rs2));
                let r = match op {
                    0 => sf::f64_add(a, b, rm, &mut fl),
                    1 => sf::f64_sub(a, b, rm, &mut fl),
                    2 => sf::f64_mul(a, b, rm, &mut fl),
                    _ => sf::f64_div(a, b, rm, &mut fl),
                };
                write_f(cpu, rd, r);
            }
        }
        // FSQRT
        0x0b => {
            if rs2 != 0 {
                return Err(illegal);
            }
            let rm = rounding_mode(cpu, insn)?;
            if single {
                let r = sf::f32_sqrt(f32_operand(cpu, rs1), rm, &mut fl);
                write_f32(cpu, rd, r);
            } else {
                let r = sf::f64_sqrt(cpu.f(rs1), rm, &mut fl);
                write_f(cpu, rd, r);
            }
        }
        // FSGNJ, FSGNJN, FSGNJX
        0x04 => {
            if f3 > 2 {
                return Err(illegal);
            }
            if single {
                let (a, b) = (f32_operand(cpu, rs1), f32_operand(cpu, rs2));
                let r = match f3 {
                    0 => sf::f32_sgnj(a, b),
                    1 => sf::f32_sgnjn(a, b),
                    _ => sf::f32_sgnjx(a, b),
                };
                write_f32(cpu, rd, r);
            } else {
                let (a, b) = (cpu.f(rs1), cpu.f(rs2));
                let r = match f3 {
                    0 => sf::f64_sgnj(a, b),
                    1 => sf::f64_sgnjn(a, b),
                    _ => sf::f64_sgnjx(a, b),
                };
                write_f(cpu, rd, r);
            }
        }
        // FMIN, FMAX
        0x05 => {
            if f3 > 1 {
                return Err(illegal);
            }
            if single {
                let (a, b) = (f32_operand(cpu, rs1), f32_operand(cpu, rs2));
                let r = if f3 == 0 { sf::f32_min(a, b, &mut fl) } else { sf::f32_max(a, b, &mut fl) };
                write_f32(cpu, rd, r);
            } else {
                let (a, b) = (cpu.f(rs1), cpu.f(rs2));
                let r = if f3 == 0 { sf::f64_min(a, b, &mut fl) } else { sf::f64_max(a, b, &mut fl) };
                write_f(cpu, rd, r);
            }
        }
        // FCVT.S.D (fmt S, rs2 = D) and FCVT.D.S (fmt D, rs2 = S)
        0x08 => {
            if single && rs2 == FMT_D {
                let rm = rounding_mode(cpu, insn)?;
                let r = sf::f64_to_f32(cpu.f(rs1), rm, &mut fl);
                write_f32(cpu, rd, r);
            } else if !single && rs2 == FMT_S {
                rounding_mode(cpu, insn)?;
                let r = sf::f32_to_f64(f32_operand(cpu, rs1), &mut fl);
                write_f(cpu, rd, r);
            } else {
                return Err(illegal);
            }
        }
        // FEQ, FLT, FLE
        0x14 => {
            let r = if single {
                let (a, b) = (f32_operand(cpu, rs1), f32_operand(cpu, rs2));
                match f3 {
                    2 => sf::f32_eq(a, b, &mut fl),
                    1 => sf::f32_lt(a, b, &mut fl),
                    0 => sf::f32_le(a, b, &mut fl),
                    _ => return Err(illegal),
                }
            } else {
                let (a, b) = (cpu.f(rs1), cpu.f(rs2));
                match f3 {
                    2 => sf::f64_eq(a, b, &mut fl),
                    1 => sf::f64_lt(a, b, &mut fl),
                    0 => sf::f64_le(a, b, &mut fl),
                    _ => return Err(illegal),
                }
            };
            cpu.set_x(rd, r as u64);
        }
        // FCVT.{W,WU,L,LU}.fmt
        0x18 => {
            if rs2 > 3 {
                return Err(illegal);
            }
            let rm = rounding_mode(cpu, insn)?;
            let r = if single {
                let a = f32_operand(cpu, rs1);
                match rs2 {
                    0 => sext32(sf::f32_to_i32(a, rm, &mut fl) as u32),
                    1 => sext32(sf::f32_to_u32(a, rm, &mut fl)),
                    2 => sf::f32_to_i64(a, rm, &mut fl) as u64,
                    _ => sf::f32_to_u64(a, rm, &mut fl),
                }
            } else {
                let a = cpu.f(rs1);
                match rs2 {
                    0 => sext32(sf::f64_to_i32(a, rm, &mut fl) as u32),
                    1 => sext32(sf::f64_to_u32(a, rm, &mut fl)),
                    2 => sf::f64_to_i64(a, rm, &mut fl) as u64,
                    _ => sf::f64_to_u64(a, rm, &mut fl),
                }
            };
            cpu.set_x(rd, r);
        }
        // FCVT.fmt.{W,WU,L,LU}
        0x1a => {
            if rs2 > 3 {
                return Err(illegal);
            }
            let rm = rounding_mode(cpu, insn)?;
            let x = cpu.x(rs1);
            if single {
                let r = match rs2 {
                    0 => sf::i32_to_f32(x as i32, rm, &mut fl),
                    1 => sf::u32_to_f32(x as u32, rm, &mut fl),
                    2 => sf::i64_to_f32(x as i64, rm, &mut fl),
                    _ => sf::u64_to_f32(x, rm, &mut fl),
                };
                write_f32(cpu, rd, r);
            } else {
                let r = match rs2 {
                    0 => sf::i32_to_f64(x as i32, rm, &mut fl),
                    1 => sf::u32_to_f64(x as u32, rm, &mut fl),
                    2 => sf::i64_to_f64(x as i64, rm, &mut fl),
                    _ => sf::u64_to_f64(x, rm, &mut fl),
                };
                write_f(cpu, rd, r);
            }
        }
        // FMV.X.W / FMV.X.D (funct3 0) and FCLASS (funct3 1)
        0x1c => {
            if rs2 != 0 {
                return Err(illegal);
            }
            let r = match (f3, single) {
                (0, true) => sext32(cpu.f(rs1) as u32),
                (0, false) => cpu.f(rs1),
                (1, true) => sf::f32_classify(f32_operand(cpu, rs1)),
                (1, false) => sf::f64_classify(cpu.f(rs1)),
                _ => return Err(illegal),
            };
            cpu.set_x(rd, r);
        }
        // FMV.W.X / FMV.D.X
        0x1e => {
            if rs2 != 0 || f3 != 0 {
                return Err(illegal);
            }
            let x = cpu.x(rs1);
            if single {
                write_f32(cpu, rd, x as u32);
            } else {
                write_f(cpu, rd, x);
            }
        }
        _ => return Err(illegal),
    }
    accrue(cpu, fl);
    Ok(())
}

/// Read `fflags` (0x001), `frm` (0x002), or `fcsr` (0x003). `Err` means illegal instruction.
pub fn read_csr(cpu: &Cpu, addr: u16) -> Result<u64, ()> {
    if cpu.mstatus_fs() == FS_OFF {
        return Err(());
    }
    let fp = &cpu.fp;
    Ok(match addr {
        csr::FFLAGS => fp.fflags as u64,
        csr::FRM => fp.frm as u64,
        csr::FCSR => ((fp.frm as u64) << 5) | fp.fflags as u64,
        _ => return Err(()),
    })
}

/// Write `fflags`, `frm`, or `fcsr`. `Err` means illegal instruction.
///
/// Logs the masked values the way Spike does: `fflags` and/or `frm` entries, never an
/// `fcsr` entry, plus `mstatus` when FS becomes Dirty.
pub fn write_csr(cpu: &mut Cpu, addr: u16, value: u64) -> Result<(), ()> {
    if cpu.mstatus_fs() == FS_OFF {
        return Err(());
    }
    let (fflags, frm) = match addr {
        csr::FFLAGS => (Some(value & 0x1f), None),
        csr::FRM => (None, Some(value & 0x7)),
        csr::FCSR => (Some(value & 0x1f), Some((value >> 5) & 0x7)),
        _ => return Err(()),
    };
    if let Some(v) = fflags {
        cpu.fp.fflags = v as u8;
        cpu.log_csr_write(csr::FFLAGS, v);
    }
    if let Some(v) = frm {
        cpu.fp.frm = v as u8;
        cpu.log_csr_write(csr::FRM, v);
    }
    cpu.set_mstatus_fs(FS_DIRTY);
    Ok(())
}

#[cfg(test)]
mod tests;
