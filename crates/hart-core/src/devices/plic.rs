//! SiFive-compatible platform-level interrupt controller (PLIC) with two
//! contexts: hart 0 M-mode (context 0) and hart 0 S-mode (context 1), as on
//! QEMU virt (`interrupts-extended = <&intc 11 &intc 9>`).
//!
//! Sources are level-triggered. A source's pending bit follows its input
//! level while it is not claimed; claiming clears it, and completing re-arms
//! the gateway so a still-high input becomes pending again.
//!
//! Input levels come from [`Wires::irq_sources`]; the PLIC samples them on
//! each of its own accesses and on [`Device::tick`], and drives MEIP and
//! SEIP into [`Wires::hart_irq`].

use crate::bus::{BusFault, Device};
use crate::irq::{bits, Wires};

pub const PLIC_BASE: u64 = 0x0c00_0000;
pub const PLIC_SIZE: u64 = 0x60_0000;

/// Number of sources, excluding the reserved source 0.
pub const PLIC_NDEV: u32 = 31;
/// Priorities are 3 bits wide (0..=7), as on QEMU virt.
pub const PLIC_PRIORITY_BITS: u32 = 3;
pub const NUM_CONTEXTS: usize = 2;
pub const CONTEXT_M: usize = 0;
pub const CONTEXT_S: usize = 1;

const PRIORITY_END: u64 = 0x1000;
const PENDING_BASE: u64 = 0x1000;
const PENDING_END: u64 = 0x1080;
const ENABLE_BASE: u64 = 0x2000;
const ENABLE_STRIDE: u64 = 0x80;
const ENABLE_END: u64 = 0x20_0000;
const CONTEXT_BASE: u64 = 0x20_0000;
const CONTEXT_STRIDE: u64 = 0x1000;

#[derive(Clone, Debug)]
pub struct Plic {
    ndev: u32,
    priority_mask: u32,
    priority: [u32; 64],
    level: u64,
    pending: u64,
    claimed: u64,
    enable: [u64; NUM_CONTEXTS],
    threshold: [u32; NUM_CONTEXTS],
}

impl Default for Plic {
    fn default() -> Self {
        Self::new(PLIC_NDEV, PLIC_PRIORITY_BITS)
    }
}

impl Plic {
    /// `ndev` must be in 1..=63 and `priority_bits` in 1..=32.
    pub fn new(ndev: u32, priority_bits: u32) -> Self {
        assert!((1..=63).contains(&ndev), "PLIC supports 1..=63 sources");
        assert!((1..=32).contains(&priority_bits));
        Self {
            ndev,
            priority_mask: if priority_bits == 32 { u32::MAX } else { (1 << priority_bits) - 1 },
            priority: [0; 64],
            level: 0,
            pending: 0,
            claimed: 0,
            enable: [0; NUM_CONTEXTS],
            threshold: [0; NUM_CONTEXTS],
        }
    }

    pub fn ndev(&self) -> u32 {
        self.ndev
    }

    pub fn max_priority(&self) -> u32 {
        self.priority_mask
    }

    fn valid_mask(&self) -> u64 {
        let all = if self.ndev == 63 { u64::MAX } else { (1u64 << (self.ndev + 1)) - 1 };
        all & !1
    }

    /// Drives the input line of `source` (1..=ndev). Other sources are ignored.
    pub fn set_irq(&mut self, source: u32, level: bool) {
        if source == 0 || source > self.ndev {
            return;
        }
        let bit = 1u64 << source;
        if level {
            self.level |= bit;
            if self.claimed & bit == 0 {
                self.pending |= bit;
            }
        } else {
            self.level &= !bit;
            self.pending &= !bit;
        }
    }

    /// Whether `context` has an enabled pending source above its threshold.
    pub fn interrupt_pending(&self, context: usize) -> bool {
        self.best(context).is_some()
    }

    /// Machine external interrupt pending (mip.MEIP).
    pub fn meip(&self) -> bool {
        self.interrupt_pending(CONTEXT_M)
    }

    /// Supervisor external interrupt pending (the hardware part of mip.SEIP).
    pub fn seip(&self) -> bool {
        self.interrupt_pending(CONTEXT_S)
    }

    fn best(&self, context: usize) -> Option<u32> {
        let mut candidates = self.pending & self.enable[context];
        let mut best = None;
        let mut best_priority = self.threshold[context];
        while candidates != 0 {
            let source = candidates.trailing_zeros();
            candidates &= candidates - 1;
            // Strictly greater: ties go to the lowest source ID.
            if self.priority[source as usize] > best_priority {
                best_priority = self.priority[source as usize];
                best = Some(source);
            }
        }
        best
    }

    fn claim(&mut self, context: usize) -> u32 {
        match self.best(context) {
            Some(source) => {
                let bit = 1u64 << source;
                self.pending &= !bit;
                self.claimed |= bit;
                source
            }
            None => 0,
        }
    }

    fn complete(&mut self, context: usize, source: u32) {
        if source == 0 || source > self.ndev {
            return;
        }
        let bit = 1u64 << source;
        // The spec ignores completions for sources not enabled in the context.
        if self.enable[context] & bit == 0 {
            return;
        }
        self.claimed &= !bit;
        if self.level & bit != 0 {
            self.pending |= bit;
        }
    }

    fn read32(&mut self, offset: u64) -> u32 {
        match offset {
            0..PRIORITY_END => {
                let source = (offset / 4) as u32;
                if source >= 1 && source <= self.ndev {
                    self.priority[source as usize]
                } else {
                    0
                }
            }
            PENDING_BASE..PENDING_END => {
                let word = (offset - PENDING_BASE) / 4;
                if word < 2 {
                    (self.pending >> (32 * word)) as u32
                } else {
                    0
                }
            }
            ENABLE_BASE..ENABLE_END => {
                let context = ((offset - ENABLE_BASE) / ENABLE_STRIDE) as usize;
                let word = ((offset - ENABLE_BASE) % ENABLE_STRIDE) / 4;
                if context < NUM_CONTEXTS && word < 2 {
                    (self.enable[context] >> (32 * word)) as u32
                } else {
                    0
                }
            }
            CONTEXT_BASE..PLIC_SIZE => {
                let context = ((offset - CONTEXT_BASE) / CONTEXT_STRIDE) as usize;
                if context >= NUM_CONTEXTS {
                    return 0;
                }
                match (offset - CONTEXT_BASE) % CONTEXT_STRIDE {
                    0 => self.threshold[context],
                    4 => self.claim(context),
                    _ => 0,
                }
            }
            _ => 0,
        }
    }

    fn write32(&mut self, offset: u64, value: u32) {
        match offset {
            0..PRIORITY_END => {
                let source = (offset / 4) as u32;
                if source >= 1 && source <= self.ndev {
                    self.priority[source as usize] = value & self.priority_mask;
                }
            }
            ENABLE_BASE..ENABLE_END => {
                let context = ((offset - ENABLE_BASE) / ENABLE_STRIDE) as usize;
                let word = ((offset - ENABLE_BASE) % ENABLE_STRIDE) / 4;
                if context < NUM_CONTEXTS && word < 2 {
                    let shift = 32 * word;
                    let valid = self.valid_mask();
                    let e = &mut self.enable[context];
                    *e = ((*e & !(0xffff_ffffu64 << shift)) | (u64::from(value) << shift)) & valid;
                }
            }
            CONTEXT_BASE..PLIC_SIZE => {
                let context = ((offset - CONTEXT_BASE) / CONTEXT_STRIDE) as usize;
                if context >= NUM_CONTEXTS {
                    return;
                }
                match (offset - CONTEXT_BASE) % CONTEXT_STRIDE {
                    0 => self.threshold[context] = value & self.priority_mask,
                    4 => self.complete(context, value),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Samples the source levels from `wires`.
    pub fn sample(&mut self, wires: &Wires) {
        let levels = wires.irq_sources & self.valid_mask();
        let mut changed = levels ^ self.level;
        while changed != 0 {
            let source = changed.trailing_zeros();
            changed &= changed - 1;
            self.set_irq(source, levels & (1 << source) != 0);
        }
    }

    /// Drives MEIP and SEIP into `wires`.
    pub fn drive(&self, wires: &mut Wires) {
        wires.set_hart_irq(bits::MEIP, self.meip());
        wires.set_hart_irq(bits::SEIP, self.seip());
    }

    /// Register read without sampling inputs. Registers are 32 bits wide; an
    /// 8-byte access covers two of them. Narrower accesses read part of a
    /// register (and still claim).
    pub fn read_reg(&mut self, offset: u64, size: u8) -> u64 {
        let aligned = offset & !3;
        match size {
            8 if offset & 7 == 0 => u64::from(self.read32(offset)) | (u64::from(self.read32(offset + 4)) << 32),
            _ => {
                let word = u64::from(self.read32(aligned));
                super::reg_read(word, offset - aligned, size.min(4))
            }
        }
    }

    /// Register write without sampling inputs. 4- and 8-byte accesses are
    /// supported; narrower writes are ignored.
    pub fn write_reg(&mut self, offset: u64, size: u8, value: u64) {
        match size {
            8 if offset & 7 == 0 => {
                self.write32(offset, value as u32);
                self.write32(offset + 4, (value >> 32) as u32);
            }
            4 if offset & 3 == 0 => self.write32(offset, value as u32),
            _ => {}
        }
    }
}

impl Device for Plic {
    fn read(&mut self, offset: u64, size: u8, wires: &mut Wires) -> Result<u64, BusFault> {
        self.sample(wires);
        let v = self.read_reg(offset, size);
        self.drive(wires);
        Ok(v)
    }

    fn write(&mut self, offset: u64, size: u8, value: u64, wires: &mut Wires) -> Result<(), BusFault> {
        self.sample(wires);
        self.write_reg(offset, size, value);
        self.drive(wires);
        Ok(())
    }

    fn tick(&mut self, wires: &mut Wires) {
        self.sample(wires);
        self.drive(wires);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRIO: u64 = 0;
    const ENABLE_S: u64 = ENABLE_BASE + ENABLE_STRIDE;
    const THRESHOLD_M: u64 = CONTEXT_BASE;
    const CLAIM_M: u64 = CONTEXT_BASE + 4;
    const THRESHOLD_S: u64 = CONTEXT_BASE + CONTEXT_STRIDE;
    const CLAIM_S: u64 = CONTEXT_BASE + CONTEXT_STRIDE + 4;

    fn plic_with_s_source(source: u32, priority: u64) -> Plic {
        let mut p = Plic::default();
        p.write_reg(PRIO + 4 * u64::from(source), 4, priority);
        p.write_reg(ENABLE_S, 4, 1 << source);
        p
    }

    #[test]
    fn claim_complete_cycle_for_level_source() {
        let mut p = plic_with_s_source(10, 1);
        assert!(!p.seip());
        p.set_irq(10, true);
        assert!(p.seip());
        assert!(!p.meip(), "not enabled in M context");
        assert_eq!(p.read_reg(PENDING_BASE, 4), 1 << 10);
        assert_eq!(p.read_reg(CLAIM_S, 4), 10);
        assert!(!p.seip(), "claimed source is no longer pending");
        assert_eq!(p.read_reg(CLAIM_S, 4), 0, "nothing else to claim");
        // Still high at completion: pending again.
        p.write_reg(CLAIM_S, 4, 10);
        assert!(p.seip());
        assert_eq!(p.read_reg(CLAIM_S, 4), 10);
        p.set_irq(10, false);
        p.write_reg(CLAIM_S, 4, 10);
        assert!(!p.seip());
    }

    #[test]
    fn level_drop_before_claim_clears_pending() {
        let mut p = plic_with_s_source(3, 2);
        p.set_irq(3, true);
        p.set_irq(3, false);
        assert!(!p.seip());
        assert_eq!(p.read_reg(CLAIM_S, 4), 0);
    }

    #[test]
    fn input_while_claimed_waits_for_completion() {
        let mut p = plic_with_s_source(5, 1);
        p.set_irq(5, true);
        assert_eq!(p.read_reg(CLAIM_S, 4), 5);
        p.set_irq(5, false);
        p.set_irq(5, true);
        assert!(!p.seip(), "gateway closed until completion");
        p.write_reg(CLAIM_S, 4, 5);
        assert!(p.seip());
    }

    #[test]
    fn priority_threshold_and_tie_break() {
        let mut p = Plic::default();
        for (source, prio) in [(1u64, 2u64), (2, 5), (3, 5), (4, 0)] {
            p.write_reg(PRIO + 4 * source, 4, prio);
        }
        p.write_reg(ENABLE_BASE, 4, 0b11110);
        for s in 1..=4 {
            p.set_irq(s, true);
        }
        p.write_reg(THRESHOLD_M, 4, 1);
        assert_eq!(p.read_reg(CLAIM_M, 4), 2, "highest priority, lowest ID");
        assert_eq!(p.read_reg(CLAIM_M, 4), 3);
        assert_eq!(p.read_reg(CLAIM_M, 4), 1);
        assert_eq!(p.read_reg(CLAIM_M, 4), 0, "priority 0 never interrupts");
        p.write_reg(CLAIM_M, 4, 1);
        p.write_reg(THRESHOLD_M, 4, 2);
        assert!(!p.meip(), "priority must exceed the threshold");
        p.write_reg(THRESHOLD_M, 4, 1);
        assert!(p.meip());
    }

    #[test]
    fn completion_of_disabled_source_is_ignored() {
        let mut p = plic_with_s_source(7, 1);
        p.set_irq(7, true);
        assert_eq!(p.read_reg(CLAIM_S, 4), 7);
        p.write_reg(ENABLE_S, 4, 0);
        p.write_reg(CLAIM_S, 4, 7);
        p.write_reg(ENABLE_S, 4, 1 << 7);
        assert!(!p.seip(), "still claimed");
        p.write_reg(CLAIM_S, 4, 7);
        assert!(p.seip());
    }

    #[test]
    fn device_samples_wires_and_drives_seip() {
        let mut p = plic_with_s_source(10, 1);
        let mut w = Wires::default();
        w.set_irq_source(10, true);
        p.tick(&mut w);
        assert_eq!(w.hart_irq & bits::SEIP, bits::SEIP);
        assert_eq!(w.hart_irq & bits::MEIP, 0);
        assert_eq!(p.read(CLAIM_S, 4, &mut w), Ok(10));
        assert_eq!(w.hart_irq & bits::SEIP, 0, "claim drops SEIP");
        w.set_irq_source(10, false);
        p.write(CLAIM_S, 4, 10, &mut w).unwrap();
        assert_eq!(w.hart_irq & bits::SEIP, 0);
        w.set_irq_source(10, true);
        p.tick(&mut w);
        assert_eq!(w.hart_irq & bits::SEIP, bits::SEIP);
    }

    #[test]
    fn register_masks() {
        let mut p = Plic::default();
        p.write_reg(PRIO + 4, 4, 0xffff_ffff);
        assert_eq!(p.read_reg(PRIO + 4, 4), 7);
        p.write_reg(PRIO, 4, 5);
        assert_eq!(p.read_reg(PRIO, 4), 0, "source 0 is reserved");
        p.write_reg(ENABLE_S, 4, 0xffff_ffff);
        assert_eq!(p.read_reg(ENABLE_S, 4), 0xffff_fffe);
        assert_eq!(p.read_reg(ENABLE_S + 4, 4), 0, "beyond ndev");
        p.write_reg(THRESHOLD_S, 4, 0xff);
        assert_eq!(p.read_reg(THRESHOLD_S, 4), 7);
        p.write_reg(PENDING_BASE, 4, 0xffff);
        assert_eq!(p.read_reg(PENDING_BASE, 4), 0, "pending is read-only");
        p.write_reg(CONTEXT_BASE + 2 * CONTEXT_STRIDE, 4, 3);
        assert_eq!(p.read_reg(CONTEXT_BASE + 2 * CONTEXT_STRIDE, 4), 0);
    }
}
