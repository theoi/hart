//! Address translation: Sv39 page walks with a software TLB, then PMP.
//!
//! A/D bits follow Svade: a leaf PTE with A = 0, or D = 0 on a store, raises a page fault
//! and the hart never writes PTEs.

use super::{csr, Cpu, Exception, ExceptionCause, Priv};
use crate::bus::Bus;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Access {
    Fetch,
    Load,
    Store,
}

impl Access {
    pub fn page_fault(self, vaddr: u64) -> Exception {
        let cause = match self {
            Access::Fetch => ExceptionCause::InstructionPageFault,
            Access::Load => ExceptionCause::LoadPageFault,
            Access::Store => ExceptionCause::StorePageFault,
        };
        Exception::new(cause, vaddr)
    }

    pub fn access_fault(self, vaddr: u64) -> Exception {
        let cause = match self {
            Access::Fetch => ExceptionCause::InstructionAccessFault,
            Access::Load => ExceptionCause::LoadAccessFault,
            Access::Store => ExceptionCause::StoreAccessFault,
        };
        Exception::new(cause, vaddr)
    }
}

pub const PTE_V: u64 = 1 << 0;
pub const PTE_R: u64 = 1 << 1;
pub const PTE_W: u64 = 1 << 2;
pub const PTE_X: u64 = 1 << 3;
pub const PTE_U: u64 = 1 << 4;
pub const PTE_G: u64 = 1 << 5;
pub const PTE_A: u64 = 1 << 6;
pub const PTE_D: u64 = 1 << 7;
/// Bits 63:54 (N, PBMT, reserved) must be zero: Svnapot and Svpbmt are not implemented.
const PTE_RESERVED: u64 = 0x3ff << 54;
const PTE_PPN_MASK: u64 = (1 << 44) - 1;

const TLB_SIZE: usize = 256;
const INVALID: u64 = u64::MAX;

#[derive(Clone, Copy, Debug)]
struct TlbEntry {
    /// Virtual page number, or `INVALID`.
    vpn: u64,
    /// Physical address of the 4 KiB page.
    ppage: u64,
    /// Leaf PTE permission bits (R, W, X, U, A, D).
    flags: u64,
}

const EMPTY: TlbEntry = TlbEntry { vpn: INVALID, ppage: 0, flags: 0 };

/// Software TLB for Sv39, direct-mapped by virtual page number. Superpages are entered
/// one 4 KiB page at a time. Flushed on SFENCE.VMA, satp writes, and PMP writes.
#[derive(Clone, Debug)]
pub struct Mmu {
    tlb: Box<[TlbEntry; TLB_SIZE]>,
}

impl Default for Mmu {
    fn default() -> Self {
        Mmu::new()
    }
}

impl Mmu {
    pub fn new() -> Self {
        Mmu { tlb: Box::new([EMPTY; TLB_SIZE]) }
    }

    pub fn flush(&mut self) {
        self.tlb.fill(EMPTY);
    }
}

/// The privilege mode used for translation and PMP checks of this access.
#[inline]
pub fn effective_mode(cpu: &Cpu, access: Access) -> Priv {
    if access != Access::Fetch && cpu.csr.mstatus & csr::MSTATUS_MPRV != 0 {
        Priv::from_bits(cpu.csr.mstatus >> csr::MSTATUS_MPP_SHIFT)
    } else {
        cpu.mode
    }
}

/// Whether a leaf PTE with `flags` permits `access` in `mode`.
#[inline]
fn leaf_allows(flags: u64, access: Access, mode: Priv, mstatus: u64) -> bool {
    if mode == Priv::User {
        if flags & PTE_U == 0 {
            return false;
        }
    } else if flags & PTE_U != 0 && (access == Access::Fetch || mstatus & csr::MSTATUS_SUM == 0) {
        return false;
    }
    let ok = match access {
        Access::Fetch => flags & PTE_X != 0,
        Access::Load => flags & PTE_R != 0 || (mstatus & csr::MSTATUS_MXR != 0 && flags & PTE_X != 0),
        Access::Store => flags & PTE_W != 0,
    };
    // Svade: no hardware A/D updates.
    ok && flags & PTE_A != 0 && (access != Access::Store || flags & PTE_D != 0)
}

/// Translate `vaddr` for an access of `size` bytes that does not cross a page boundary.
#[inline]
pub fn translate(cpu: &mut Cpu, bus: &mut Bus, vaddr: u64, size: u8, access: Access) -> Result<u64, Exception> {
    let mode = effective_mode(cpu, access);
    let paddr = if mode == Priv::Machine || cpu.csr.satp_mode() != csr::SATP_MODE_SV39 {
        vaddr
    } else {
        let vpn = vaddr >> 12;
        let e = cpu.mmu.tlb[vpn as usize % TLB_SIZE];
        if e.vpn == vpn && leaf_allows(e.flags, access, mode, cpu.csr.mstatus) {
            e.ppage | (vaddr & 0xfff)
        } else {
            walk(cpu, bus, vaddr, access, mode)?
        }
    };
    if !cpu.pmp.check(paddr, size as u64, access, mode) {
        return Err(access.access_fault(vaddr));
    }
    Ok(paddr)
}

/// Sv39 page-table walk. Fills the TLB on success.
fn walk(cpu: &mut Cpu, bus: &mut Bus, vaddr: u64, access: Access, mode: Priv) -> Result<u64, Exception> {
    // Bits 63:39 must all equal bit 38.
    if ((vaddr as i64) << 25 >> 25) as u64 != vaddr {
        return Err(access.page_fault(vaddr));
    }
    let mut table = (cpu.csr.satp & PTE_PPN_MASK) << 12;
    let mut level: i32 = 2;
    let pte = loop {
        let vpn_i = (vaddr >> (12 + 9 * level as u64)) & 0x1ff;
        let pte_addr = table + vpn_i * 8;
        // Implicit page-table accesses are checked by PMP as S-mode loads.
        if !cpu.pmp.check(pte_addr, 8, Access::Load, Priv::Supervisor) {
            return Err(access.access_fault(vaddr));
        }
        let pte = bus.read(pte_addr, 8).map_err(|_| access.access_fault(vaddr))?;
        if pte & PTE_V == 0 || (pte & PTE_R == 0 && pte & PTE_W != 0) || pte & PTE_RESERVED != 0 {
            return Err(access.page_fault(vaddr));
        }
        if pte & (PTE_R | PTE_X) != 0 {
            break pte;
        }
        // Non-leaf: A, D, U are reserved and must be zero.
        if pte & (PTE_A | PTE_D | PTE_U) != 0 {
            return Err(access.page_fault(vaddr));
        }
        level -= 1;
        if level < 0 {
            return Err(access.page_fault(vaddr));
        }
        table = ((pte >> 10) & PTE_PPN_MASK) << 12;
    };
    let ppn = (pte >> 10) & PTE_PPN_MASK;
    // Superpages must be aligned.
    let low_ppn_bits = 9 * level as u64;
    if ppn & ((1 << low_ppn_bits) - 1) != 0 {
        return Err(access.page_fault(vaddr));
    }
    let flags = pte & 0xff;
    if !leaf_allows(flags, access, mode, cpu.csr.mstatus) {
        return Err(access.page_fault(vaddr));
    }
    let page_mask = (1u64 << (12 + low_ppn_bits)) - 1;
    let paddr = (ppn << 12) | (vaddr & page_mask);
    let vpn = vaddr >> 12;
    cpu.mmu.tlb[vpn as usize % TLB_SIZE] = TlbEntry { vpn, ppage: paddr & !0xfff, flags };
    Ok(paddr)
}
