//! Platform devices: CLINT, PLIC, NS16550A UART, syscon power control, the
//! HTIF host interface used by bare-metal tests and the Spike profile, and
//! the test-only interrupt trigger (SIG) used by ACT4.
//! The MMIO devices implement [`crate::bus::Device`]; HTIF is polled (see
//! [`htif`]).

pub mod clint;
pub mod htif;
pub mod plic;
pub mod sig;
pub mod syscon;
pub mod uart;

/// Reads `size` bytes starting `byte` bytes into a little-endian register.
#[inline]
pub(crate) fn reg_read(reg: u64, byte: u64, size: u8) -> u64 {
    if byte >= 8 {
        return 0;
    }
    let v = reg >> (byte * 8);
    if size >= 8 {
        v
    } else {
        v & ((1u64 << (u32::from(size) * 8)) - 1)
    }
}

/// Replaces `size` bytes starting `byte` bytes into a little-endian register.
#[inline]
pub(crate) fn reg_write(reg: u64, byte: u64, size: u8, value: u64) -> u64 {
    if byte >= 8 {
        return reg;
    }
    let mask = if size >= 8 { u64::MAX } else { (1u64 << (u32::from(size) * 8)) - 1 };
    let shift = byte * 8;
    (reg & !(mask << shift)) | ((value & mask) << shift)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_register_access() {
        let r = 0x1122_3344_5566_7788u64;
        assert_eq!(reg_read(r, 0, 8), r);
        assert_eq!(reg_read(r, 0, 4), 0x5566_7788);
        assert_eq!(reg_read(r, 4, 4), 0x1122_3344);
        assert_eq!(reg_read(r, 1, 1), 0x77);
        assert_eq!(reg_read(r, 8, 4), 0);
        assert_eq!(reg_write(r, 4, 4, 0xdead_beef), 0xdead_beef_5566_7788);
        assert_eq!(reg_write(r, 0, 2, 0xffff_0000), 0x1122_3344_5566_0000);
        assert_eq!(reg_write(r, 0, 8, 7), 7);
    }
}
