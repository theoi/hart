//! Instruction fetch and execution.

use super::csr;
use super::decode::{decode, Insn, Op};
use super::{fp, Access, Cpu, Exception, ExceptionCause, Priv};
use crate::bus::Bus;

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
/// for RAM pages reachable through the fetch TLB. Also returns the raw bits.
#[inline]
pub fn fetch_decoded(cpu: &mut Cpu, bus: &mut Bus) -> Result<(Insn, u32), Exception> {
    let pc = cpu.pc;
    if let Some(page) = cpu.mmu.lookup(pc, Access::Fetch) {
        let off = page as usize + (pc & 0xfff) as usize;
        let insn = bus.ram.code(bus.ram.code_slot(off));
        if insn.op != Op::Uncached {
            return Ok((insn, bus.ram.read_at(off, insn.size() as u8) as u32));
        }
        let filled = fill_code(bus, pc, off);
        if bus.ram.take_code_page_added() {
            cpu.mmu.flush_store();
        }
        if let Some(r) = filled {
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
pub(crate) fn fill_code(bus: &mut Bus, pc: u64, off: usize) -> Option<(Insn, u32)> {
    let lo = bus.ram.read_at(off, 2) as u32;
    let raw = if lo & 3 != 3 {
        lo
    } else if pc & 0xfff == 0xffe {
        return None;
    } else {
        bus.ram.read_at(off, 4) as u32
    };
    let insn = decode(raw);
    bus.ram.cache_code(off, insn);
    Some((insn, raw))
}

/// The raw bits of the instruction at `pc`, fetched again for the `xtval` of an
/// instruction found to be illegal after decoding. The same fetch has just succeeded.
#[cold]
fn refetch(cpu: &mut Cpu, bus: &mut Bus, pc: u64) -> u32 {
    let saved = cpu.pc;
    cpu.pc = pc;
    let raw = fetch(cpu, bus).unwrap_or(0);
    cpu.pc = saved;
    raw
}

#[cold]
fn illegal_at(cpu: &mut Cpu, bus: &mut Bus, pc: u64) -> Exception {
    Exception::illegal(refetch(cpu, bus, pc))
}

/// High 64 bits of the unsigned 128-bit product, computed from 32-bit halves: wasm32
/// has no 64x64->128 multiply, and `u128` arithmetic there becomes a libcall.
#[inline]
fn mulhu(a: u64, b: u64) -> u64 {
    let (a_lo, a_hi) = (a & 0xffff_ffff, a >> 32);
    let (b_lo, b_hi) = (b & 0xffff_ffff, b >> 32);
    let lh = a_lo * b_hi;
    let hl = a_hi * b_lo;
    let mid = ((a_lo * b_lo) >> 32) + (lh & 0xffff_ffff) + (hl & 0xffff_ffff);
    a_hi * b_hi + (lh >> 32) + (hl >> 32) + (mid >> 32)
}

/// MULH from the unsigned high product `hu` of the same operands.
#[inline]
fn mulh_from(hu: u64, a: u64, b: u64) -> u64 {
    let mut h = hu;
    if (a as i64) < 0 {
        h = h.wrapping_sub(b);
    }
    if (b as i64) < 0 {
        h = h.wrapping_sub(a);
    }
    h
}

/// MULHSU (`a` signed, `b` unsigned) from the unsigned high product `hu`.
#[inline]
fn mulhsu_from(hu: u64, a: u64, b: u64) -> u64 {
    if (a as i64) < 0 {
        hu.wrapping_sub(b)
    } else {
        hu
    }
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

/// Execute the decoded instruction `i` at `pc` and return the next pc. `cpu.pc` is not
/// read or written. `i` must not be a serializing op (see [`execute_serializing`]). With
/// `TRACE` false the commit-log hooks are compiled out, so the caller must ensure that no
/// tracer is installed.
#[inline(always)]
pub fn execute<const TRACE: bool>(cpu: &mut Cpu, bus: &mut Bus, i: Insn, pc: u64) -> Result<u64, Exception> {
    use Op::*;
    let mut next = pc.wrapping_add(i.size());
    let rd = i.rd as u32;
    let a = cpu.x[i.rs1()];
    let b = cpu.x[i.rs2 as usize & 31];
    let imm = i.imm as i64 as u64;
    match i.op {
        Illegal => return Err(illegal_at(cpu, bus, pc)),
        Lui => cpu.set_x_t::<TRACE>(rd, imm),
        Auipc => cpu.set_x_t::<TRACE>(rd, pc.wrapping_add(imm)),
        Jal => {
            cpu.set_x_t::<TRACE>(rd, next);
            next = pc.wrapping_add(imm);
        }
        Jalr => {
            let t = a.wrapping_add(imm) & !1;
            cpu.set_x_t::<TRACE>(rd, next);
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
            let v = cpu.load_t::<TRACE>(bus, a.wrapping_add(imm), 1)?;
            cpu.set_x_t::<TRACE>(rd, v as i8 as i64 as u64);
        }
        Lh => {
            let v = cpu.load_t::<TRACE>(bus, a.wrapping_add(imm), 2)?;
            cpu.set_x_t::<TRACE>(rd, v as i16 as i64 as u64);
        }
        Lw => {
            let v = cpu.load_t::<TRACE>(bus, a.wrapping_add(imm), 4)?;
            cpu.set_x_t::<TRACE>(rd, sext32(v));
        }
        Ld => {
            let v = cpu.load_t::<TRACE>(bus, a.wrapping_add(imm), 8)?;
            cpu.set_x_t::<TRACE>(rd, v);
        }
        Lbu => {
            let v = cpu.load_t::<TRACE>(bus, a.wrapping_add(imm), 1)?;
            cpu.set_x_t::<TRACE>(rd, v);
        }
        Lhu => {
            let v = cpu.load_t::<TRACE>(bus, a.wrapping_add(imm), 2)?;
            cpu.set_x_t::<TRACE>(rd, v);
        }
        Lwu => {
            let v = cpu.load_t::<TRACE>(bus, a.wrapping_add(imm), 4)?;
            cpu.set_x_t::<TRACE>(rd, v);
        }
        Sb => cpu.store_t::<TRACE>(bus, a.wrapping_add(imm), 1, b)?,
        Sh => cpu.store_t::<TRACE>(bus, a.wrapping_add(imm), 2, b)?,
        Sw => cpu.store_t::<TRACE>(bus, a.wrapping_add(imm), 4, b)?,
        Sd => cpu.store_t::<TRACE>(bus, a.wrapping_add(imm), 8, b)?,
        Addi => cpu.set_x_t::<TRACE>(rd, a.wrapping_add(imm)),
        Slti => cpu.set_x_t::<TRACE>(rd, ((a as i64) < (imm as i64)) as u64),
        Sltiu => cpu.set_x_t::<TRACE>(rd, (a < imm) as u64),
        Xori => cpu.set_x_t::<TRACE>(rd, a ^ imm),
        Ori => cpu.set_x_t::<TRACE>(rd, a | imm),
        Andi => cpu.set_x_t::<TRACE>(rd, a & imm),
        Slli => cpu.set_x_t::<TRACE>(rd, a << (imm & 63)),
        Srli => cpu.set_x_t::<TRACE>(rd, a >> (imm & 63)),
        Srai => cpu.set_x_t::<TRACE>(rd, ((a as i64) >> (imm & 63)) as u64),
        Addiw => cpu.set_x_t::<TRACE>(rd, sext32(a.wrapping_add(imm))),
        Slliw => cpu.set_x_t::<TRACE>(rd, sext32(a << (imm & 31))),
        Srliw => cpu.set_x_t::<TRACE>(rd, sext32((a as u32 >> (imm & 31)) as u64)),
        Sraiw => cpu.set_x_t::<TRACE>(rd, ((a as i32) >> (imm & 31)) as i64 as u64),
        Add => cpu.set_x_t::<TRACE>(rd, a.wrapping_add(b)),
        Sub => cpu.set_x_t::<TRACE>(rd, a.wrapping_sub(b)),
        Sll => cpu.set_x_t::<TRACE>(rd, a << (b & 63)),
        Slt => cpu.set_x_t::<TRACE>(rd, ((a as i64) < (b as i64)) as u64),
        Sltu => cpu.set_x_t::<TRACE>(rd, (a < b) as u64),
        Xor => cpu.set_x_t::<TRACE>(rd, a ^ b),
        Srl => cpu.set_x_t::<TRACE>(rd, a >> (b & 63)),
        Sra => cpu.set_x_t::<TRACE>(rd, ((a as i64) >> (b & 63)) as u64),
        Or => cpu.set_x_t::<TRACE>(rd, a | b),
        And => cpu.set_x_t::<TRACE>(rd, a & b),
        Addw => cpu.set_x_t::<TRACE>(rd, sext32(a.wrapping_add(b))),
        Subw => cpu.set_x_t::<TRACE>(rd, sext32(a.wrapping_sub(b))),
        Sllw => cpu.set_x_t::<TRACE>(rd, sext32(a << (b & 31))),
        Srlw => cpu.set_x_t::<TRACE>(rd, sext32((a as u32 >> (b & 31)) as u64)),
        Sraw => cpu.set_x_t::<TRACE>(rd, ((a as i32) >> (b & 31)) as i64 as u64),
        Mul => cpu.set_x_t::<TRACE>(rd, a.wrapping_mul(b)),
        Mulw => cpu.set_x_t::<TRACE>(rd, sext32(a.wrapping_mul(b))),
        Mulh | Mulhsu | Mulhu | Div | Divu | Rem | Remu | Divw | Divuw | Remw | Remuw | LrW | ScW | AmoswapW
        | AmoaddW | AmoxorW | AmoandW | AmoorW | AmominW | AmomaxW | AmominuW | AmomaxuW | LrD | ScD | AmoswapD
        | AmoaddD | AmoxorD | AmoandD | AmoorD | AmominD | AmomaxD | AmominuD | AmomaxuD => {
            execute_rare::<TRACE>(cpu, bus, i.op, rd, a, b)?
        }
        Fence => {}
        FenceI => cpu.flush_icache(),
        Ecall => return Err(Exception::new(ecall_cause(cpu.mode), 0)),
        Ebreak => return Err(Exception::new(ExceptionCause::Breakpoint, pc)),
        // These need the raw bits: callers send them to `execute_serializing`.
        Mret | Sret | Wfi | Csrrw | Csrrs | Csrrc | Csrrwi | Csrrsi | Csrrci => {
            unreachable!("serializing op passed to execute")
        }
        SfenceVma => {
            let tvm = cpu.csr.mstatus & csr::MSTATUS_TVM != 0;
            if cpu.mode == Priv::User || (cpu.mode == Priv::Supervisor && tvm) {
                return Err(illegal_at(cpu, bus, pc));
            }
            cpu.mmu.flush();
        }
        Fp => {
            if let Err(e) = fp::execute(cpu, bus, i.imm as u32) {
                if e.cause == ExceptionCause::IllegalInstruction {
                    return Err(illegal_at(cpu, bus, pc));
                }
                return Err(e);
            }
        }
        Uncached => unreachable!("executed an empty decoded-instruction slot"),
    }
    Ok(next)
}

/// The less frequent M and A instructions, kept out of line so the hot dispatch in
/// [`execute`] stays small. `a` and `b` are the `rs1` and `rs2` values.
#[inline(never)]
fn execute_rare<const TRACE: bool>(
    cpu: &mut Cpu,
    bus: &mut Bus,
    op: Op,
    rd: u32,
    a: u64,
    b: u64,
) -> Result<(), Exception> {
    use Op::*;
    match op {
        Mulh => cpu.set_x_t::<TRACE>(rd, mulh_from(mulhu(a, b), a, b)),
        Mulhsu => cpu.set_x_t::<TRACE>(rd, mulhsu_from(mulhu(a, b), a, b)),
        Mulhu => cpu.set_x_t::<TRACE>(rd, mulhu(a, b)),
        Div => {
            let (x, y) = (a as i64, b as i64);
            let q = if y == 0 { -1 } else { x.wrapping_div(y) };
            cpu.set_x_t::<TRACE>(rd, q as u64);
        }
        Divu => cpu.set_x_t::<TRACE>(rd, a.checked_div(b).unwrap_or(u64::MAX)),
        Rem => {
            let (x, y) = (a as i64, b as i64);
            let r = if y == 0 { x } else { x.wrapping_rem(y) };
            cpu.set_x_t::<TRACE>(rd, r as u64);
        }
        Remu => cpu.set_x_t::<TRACE>(rd, if b == 0 { a } else { a % b }),
        Divw => {
            let (x, y) = (a as i32, b as i32);
            let q = if y == 0 { -1 } else { x.wrapping_div(y) };
            cpu.set_x_t::<TRACE>(rd, q as i64 as u64);
        }
        Divuw => {
            let (x, y) = (a as u32, b as u32);
            let q = x.checked_div(y).unwrap_or(u32::MAX);
            cpu.set_x_t::<TRACE>(rd, sext32(q as u64));
        }
        Remw => {
            let (x, y) = (a as i32, b as i32);
            let r = if y == 0 { x } else { x.wrapping_rem(y) };
            cpu.set_x_t::<TRACE>(rd, r as i64 as u64);
        }
        Remuw => {
            let (x, y) = (a as u32, b as u32);
            let r = if y == 0 { x } else { x % y };
            cpu.set_x_t::<TRACE>(rd, sext32(r as u64));
        }
        LrW | LrD => {
            let size = if op == LrW { 4 } else { 8 };
            if a & (size as u64 - 1) != 0 {
                return Err(Exception::new(ExceptionCause::LoadAccessFault, a));
            }
            let pa = cpu.translate(bus, a, size, Access::Load)?;
            // Only RAM is reservable (as in Spike and Sail's PMAs).
            if bus.ram.offset(pa, size as u64).is_none() {
                return Err(Exception::new(ExceptionCause::LoadAccessFault, a));
            }
            let v = bus.read(pa, size).map_err(|_| Exception::new(ExceptionCause::LoadAccessFault, a))?;
            cpu.log_load_t::<TRACE>(a);
            cpu.reservation = Some(pa);
            cpu.set_x_t::<TRACE>(rd, if size == 4 { sext32(v) } else { v });
        }
        ScW | ScD => {
            let size = if op == ScW { 4 } else { 8 };
            if a & (size as u64 - 1) != 0 {
                return Err(Exception::new(ExceptionCause::StoreAccessFault, a));
            }
            // An SC is translated and checked (PMP, and that the target is reservable
            // RAM) even when it will fail, as in Spike and Sail.
            let pa = cpu.translate(bus, a, size, Access::Store)?;
            if bus.ram.offset(pa, size as u64).is_none() {
                return Err(Exception::new(ExceptionCause::StoreAccessFault, a));
            }
            let ok = cpu.reservation == Some(pa);
            cpu.reservation = None;
            if ok {
                bus.write(pa, size, b).map_err(|_| Exception::new(ExceptionCause::StoreAccessFault, a))?;
                cpu.after_bus_write(bus);
                cpu.log_store_t::<TRACE>(a, b, size);
            }
            cpu.set_x_t::<TRACE>(rd, (!ok) as u64);
        }
        AmoswapW | AmoaddW | AmoxorW | AmoandW | AmoorW | AmominW | AmomaxW | AmominuW | AmomaxuW => {
            if a & 3 != 0 {
                return Err(Exception::new(ExceptionCause::StoreAccessFault, a));
            }
            let pa = cpu.translate(bus, a, 4, Access::Store)?;
            let fault = |_| Exception::new(ExceptionCause::StoreAccessFault, a);
            let old = bus.read(pa, 4).map_err(fault)? as u32;
            let y = b as u32;
            let new = match op {
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
            cpu.after_bus_write(bus);
            cpu.log_load_t::<TRACE>(a);
            cpu.log_store_t::<TRACE>(a, new as u64, 4);
            cpu.set_x_t::<TRACE>(rd, old as i32 as i64 as u64);
        }
        AmoswapD | AmoaddD | AmoxorD | AmoandD | AmoorD | AmominD | AmomaxD | AmominuD | AmomaxuD => {
            if a & 7 != 0 {
                return Err(Exception::new(ExceptionCause::StoreAccessFault, a));
            }
            let pa = cpu.translate(bus, a, 8, Access::Store)?;
            let fault = |_| Exception::new(ExceptionCause::StoreAccessFault, a);
            let old = bus.read(pa, 8).map_err(fault)?;
            let new = match op {
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
            cpu.after_bus_write(bus);
            cpu.log_load_t::<TRACE>(a);
            cpu.log_store_t::<TRACE>(a, new, 8);
            cpu.set_x_t::<TRACE>(rd, old);
        }
        _ => unreachable!("not a rare op: {op:?}"),
    }
    Ok(())
}

/// Execute MRET, SRET, WFI, or a Zicsr instruction. Kept out of line so the hot
/// `execute` stays small.
#[inline(never)]
pub fn execute_serializing(cpu: &mut Cpu, bus: &mut Bus, i: Insn, raw: u32) -> Result<(), Exception> {
    use Op::*;
    // None of these have compressed forms.
    let illegal = Exception::illegal(raw);
    let rd = i.rd as u32;
    match i.op {
        Mret => {
            if cpu.mode != Priv::Machine {
                return Err(illegal);
            }
            csr::mret(cpu);
            return Ok(());
        }
        Sret => {
            let tsr = cpu.csr.mstatus & csr::MSTATUS_TSR != 0;
            if cpu.mode == Priv::User || (cpu.mode == Priv::Supervisor && tsr) {
                return Err(illegal);
            }
            csr::sret(cpu);
            return Ok(());
        }
        Wfi => {
            let tw = cpu.csr.mstatus & csr::MSTATUS_TW != 0;
            if cpu.mode == Priv::User || (cpu.mode == Priv::Supervisor && tw) {
                return Err(illegal);
            }
            cpu.waiting = true;
        }
        Csrrw | Csrrs | Csrrc | Csrrwi | Csrrsi | Csrrci => {
            let addr = i.imm as u16 & 0xfff;
            let src = match i.op {
                Csrrw | Csrrs | Csrrc => cpu.x[i.rs1()],
                _ => i.rs1() as u64,
            };
            match i.op {
                Csrrw | Csrrwi => {
                    let old = if rd != 0 { Some(csr::read(cpu, bus, addr, raw)?) } else { None };
                    csr::write(cpu, bus, addr, src, raw)?;
                    if let Some(old) = old {
                        cpu.set_x(rd, old);
                    }
                }
                _ => {
                    let old = csr::read(cpu, bus, addr, raw)?;
                    if i.rs1() != 0 {
                        // Read-modify-write of mip/sip uses only the software-writable bits.
                        let base = match addr {
                            csr::MIP => cpu.csr.mip,
                            csr::SIP => cpu.csr.mip & cpu.csr.mideleg,
                            _ => old,
                        };
                        let new = if matches!(i.op, Csrrs | Csrrsi) { base | src } else { base & !src };
                        csr::write(cpu, bus, addr, new, raw)?;
                    }
                    cpu.set_x(rd, old);
                }
            }
        }
        _ => unreachable!("not a serializing op: {:?}", i.op),
    }
    cpu.pc = cpu.pc.wrapping_add(4);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Edge values plus a deterministic pseudo-random sweep.
    fn operands() -> Vec<u64> {
        let mut v = vec![0, 1, 2, 0x7fff_ffff, 0x8000_0000, 0xffff_ffff, 1 << 32, i64::MAX as u64, 1 << 63, u64::MAX];
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        for _ in 0..200 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            v.push(x);
        }
        v
    }

    #[test]
    fn high_multiplies_from_halves_match_128_bit_products() {
        let ops = operands();
        for &a in &ops {
            for &b in &ops {
                let hu = mulhu(a, b);
                assert_eq!(hu, ((a as u128 * b as u128) >> 64) as u64, "mulhu {a:#x} {b:#x}");
                let h = ((a as i64 as i128 * b as i64 as i128) >> 64) as u64;
                assert_eq!(mulh_from(hu, a, b), h, "mulh {a:#x} {b:#x}");
                let hsu = ((a as i64 as i128).wrapping_mul(b as i128) >> 64) as u64;
                assert_eq!(mulhsu_from(hu, a, b), hsu, "mulhsu {a:#x} {b:#x}");
            }
        }
    }
}
