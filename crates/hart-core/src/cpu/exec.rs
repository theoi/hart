//! Instruction fetch and execution.

use super::{Access, Cpu, Exception};
use crate::bus::Bus;

/// Fetch, decode, and execute one instruction at `cpu.pc`. On success returns the raw
/// instruction bits (16-bit encodings in the low half) and leaves `pc` at the next
/// instruction. On an exception, `pc` is unchanged.
pub fn step(cpu: &mut Cpu, bus: &mut Bus) -> Result<u32, Exception> {
    let pc = cpu.pc;
    let pa = cpu.translate(bus, pc, 2, Access::Fetch)?;
    let lo = bus.read(pa, 2).map_err(|_| Access::Fetch.access_fault(pc))? as u32;
    Err(Exception::illegal(lo))
}
