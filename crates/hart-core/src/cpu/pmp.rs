//! Physical memory protection: 16 entries, 4-byte grain (G = 0).
//!
//! Decoded entry ranges are recomputed on every CSR write, so an access check is a scan
//! over the active entries only. M-mode accesses skip the scan entirely when no entry is
//! locked.

use super::{Access, Priv};

pub const PMP_ENTRIES: usize = 16;
const R: u8 = 1;
const W: u8 = 2;
const X: u8 = 4;
const A_SHIFT: u8 = 3;
const A_MASK: u8 = 3 << A_SHIFT;
const L: u8 = 0x80;
const A_OFF: u8 = 0;
const A_TOR: u8 = 1;
const A_NA4: u8 = 2;
const A_NAPOT: u8 = 3;
/// pmpaddr holds physical address bits 55:2.
const ADDR_MASK: u64 = (1 << 54) - 1;

#[derive(Clone, Copy, Debug, Default)]
struct Range {
    lo: u64,
    hi: u64,
    cfg: u8,
}

#[derive(Clone, Debug, Default)]
pub struct Pmp {
    cfg: [u8; PMP_ENTRIES],
    addr: [u64; PMP_ENTRIES],
    /// Active (A != OFF, non-empty) entries in priority order.
    active: [Range; PMP_ENTRIES],
    n_active: usize,
    any_locked: bool,
}

impl Pmp {
    pub fn new() -> Self {
        Pmp::default()
    }

    /// The `pmpNcfg` byte.
    pub fn cfg(&self, i: usize) -> u8 {
        self.cfg[i]
    }

    /// The `pmpaddrN` value.
    pub fn addr(&self, i: usize) -> u64 {
        self.addr[i]
    }

    fn mode(&self, i: usize) -> u8 {
        (self.cfg[i] & A_MASK) >> A_SHIFT
    }

    /// Read pmpcfgN or pmpaddrN. `None` for CSRs that do not exist on RV64 (odd pmpcfg).
    pub fn read_csr(&self, addr: u16) -> Option<u64> {
        match addr {
            0x3a0..=0x3af => {
                let n = (addr - 0x3a0) as usize;
                if n % 2 != 0 {
                    return None;
                }
                let base = n * 4;
                let mut v = 0u64;
                for k in 0..8 {
                    let i = base + k;
                    if i < PMP_ENTRIES {
                        v |= (self.cfg[i] as u64) << (8 * k);
                    }
                }
                Some(v)
            }
            0x3b0..=0x3ef => {
                let i = (addr - 0x3b0) as usize;
                Some(if i < PMP_ENTRIES { self.addr[i] } else { 0 })
            }
            _ => None,
        }
    }

    /// Write pmpcfgN or pmpaddrN. Returns false for CSRs that do not exist on RV64.
    pub fn write_csr(&mut self, addr: u16, value: u64) -> bool {
        match addr {
            0x3a0..=0x3af => {
                let n = (addr - 0x3a0) as usize;
                if n % 2 != 0 {
                    return false;
                }
                let base = n * 4;
                for k in 0..8 {
                    let i = base + k;
                    if i >= PMP_ENTRIES || self.cfg[i] & L != 0 {
                        continue;
                    }
                    let mut c = (value >> (8 * k)) as u8 & (R | W | X | A_MASK | L);
                    // R=0, W=1 is reserved; hart clears W.
                    if c & R == 0 {
                        c &= !W;
                    }
                    self.cfg[i] = c;
                }
            }
            0x3b0..=0x3ef => {
                let i = (addr - 0x3b0) as usize;
                if i < PMP_ENTRIES && !self.addr_locked(i) {
                    self.addr[i] = value & ADDR_MASK;
                }
            }
            _ => return false,
        }
        self.recompute();
        true
    }

    fn addr_locked(&self, i: usize) -> bool {
        self.cfg[i] & L != 0 || (i + 1 < PMP_ENTRIES && self.cfg[i + 1] & L != 0 && self.mode(i + 1) == A_TOR)
    }

    /// Byte range `[lo, hi)` matched by entry `i`, or `None` if it is off or empty.
    fn range(&self, i: usize) -> Option<(u64, u64)> {
        let a = self.addr[i];
        let (lo, hi) = match self.mode(i) {
            A_OFF => return None,
            A_TOR => (if i == 0 { 0 } else { self.addr[i - 1] << 2 }, a << 2),
            A_NA4 => (a << 2, (a << 2) + 4),
            A_NAPOT => {
                let ones = (!a).trailing_zeros().min(54) as u64;
                let lo = (a & !((1u64 << ones) - 1)) << 2;
                (lo, lo.wrapping_add(1u64 << (ones + 3)))
            }
            _ => unreachable!(),
        };
        // A TOR entry with pmpaddr[i-1] >= pmpaddr[i] matches nothing.
        (lo < hi).then_some((lo, hi))
    }

    fn recompute(&mut self) {
        self.n_active = 0;
        self.any_locked = false;
        for i in 0..PMP_ENTRIES {
            if let Some((lo, hi)) = self.range(i) {
                self.active[self.n_active] = Range { lo, hi, cfg: self.cfg[i] };
                self.n_active += 1;
                self.any_locked |= self.cfg[i] & L != 0;
            }
        }
    }

    /// True if an access of `len` bytes at `paddr` is permitted for `mode`. An access
    /// that only partly overlaps the highest-priority overlapping entry fails.
    #[inline]
    pub fn check(&self, paddr: u64, len: u64, access: Access, mode: Priv) -> bool {
        if mode == Priv::Machine && !self.any_locked {
            return true;
        }
        let end = paddr.wrapping_add(len);
        for r in &self.active[..self.n_active] {
            if !(paddr < r.hi && end > r.lo) {
                continue;
            }
            if !(paddr >= r.lo && end <= r.hi) {
                return false;
            }
            if mode == Priv::Machine && r.cfg & L == 0 {
                return true;
            }
            let need = match access {
                Access::Fetch => X,
                Access::Load => R,
                Access::Store => W,
            };
            return r.cfg & need != 0;
        }
        mode == Priv::Machine
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn napot_tor_and_default_deny() {
        let mut p = Pmp::new();
        assert!(p.check(0x8000_0000, 4, Access::Load, Priv::Machine));
        assert!(!p.check(0x8000_0000, 4, Access::Load, Priv::Supervisor));
        // Entry 0: NAPOT 0x8000_0000..+64KiB, RX.
        p.write_csr(0x3b0, (0x8000_0000u64 >> 2) | ((0x10000 >> 3) - 1));
        // Entry 1: TOR 0x8001_0000..0x9000_0000, RW.
        p.write_csr(0x3b1, 0x9000_0000u64 >> 2);
        p.write_csr(0x3a0, (A_NAPOT << A_SHIFT | R | X) as u64 | (((A_TOR << A_SHIFT) | R | W) as u64) << 8);
        assert!(p.check(0x8000_fffc, 4, Access::Fetch, Priv::User));
        assert!(!p.check(0x8000_fffc, 4, Access::Store, Priv::User));
        assert!(!p.check(0x8000_fffe, 4, Access::Load, Priv::User));
        assert!(p.check(0x8001_0000, 8, Access::Store, Priv::Supervisor));
        assert!(!p.check(0x9000_0000, 1, Access::Load, Priv::Supervisor));
        assert!(p.check(0x8001_0000, 4096, Access::Load, Priv::Supervisor));
        assert_eq!(p.read_csr(0x3a1), None);
    }

    #[test]
    fn locked_entry_applies_to_machine_mode() {
        let mut p = Pmp::new();
        // Entry 0: NA4 at 0x1000, locked, read-only.
        p.write_csr(0x3b0, 0x1000 >> 2);
        p.write_csr(0x3a0, ((A_NA4 << A_SHIFT) | R | L) as u64);
        assert!(p.check(0x1000, 4, Access::Load, Priv::Machine));
        assert!(!p.check(0x1000, 4, Access::Store, Priv::Machine));
        assert!(p.check(0x2000, 4, Access::Store, Priv::Machine));
        // Locked: further writes are ignored.
        p.write_csr(0x3a0, 0);
        assert_eq!(p.cfg(0), (A_NA4 << A_SHIFT) | R | L);
    }

    #[test]
    fn empty_tor_matches_nothing() {
        let mut p = Pmp::new();
        p.write_csr(0x3b0, 0x3000 >> 2);
        p.write_csr(0x3b1, 0x1000 >> 2);
        // Entry 1: TOR [0x3000, 0x1000) is empty; locked so M-mode would see it.
        p.write_csr(0x3a0, ((((A_TOR << A_SHIFT) | L) as u64) << 8) | (((A_OFF << A_SHIFT) | L) as u64));
        assert!(p.check(0x0ffc, 0x2008, Access::Store, Priv::Machine));
    }
}
