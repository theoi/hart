//! SiFive-compatible core-local interruptor (CLINT) for one hart.
//!
//! `mtime` lives in [`Wires::mtime`]: the board advances it (from host time
//! or from the instruction count) and the `time` CSR reads it. The CLINT
//! holds `msip` and `mtimecmp`, and drives MSIP and MTIP on every access and
//! on [`Device::tick`].

use super::{reg_read, reg_write};
use crate::bus::{BusFault, Device};
use crate::irq::{bits, Wires};

pub const CLINT_BASE: u64 = 0x0200_0000;
pub const CLINT_SIZE: u64 = 0x1_0000;

const MSIP: u64 = 0x0000;
const MSIP_END: u64 = MSIP + 3;
const MTIMECMP: u64 = 0x4000;
const MTIMECMP_END: u64 = MTIMECMP + 7;
const MTIME: u64 = 0xbff8;
const MTIME_END: u64 = MTIME + 7;

#[derive(Clone, Debug, Default)]
pub struct Clint {
    msip: bool,
    mtimecmp: u64,
}

impl Clint {
    /// mtimecmp resets to 0, as in QEMU and Spike, so MTIP reads as pending
    /// until firmware programs the comparator.
    pub const fn new() -> Self {
        Self { msip: false, mtimecmp: 0 }
    }

    pub fn mtimecmp(&self) -> u64 {
        self.mtimecmp
    }

    pub fn msip(&self) -> bool {
        self.msip
    }

    /// Ticks from `mtime` until MTIP asserts, 0 when it already has.
    pub fn ticks_until_mtip(&self, mtime: u64) -> u64 {
        self.mtimecmp.saturating_sub(mtime)
    }

    fn drive(&self, wires: &mut Wires) {
        wires.set_hart_irq(bits::MSIP, self.msip);
        wires.set_hart_irq(bits::MTIP, wires.mtime >= self.mtimecmp);
    }
}

impl Device for Clint {
    fn read(&mut self, offset: u64, size: u8, wires: &mut Wires) -> Result<u64, BusFault> {
        Ok(match offset {
            MSIP..=MSIP_END => reg_read(u64::from(self.msip), offset - MSIP, size),
            MTIMECMP..=MTIMECMP_END => reg_read(self.mtimecmp, offset - MTIMECMP, size),
            MTIME..=MTIME_END => reg_read(wires.mtime, offset - MTIME, size),
            _ => 0,
        })
    }

    fn write(&mut self, offset: u64, size: u8, value: u64, wires: &mut Wires) -> Result<(), BusFault> {
        match offset {
            MSIP..=MSIP_END => {
                self.msip = reg_write(u64::from(self.msip), offset - MSIP, size, value) & 1 != 0;
            }
            MTIMECMP..=MTIMECMP_END => {
                self.mtimecmp = reg_write(self.mtimecmp, offset - MTIMECMP, size, value);
            }
            MTIME..=MTIME_END => wires.mtime = reg_write(wires.mtime, offset - MTIME, size, value),
            _ => {}
        }
        self.drive(wires);
        Ok(())
    }

    fn tick(&mut self, wires: &mut Wires) {
        self.drive(wires);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mtip(w: &Wires) -> bool {
        w.hart_irq & bits::MTIP != 0
    }

    #[test]
    fn msip_is_bit_zero_only() {
        let mut c = Clint::new();
        let mut w = Wires::default();
        c.write(0, 4, 0xffff_fffe, &mut w).unwrap();
        assert!(!c.msip());
        assert_eq!(w.hart_irq & bits::MSIP, 0);
        c.write(0, 4, 1, &mut w).unwrap();
        assert_eq!(w.hart_irq & bits::MSIP, bits::MSIP);
        assert_eq!(c.read(0, 4, &mut w), Ok(1));
        assert_eq!(c.read(0, 8, &mut w), Ok(1));
        c.write(0, 4, 0, &mut w).unwrap();
        assert_eq!(w.hart_irq & bits::MSIP, 0);
    }

    #[test]
    fn timer_compare_follows_mtime() {
        let mut c = Clint::new();
        let mut w = Wires::default();
        c.tick(&mut w);
        assert!(mtip(&w), "mtimecmp resets to 0");
        c.write(0x4000, 8, 1000, &mut w).unwrap();
        assert!(!mtip(&w));
        assert_eq!(c.ticks_until_mtip(w.mtime), 1000);
        w.mtime = 999;
        c.tick(&mut w);
        assert!(!mtip(&w));
        w.mtime = 1000;
        c.tick(&mut w);
        assert!(mtip(&w));
        assert_eq!(c.ticks_until_mtip(w.mtime), 0);
        c.write(0x4000, 8, u64::MAX, &mut w).unwrap();
        assert!(!mtip(&w), "writing mtimecmp updates MTIP at once");
    }

    #[test]
    fn mtimecmp_32bit_halves() {
        let mut c = Clint::new();
        let mut w = Wires::default();
        c.write(0x4004, 4, 0x1234_5678, &mut w).unwrap();
        c.write(0x4000, 4, 0x9abc_def0, &mut w).unwrap();
        assert_eq!(c.mtimecmp(), 0x1234_5678_9abc_def0);
        assert_eq!(c.read(0x4004, 4, &mut w), Ok(0x1234_5678));
        assert_eq!(c.read(0x4000, 4, &mut w), Ok(0x9abc_def0));
    }

    #[test]
    fn mtime_is_the_shared_wire() {
        let mut c = Clint::new();
        let mut w = Wires { mtime: 500, ..Wires::default() };
        assert_eq!(c.read(0xbff8, 8, &mut w), Ok(500));
        c.write(0xbff8, 8, 10_000, &mut w).unwrap();
        assert_eq!(w.mtime, 10_000);
        c.write(0xbffc, 4, 1, &mut w).unwrap();
        assert_eq!(w.mtime, (1 << 32) | 10_000);
        assert_eq!(c.read(0xbffc, 4, &mut w), Ok(1));
    }

    #[test]
    fn unmapped_offsets_read_zero() {
        let mut c = Clint::new();
        let mut w = Wires::default();
        c.write(0x8000, 8, 0xffff, &mut w).unwrap();
        assert_eq!(c.read(0x8000, 8, &mut w), Ok(0));
        assert_eq!(c.read(0x4008, 8, &mut w), Ok(0));
    }
}
