//! Signals shared between the hart and platform devices.
//!
//! Devices never hold references to the CPU or to each other. Instead every device
//! access and tick receives `&mut Wires`, the small set of shared lines a board needs:
//! hart interrupt lines, PLIC source levels, the machine timer, and halt requests.

/// Bit masks in the `mip`/`mie` layout.
pub mod bits {
    pub const SSIP: u64 = 1 << 1;
    pub const MSIP: u64 = 1 << 3;
    pub const STIP: u64 = 1 << 5;
    pub const MTIP: u64 = 1 << 7;
    pub const SEIP: u64 = 1 << 9;
    pub const MEIP: u64 = 1 << 11;
}

/// Lines that devices drive into the hart. Only these bits of [`Wires::hart_irq`] are honored.
pub const HART_IRQ_MASK: u64 = bits::MSIP | bits::MTIP | bits::SEIP | bits::MEIP;

/// Why the machine asked to stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Halt {
    /// Syscon power-off.
    PowerOff,
    /// Syscon reboot.
    Reboot,
    /// Test harness exit (HTIF `tohost` or a test device). 0 means pass.
    TestExit(u64),
}

#[derive(Clone, Debug, Default)]
pub struct Wires {
    /// Hart interrupt lines driven by devices, in `mip` bit layout. CLINT drives MSIP and
    /// MTIP; the PLIC drives MEIP and SEIP. The CPU ORs these into `mip` on every read.
    pub hart_irq: u64,
    /// Interrupt source levels into the PLIC: bit `n` is source `n` (1..=63). Bit 0 is unused.
    pub irq_sources: u64,
    /// The machine timer (CLINT `mtime`), in timebase ticks. The `time` CSR reads this.
    /// The board's run loop advances it; the CLINT may also write it.
    pub mtime: u64,
    /// Set by a device to make the run loop stop.
    pub halt: Option<Halt>,
}

impl Wires {
    /// Raise or lower hart interrupt lines (`mask` in `mip` layout).
    #[inline]
    pub fn set_hart_irq(&mut self, mask: u64, level: bool) {
        if level {
            self.hart_irq |= mask & HART_IRQ_MASK;
        } else {
            self.hart_irq &= !mask;
        }
    }

    /// Raise or lower PLIC interrupt source `source` (1..=63).
    #[inline]
    pub fn set_irq_source(&mut self, source: u32, level: bool) {
        debug_assert!((1..64).contains(&source));
        let bit = 1u64 << source;
        if level {
            self.irq_sources |= bit;
        } else {
            self.irq_sources &= !bit;
        }
    }

    #[inline]
    pub fn irq_source(&self, source: u32) -> bool {
        self.irq_sources & (1u64 << source) != 0
    }
}
