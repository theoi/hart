//! SiFive test finisher, used as the syscon power-off/reboot register
//! (`sifive,test0` + `syscon`, with `syscon-poweroff` and `syscon-reboot`
//! nodes pointing at offset 0), as on QEMU virt.
//!
//! A write of FINISHER_PASS requests [`Halt::PowerOff`], FINISHER_RESET
//! requests [`Halt::Reboot`], and FINISHER_FAIL with a code in bits 31:16
//! requests [`Halt::TestExit`] with that code (at least 1).

use crate::bus::{BusFault, Device};
use crate::irq::{Halt, Wires};

pub const SYSCON_BASE: u64 = 0x10_0000;
pub const SYSCON_SIZE: u64 = 0x1000;

pub const FINISHER_FAIL: u32 = 0x3333;
pub const FINISHER_PASS: u32 = 0x5555;
pub const FINISHER_RESET: u32 = 0x7777;

#[derive(Clone, Debug, Default)]
pub struct Syscon;

impl Syscon {
    pub const fn new() -> Self {
        Self
    }
}

impl Device for Syscon {
    fn read(&mut self, _offset: u64, _size: u8, _wires: &mut Wires) -> Result<u64, BusFault> {
        Ok(0)
    }

    fn write(&mut self, offset: u64, size: u8, value: u64, wires: &mut Wires) -> Result<(), BusFault> {
        if offset != 0 || size < 2 || wires.halt.is_some() {
            return Ok(());
        }
        let value = value as u32;
        wires.halt = match value & 0xffff {
            FINISHER_PASS => Some(Halt::PowerOff),
            FINISHER_FAIL => Some(Halt::TestExit(u64::from((value >> 16).max(1)))),
            FINISHER_RESET => Some(Halt::Reboot),
            _ => None,
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(value: u64, offset: u64) -> Option<Halt> {
        let mut w = Wires::default();
        Syscon::new().write(offset, 4, value, &mut w).unwrap();
        w.halt
    }

    #[test]
    fn finisher_values() {
        assert_eq!(write(0x1234, 0), None);
        assert_eq!(write(0x5555, 0), Some(Halt::PowerOff));
        assert_eq!(write(0x7777, 0), Some(Halt::Reboot));
        assert_eq!(write((7 << 16) | 0x3333, 0), Some(Halt::TestExit(7)));
        assert_eq!(write(0x3333, 0), Some(Halt::TestExit(1)));
        assert_eq!(write(0x5555, 4), None);
    }

    #[test]
    fn first_request_wins() {
        let mut s = Syscon::new();
        let mut w = Wires::default();
        s.write(0, 4, 0x7777, &mut w).unwrap();
        s.write(0, 4, 0x5555, &mut w).unwrap();
        assert_eq!(w.halt, Some(Halt::Reboot));
        assert_eq!(s.read(0, 4, &mut w), Ok(0));
    }
}
