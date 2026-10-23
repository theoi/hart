//! Address translation: Sv39 page walks, PMP, and software TLBs.
//!
//! There is one direct-mapped TLB per access type (fetch, load, store). An entry maps a
//! virtual page, tagged with the translation context (effective privilege, SUM, MXR), to
//! the byte offset of a RAM page. Entries are only created for RAM pages whose whole
//! 4 KiB is permitted by PMP for that access, so a hit needs no further checks.
//! Everything else (MMIO, partial PMP coverage, misses) takes the slow path.
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

pub const TLB_SIZE: usize = 1024;
const INVALID: u64 = u64::MAX;
/// Context bits sit above the 52-bit virtual page number in a tag.
const CTX_SHIFT: u64 = 52;

#[derive(Clone, Copy, Debug)]
pub struct TlbEntry {
    /// `vpn | ctx`, or `INVALID`.
    pub tag: u64,
    /// Byte offset of the physical page in RAM.
    pub off: u64,
}

const EMPTY: TlbEntry = TlbEntry { tag: INVALID, off: 0 };

#[derive(Clone, Debug)]
pub struct Mmu {
    pub fetch: Box<[TlbEntry]>,
    pub load: Box<[TlbEntry]>,
    pub store: Box<[TlbEntry]>,
    /// Tag context for fetches (current mode).
    pub fetch_ctx: u64,
    /// Tag context for loads and stores (effective mode, SUM, MXR).
    pub data_ctx: u64,
}

impl Default for Mmu {
    fn default() -> Self {
        Mmu::new()
    }
}

impl Mmu {
    pub fn new() -> Self {
        let t = || vec![EMPTY; TLB_SIZE].into_boxed_slice();
        Mmu { fetch: t(), load: t(), store: t(), fetch_ctx: 3 << CTX_SHIFT, data_ctx: 3 << CTX_SHIFT }
    }

    /// Drop every cached translation (SFENCE.VMA, satp and PMP writes).
    pub fn flush(&mut self) {
        self.fetch.fill(EMPTY);
        self.load.fill(EMPTY);
        self.store.fill(EMPTY);
    }

    /// RAM offset of the page holding `vaddr`, if the TLB for this access type has it.
    #[inline(always)]
    pub fn lookup(&self, vaddr: u64, access: Access) -> Option<u64> {
        let vpn = vaddr >> 12;
        let (tlb, ctx) = match access {
            Access::Fetch => (&self.fetch, self.fetch_ctx),
            Access::Load => (&self.load, self.data_ctx),
            Access::Store => (&self.store, self.data_ctx),
        };
        let e = &tlb[vpn as usize & (TLB_SIZE - 1)];
        if e.tag == vpn | ctx {
            Some(e.off)
        } else {
            None
        }
    }

    fn insert(&mut self, vaddr: u64, access: Access, off: u64) {
        let vpn = vaddr >> 12;
        let (tlb, ctx) = match access {
            Access::Fetch => (&mut self.fetch, self.fetch_ctx),
            Access::Load => (&mut self.load, self.data_ctx),
            Access::Store => (&mut self.store, self.data_ctx),
        };
        tlb[vpn as usize & (TLB_SIZE - 1)] = TlbEntry { tag: vpn | ctx, off };
    }
}

/// Recompute the TLB tag contexts. Must be called after any change to the privilege
/// mode or to `mstatus` MPRV, MPP, SUM, or MXR.
#[inline]
pub fn update_ctx(cpu: &mut Cpu) {
    let m = cpu.csr.mstatus;
    let eff = if m & csr::MSTATUS_MPRV != 0 { (m >> csr::MSTATUS_MPP_SHIFT) & 3 } else { cpu.mode as u64 };
    let sum = (m & csr::MSTATUS_SUM != 0) as u64;
    let mxr = (m & csr::MSTATUS_MXR != 0) as u64;
    cpu.mmu.fetch_ctx = (cpu.mode as u64) << CTX_SHIFT;
    cpu.mmu.data_ctx = (eff | (sum << 2) | (mxr << 3)) << CTX_SHIFT;
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
    if let Some(off) = cpu.mmu.lookup(vaddr, access) {
        return Ok(bus.ram.base() + off + (vaddr & 0xfff));
    }
    translate_slow(cpu, bus, vaddr, size, access)
}

#[cold]
fn translate_slow(cpu: &mut Cpu, bus: &mut Bus, vaddr: u64, size: u8, access: Access) -> Result<u64, Exception> {
    let mode = effective_mode(cpu, access);
    let paddr = if mode == Priv::Machine || cpu.csr.satp_mode() != csr::SATP_MODE_SV39 {
        vaddr
    } else {
        walk(cpu, bus, vaddr, access, mode)?
    };
    if !cpu.pmp.check(paddr, size as u64, access, mode) {
        return Err(access.access_fault(vaddr));
    }
    let ppage = paddr & !0xfff;
    if let Some(off) = bus.ram.offset(ppage, 4096) {
        if cpu.pmp.check(ppage, 4096, access, mode) {
            cpu.mmu.insert(vaddr, access, off as u64);
        }
    }
    Ok(paddr)
}

/// Sv39 page-table walk.
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
    if !leaf_allows(pte & 0xff, access, mode, cpu.csr.mstatus) {
        return Err(access.page_fault(vaddr));
    }
    let page_mask = (1u64 << (12 + low_ppn_bits)) - 1;
    Ok((ppn << 12) | (vaddr & page_mask))
}
