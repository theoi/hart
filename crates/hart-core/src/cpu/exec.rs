//! Instruction fetch and execution.

use super::csr;
use super::decode::{decode, Insn, Op};
use super::{fp, Access, Cpu, Exception, ExceptionCause, Priv};
use crate::bus::{Bus, CodeSlot};

/// Fetch the instruction at `cpu.pc`. Returns the raw bits (16-bit encodings in the low
/// half). A 32-bit instruction that crosses a page boundary is fetched in two parts.
#[inline]
pub fn fetch(cpu: &mut Cpu, bus: &mut Bus) -> Result<u32, Exception> {
    let pc = cpu.pc;
    if pc & 0xfff != 0xffe {
        let pa = cpu.translate(bus, pc, 2, Access::Fetch)?;
        let lo = bus.read(pa, 2).map_err(|_| Access::Fetch.access_fault(pc))? as u32;
        if lo & 3 != 3 {
            return Ok(lo);
        }
        let hi = bus.read(pa + 2, 2).map_err(|_| Access::Fetch.access_fault(pc))? as u32;
        Ok(lo | (hi << 16))
    } else {
        let pa = cpu.translate(bus, pc, 2, Access::Fetch)?;
        let lo = bus.read(pa, 2).map_err(|_| Access::Fetch.access_fault(pc))? as u32;
        if lo & 3 != 3 {
            return Ok(lo);
        }
        let pc2 = pc.wrapping_add(2);
        let pa2 = cpu.translate(bus, pc2, 2, Access::Fetch)?;
        let hi = bus.read(pa2, 2).map_err(|_| Access::Fetch.access_fault(pc2))? as u32;
        Ok(lo | (hi << 16))
    }
}

/// Fetch and decode the instruction at `cpu.pc`, using the decoded-instruction cache
/// for RAM pages reachable through the fetch TLB.
#[inline(always)]
pub fn fetch_decoded(cpu: &mut Cpu, bus: &mut Bus) -> Result<(Insn, u32), Exception> {
    let pc = cpu.pc;
    if let Some(page) = cpu.mmu.lookup(pc, Access::Fetch) {
        let off = page as usize + (pc & 0xfff) as usize;
        if let Some(slot) = bus.ram.cached_code(off) {
            return Ok((slot.insn, slot.raw));
        }
        if let Some(r) = fill_code(bus, pc, off) {
            return Ok(r);
        }
    }
    let raw = fetch(cpu, bus)?;
    Ok((decode(raw), raw))
}

/// Decode the instruction at RAM offset `off` (reached through a fetch TLB hit) and cache
/// it. Returns `None` for a 32-bit instruction that continues on the next page, which
/// needs a second translation.
#[cold]
fn fill_code(bus: &mut Bus, pc: u64, off: usize) -> Option<(Insn, u32)> {
    let lo = bus.ram.read_at(off, 2) as u32;
    let raw = if lo & 3 != 3 {
        lo
    } else if pc & 0xfff == 0xffe {
        return None;
    } else {
        bus.ram.read_at(off, 4) as u32
    };
    let insn = decode(raw);
    if raw != 0 {
        bus.ram.cache_code(off, CodeSlot { insn, raw });
    }
    Some((insn, raw))
}

/// Fetch, decode, and execute one instruction at `cpu.pc`. On success returns the raw
/// instruction bits and leaves `pc` at the next instruction. On an exception, `pc` is
/// unchanged.
pub fn step(cpu: &mut Cpu, bus: &mut Bus) -> Result<u32, Exception> {
    let raw = fetch(cpu, bus)?;
    execute(cpu, bus, decode(raw), raw)?;
    Ok(raw)
}

#[inline]
fn sext32(v: u64) -> u64 {
    v as i32 as i64 as u64
}

fn ecall_cause(mode: Priv) -> ExceptionCause {
    match mode {
        Priv::User => ExceptionCause::EcallFromU,
        Priv::Supervisor => ExceptionCause::EcallFromS,
        Priv::Machine => ExceptionCause::EcallFromM,
    }
}

/// Execute a decoded instruction. `raw` is used for tracing and illegal-instruction tval.
pub fn execute(cpu: &mut Cpu, bus: &mut Bus, i: Insn, raw: u32) -> Result<(), Exception> {
    use Op::*;
    let pc = cpu.pc;
    let len = if raw & 3 == 3 { 4 } else { 2 };
    let mut next = pc.wrapping_add(len);
    let rd = i.rd as u32;
    let a = cpu.x[i.rs1 as usize];
    let b = cpu.x[i.rs2 as usize];
    let imm = i.imm as i64 as u64;
    let illegal = || Exception::illegal(if len == 2 { raw & 0xffff } else { raw });
    match i.op {
        Illegal => return Err(illegal()),
        Lui => cpu.set_x(rd, imm),
        Auipc => cpu.set_x(rd, pc.wrapping_add(imm)),
        Jal => {
            cpu.set_x(rd, next);
            next = pc.wrapping_add(imm);
        }
        Jalr => {
            let t = a.wrapping_add(imm) & !1;
            cpu.set_x(rd, next);
            next = t;
        }
        Beq | Bne | Blt | Bge | Bltu | Bgeu => {
            let taken = match i.op {
                Beq => a == b,
                Bne => a != b,
                Blt => (a as i64) < (b as i64),
                Bge => (a as i64) >= (b as i64),
                Bltu => a < b,
                _ => a >= b,
            };
            if taken {
                next = pc.wrapping_add(imm);
            }
        }
        Lb => {
            let v = cpu.load(bus, a.wrapping_add(imm), 1)?;
            cpu.set_x(rd, v as i8 as i64 as u64);
        }
        Lh => {
            let v = cpu.load(bus, a.wrapping_add(imm), 2)?;
            cpu.set_x(rd, v as i16 as i64 as u64);
        }
        Lw => {
            let v = cpu.load(bus, a.wrapping_add(imm), 4)?;
            cpu.set_x(rd, sext32(v));
        }
        Ld => {
            let v = cpu.load(bus, a.wrapping_add(imm), 8)?;
            cpu.set_x(rd, v);
        }
        Lbu => {
            let v = cpu.load(bus, a.wrapping_add(imm), 1)?;
            cpu.set_x(rd, v);
        }
        Lhu => {
            let v = cpu.load(bus, a.wrapping_add(imm), 2)?;
            cpu.set_x(rd, v);
        }
        Lwu => {
            let v = cpu.load(bus, a.wrapping_add(imm), 4)?;
            cpu.set_x(rd, v);
        }
        Sb => cpu.store(bus, a.wrapping_add(imm), 1, b)?,
        Sh => cpu.store(bus, a.wrapping_add(imm), 2, b)?,
        Sw => cpu.store(bus, a.wrapping_add(imm), 4, b)?,
        Sd => cpu.store(bus, a.wrapping_add(imm), 8, b)?,
        Addi => cpu.set_x(rd, a.wrapping_add(imm)),
        Slti => cpu.set_x(rd, ((a as i64) < (imm as i64)) as u64),
        Sltiu => cpu.set_x(rd, (a < imm) as u64),
        Xori => cpu.set_x(rd, a ^ imm),
        Ori => cpu.set_x(rd, a | imm),
        Andi => cpu.set_x(rd, a & imm),
        Slli => cpu.set_x(rd, a << (imm & 63)),
        Srli => cpu.set_x(rd, a >> (imm & 63)),
        Srai => cpu.set_x(rd, ((a as i64) >> (imm & 63)) as u64),
        Addiw => cpu.set_x(rd, sext32(a.wrapping_add(imm))),
        Slliw => cpu.set_x(rd, sext32(a << (imm & 31))),
        Srliw => cpu.set_x(rd, sext32((a as u32 >> (imm & 31)) as u64)),
        Sraiw => cpu.set_x(rd, ((a as i32) >> (imm & 31)) as i64 as u64),
        Add => cpu.set_x(rd, a.wrapping_add(b)),
        Sub => cpu.set_x(rd, a.wrapping_sub(b)),
        Sll => cpu.set_x(rd, a << (b & 63)),
        Slt => cpu.set_x(rd, ((a as i64) < (b as i64)) as u64),
        Sltu => cpu.set_x(rd, (a < b) as u64),
        Xor => cpu.set_x(rd, a ^ b),
        Srl => cpu.set_x(rd, a >> (b & 63)),
        Sra => cpu.set_x(rd, ((a as i64) >> (b & 63)) as u64),
        Or => cpu.set_x(rd, a | b),
        And => cpu.set_x(rd, a & b),
        Addw => cpu.set_x(rd, sext32(a.wrapping_add(b))),
        Subw => cpu.set_x(rd, sext32(a.wrapping_sub(b))),
        Sllw => cpu.set_x(rd, sext32(a << (b & 31))),
        Srlw => cpu.set_x(rd, sext32((a as u32 >> (b & 31)) as u64)),
        Sraw => cpu.set_x(rd, ((a as i32) >> (b & 31)) as i64 as u64),
        Mul => cpu.set_x(rd, a.wrapping_mul(b)),
        Mulh => cpu.set_x(rd, ((a as i64 as i128 * b as i64 as i128) >> 64) as u64),
        Mulhsu => cpu.set_x(rd, ((a as i64 as i128).wrapping_mul(b as i128) >> 64) as u64),
        Mulhu => cpu.set_x(rd, ((a as u128 * b as u128) >> 64) as u64),
        Div => {
            let (x, y) = (a as i64, b as i64);
            let q = if y == 0 { -1 } else { x.wrapping_div(y) };
            cpu.set_x(rd, q as u64);
        }
        Divu => cpu.set_x(rd, if b == 0 { u64::MAX } else { a / b }),
        Rem => {
            let (x, y) = (a as i64, b as i64);
            let r = if y == 0 { x } else { x.wrapping_rem(y) };
            cpu.set_x(rd, r as u64);
        }
        Remu => cpu.set_x(rd, if b == 0 { a } else { a % b }),
        Mulw => cpu.set_x(rd, sext32(a.wrapping_mul(b))),
        Divw => {
            let (x, y) = (a as i32, b as i32);
            let q = if y == 0 { -1 } else { x.wrapping_div(y) };
            cpu.set_x(rd, q as i64 as u64);
        }
        Divuw => {
            let (x, y) = (a as u32, b as u32);
            let q = if y == 0 { u32::MAX } else { x / y };
            cpu.set_x(rd, sext32(q as u64));
        }
        Remw => {
            let (x, y) = (a as i32, b as i32);
            let r = if y == 0 { x } else { x.wrapping_rem(y) };
            cpu.set_x(rd, r as i64 as u64);
        }
        Remuw => {
            let (x, y) = (a as u32, b as u32);
            let r = if y == 0 { x } else { x % y };
            cpu.set_x(rd, sext32(r as u64));
        }
        LrW | LrD => {
            let size = if i.op == LrW { 4 } else { 8 };
            if a & (size as u64 - 1) != 0 {
                return Err(Exception::new(ExceptionCause::LoadAccessFault, a));
            }
            let pa = cpu.translate(bus, a, size, Access::Load)?;
            let v = bus.read(pa, size).map_err(|_| Exception::new(ExceptionCause::LoadAccessFault, a))?;
            cpu.log_load(a);
            cpu.reservation = Some(pa);
            cpu.set_x(rd, if size == 4 { sext32(v) } else { v });
        }
        ScW | ScD => {
            let size = if i.op == ScW { 4 } else { 8 };
            if a & (size as u64 - 1) != 0 {
                return Err(Exception::new(ExceptionCause::StoreAccessFault, a));
            }
            let pa = cpu.translate(bus, a, size, Access::Store)?;
            let ok = cpu.reservation == Some(pa);
            cpu.reservation = None;
            if ok {
                bus.write(pa, size, b).map_err(|_| Exception::new(ExceptionCause::StoreAccessFault, a))?;
                cpu.log_store(a, b, size);
            }
            cpu.set_x(rd, (!ok) as u64);
        }
        AmoswapW | AmoaddW | AmoxorW | AmoandW | AmoorW | AmominW | AmomaxW | AmominuW | AmomaxuW => {
            if a & 3 != 0 {
                return Err(Exception::new(ExceptionCause::StoreAccessFault, a));
            }
            let pa = cpu.translate(bus, a, 4, Access::Store)?;
            let fault = |_| Exception::new(ExceptionCause::StoreAccessFault, a);
            let old = bus.read(pa, 4).map_err(fault)? as u32;
            let y = b as u32;
            let new = match i.op {
                AmoswapW => y,
                AmoaddW => old.wrapping_add(y),
                AmoxorW => old ^ y,
                AmoandW => old & y,
                AmoorW => old | y,
                AmominW => (old as i32).min(y as i32) as u32,
                AmomaxW => (old as i32).max(y as i32) as u32,
                AmominuW => old.min(y),
                _ => old.max(y),
            };
            bus.write(pa, 4, new as u64).map_err(fault)?;
            cpu.log_load(a);
            cpu.log_store(a, new as u64, 4);
            cpu.set_x(rd, old as i32 as i64 as u64);
        }
        AmoswapD | AmoaddD | AmoxorD | AmoandD | AmoorD | AmominD | AmomaxD | AmominuD | AmomaxuD => {
            if a & 7 != 0 {
                return Err(Exception::new(ExceptionCause::StoreAccessFault, a));
            }
            let pa = cpu.translate(bus, a, 8, Access::Store)?;
            let fault = |_| Exception::new(ExceptionCause::StoreAccessFault, a);
            let old = bus.read(pa, 8).map_err(fault)?;
            let new = match i.op {
                AmoswapD => b,
                AmoaddD => old.wrapping_add(b),
                AmoxorD => old ^ b,
                AmoandD => old & b,
                AmoorD => old | b,
                AmominD => (old as i64).min(b as i64) as u64,
                AmomaxD => (old as i64).max(b as i64) as u64,
                AmominuD => old.min(b),
                _ => old.max(b),
            };
            bus.write(pa, 8, new).map_err(fault)?;
            cpu.log_load(a);
            cpu.log_store(a, new, 8);
            cpu.set_x(rd, old);
        }
        Fence => {}
        FenceI => cpu.flush_icache(),
        Ecall => return Err(Exception::new(ecall_cause(cpu.mode), 0)),
        Ebreak => return Err(Exception::new(ExceptionCause::Breakpoint, pc)),
        Mret => {
            if cpu.mode != Priv::Machine {
                return Err(illegal());
            }
            csr::mret(cpu);
            return Ok(());
        }
        Sret => {
            let tsr = cpu.csr.mstatus & csr::MSTATUS_TSR != 0;
            if cpu.mode == Priv::User || (cpu.mode == Priv::Supervisor && tsr) {
                return Err(illegal());
            }
            csr::sret(cpu);
            return Ok(());
        }
        Wfi => {
            let tw = cpu.csr.mstatus & csr::MSTATUS_TW != 0;
            if cpu.mode == Priv::User || (cpu.mode == Priv::Supervisor && tw) {
                return Err(illegal());
            }
            cpu.waiting = true;
        }
        SfenceVma => {
            let tvm = cpu.csr.mstatus & csr::MSTATUS_TVM != 0;
            if cpu.mode == Priv::User || (cpu.mode == Priv::Supervisor && tvm) {
                return Err(illegal());
            }
            cpu.mmu.flush();
        }
        Csrrw | Csrrs | Csrrc | Csrrwi | Csrrsi | Csrrci => {
            let addr = i.imm as u16 & 0xfff;
            let src = match i.op {
                Csrrw | Csrrs | Csrrc => a,
                _ => i.rs1 as u64,
            };
            let raw32 = raw;
            match i.op {
                Csrrw | Csrrwi => {
                    let old = if rd != 0 { Some(csr::read(cpu, bus, addr, raw32)?) } else { None };
                    csr::write(cpu, bus, addr, src, raw32)?;
                    if let Some(old) = old {
                        cpu.set_x(rd, old);
                    }
                }
                _ => {
                    let old = csr::read(cpu, bus, addr, raw32)?;
                    if i.rs1 != 0 {
                        // Read-modify-write of mip/sip uses only the software-writable bits.
                        let base = match addr {
                            csr::MIP => cpu.csr.mip,
                            csr::SIP => cpu.csr.mip & cpu.csr.mideleg,
                            _ => old,
                        };
                        let new = if matches!(i.op, Csrrs | Csrrsi) { base | src } else { base & !src };
                        csr::write(cpu, bus, addr, new, raw32)?;
                    }
                    cpu.set_x(rd, old);
                }
            }
        }
        Fp => {
            fp::execute(cpu, bus, i.imm as u32).map_err(|e| {
                if e.cause == ExceptionCause::IllegalInstruction {
                    illegal()
                } else {
                    e
                }
            })?;
        }
    }
    cpu.pc = next;
    Ok(())
}
