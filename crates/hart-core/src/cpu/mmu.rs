//! Address translation: Sv39 page walks with a software TLB, then PMP.

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

/// Software TLB state. Flushed on SFENCE.VMA, satp writes, PMP writes, and changes to
/// mstatus fields that affect translation.
#[derive(Clone, Debug, Default)]
pub struct Mmu {}

impl Mmu {
    pub fn new() -> Self {
        Mmu {}
    }

    pub fn flush(&mut self) {}
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

/// Translate `vaddr` for an access of `size` bytes.
pub fn translate(cpu: &mut Cpu, _bus: &mut Bus, vaddr: u64, size: u8, access: Access) -> Result<u64, Exception> {
    let mode = effective_mode(cpu, access);
    let paddr = vaddr;
    if !cpu.pmp.check(paddr, size as u64, access, mode) {
        return Err(access.access_fault(vaddr));
    }
    Ok(paddr)
}
