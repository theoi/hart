//! Test-only interrupt trigger, modelled on the Sail RISC-V model's
//! interrupt generator ("SIG"). ACT4's interrupt tests use it to raise and
//! clear interrupts. It exists only on the bare-metal test profile, where it
//! sits at the PLIC's address (so that profile has no PLIC); the virt board
//! never has it.
//!
//! Registers (aligned 32-bit accesses only; anything else is an access
//! fault):
//!
//! | Offset | Name     | Behaviour                                              |
//! | ------ | -------- | ------------------------------------------------------ |
//! | `0x0`  | VERSION  | reads `0x0001_0001`; writes are ignored                |
//! | `0x4`  | PLATFORM | reads 0; a write sets (bit 31 = 1) or clears (bit 31 = 0) the latches selected by bits 11 (MEI), 9 (SEI), 3 (MSI), and 1 (SSI). Any other bit in 30:0 is an access fault |
//! | `0x8`+ | reserved | access fault                                           |
//!
//! The latches drive [`Wires::test_irq`], which the hart ORs into `mip`
//! separately from the CLINT and PLIC lines, so clearing a latch never
//! clears a software-written `mip` bit or the CLINT's MSIP.

use crate::bus::{BusFault, Device};
use crate::irq::{bits, Wires};

pub const SIG_BASE: u64 = 0x0c00_0000;
pub const SIG_SIZE: u64 = 0x20;

pub const SIG_VERSION: u32 = 0x0001_0001;
const LEVEL: u32 = 1 << 31;
const LATCHES: u64 = bits::MEIP | bits::SEIP | bits::MSIP | bits::SSIP;

#[derive(Clone, Debug, Default)]
pub struct InterruptTrigger {
    latches: u64,
}

impl InterruptTrigger {
    pub const fn new() -> Self {
        Self { latches: 0 }
    }

    /// The asserted latches, in `mip` layout.
    pub fn latches(&self) -> u64 {
        self.latches
    }
}

impl Device for InterruptTrigger {
    fn read(&mut self, offset: u64, size: u8, _wires: &mut Wires) -> Result<u64, BusFault> {
        match (offset, size) {
            (0x0, 4) => Ok(u64::from(SIG_VERSION)),
            (0x4, 4) => Ok(0),
            _ => Err(BusFault),
        }
    }

    fn write(&mut self, offset: u64, size: u8, value: u64, wires: &mut Wires) -> Result<(), BusFault> {
        match (offset, size) {
            (0x0, 4) => Ok(()),
            (0x4, 4) => {
                let value = value as u32;
                let select = u64::from(value & !LEVEL);
                if select & !LATCHES != 0 {
                    return Err(BusFault);
                }
                if value & LEVEL != 0 {
                    self.latches |= select;
                } else {
                    self.latches &= !select;
                }
                wires.test_irq = self.latches;
                Ok(())
            }
            _ => Err(BusFault),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_and_platform_reads() {
        let mut s = InterruptTrigger::new();
        let mut w = Wires::default();
        assert_eq!(s.read(0, 4, &mut w), Ok(0x0001_0001));
        assert_eq!(s.read(4, 4, &mut w), Ok(0));
        s.write(0, 4, 0xffff_ffff, &mut w).unwrap();
        assert_eq!(s.read(0, 4, &mut w), Ok(0x0001_0001), "VERSION ignores writes");
    }

    #[test]
    fn set_and_clear_latches() {
        let mut s = InterruptTrigger::new();
        let mut w = Wires::default();
        s.write(4, 4, (1 << 31) | (1 << 11), &mut w).unwrap();
        assert_eq!(w.test_irq, bits::MEIP);
        s.write(4, 4, (1 << 31) | (1 << 9) | (1 << 3) | (1 << 1), &mut w).unwrap();
        assert_eq!(w.test_irq, bits::MEIP | bits::SEIP | bits::MSIP | bits::SSIP);
        s.write(4, 4, 1 << 11, &mut w).unwrap();
        assert_eq!(w.test_irq, bits::SEIP | bits::MSIP | bits::SSIP);
        s.write(4, 4, (1 << 9) | (1 << 3) | (1 << 1), &mut w).unwrap();
        assert_eq!(w.test_irq, 0);
        assert_eq!(w.hart_irq, 0, "the CLINT/PLIC lines are untouched");
    }

    #[test]
    fn faults() {
        let mut s = InterruptTrigger::new();
        let mut w = Wires::default();
        assert_eq!(s.write(4, 4, (1 << 31) | (1 << 7), &mut w), Err(BusFault), "MTIP is not a latch");
        assert_eq!(s.write(4, 4, 1 << 0, &mut w), Err(BusFault));
        assert_eq!(w.test_irq, 0, "a faulting write changes nothing");
        assert_eq!(s.read(0, 8, &mut w), Err(BusFault));
        assert_eq!(s.read(0, 2, &mut w), Err(BusFault));
        assert_eq!(s.write(4, 1, 0, &mut w), Err(BusFault));
        assert_eq!(s.read(8, 4, &mut w), Err(BusFault), "GUEST is H-only");
        assert_eq!(s.write(8, 4, 0, &mut w), Err(BusFault));
        assert_eq!(s.read(0x1c, 4, &mut w), Err(BusFault));
    }
}
