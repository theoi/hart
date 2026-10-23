//! Spike-compatible machine profile for instruction-by-instruction lockstep
//! against the Spike reference simulator (riscv-isa-sim 2c300cd).
//!
//! The profile reproduces what Spike shows the guest:
//!
//! - a read-only reset ROM at 0x1000 (Spike's reset vector, the start pc at
//!   0x1018, and the device tree at 0x1020); the hart starts at 0x1000 in
//!   M-mode with all registers zero;
//! - RAM at 0x8000_0000, OpenSBI loaded from its ELF (HTIF at its
//!   `tohost`/`fromhost` symbols), the kernel at 0x8020_0000;
//! - Spike's CLINT (0x0200_0000, size 0xc000), PLIC (0x0c00_0000, 31
//!   sources, 4 priority bits, one context per privilege mode) and ns16550
//!   (0x1000_0000, PLIC source 1), with Spike's register semantics;
//! - Spike's time and interrupt model: the hart runs in chunks of up to 5000
//!   instructions (`sim_t::INTERLEAVE`), cut short by a trap, a retired WFI,
//!   or a stalled WFI; after every chunk the load reservation is dropped,
//!   `mtime` advances by 50 ticks, and the devices tick. Interrupts are
//!   polled at chunk start and around serializing instructions
//!   ([`crate::cpu::CpuConfig::spike`]). HTIF is polled between chunks, and
//!   an iteration that finds a `tohost` command runs no chunk, as in
//!   `htif_t::run`.
//!
//! The PLIC and UART share one device so that a UART level change reaches
//! the PLIC (and `mip`) within the same access, as Spike's direct
//! `set_interrupt_level` call does.

use crate::bus::{BusFault, Device};
use crate::irq::{bits, Wires};
use std::collections::VecDeque;

pub const SPIKE_ROM_BASE: u64 = 0x1000;
pub const SPIKE_CLINT_SIZE: u64 = 0xc000;
pub const SPIKE_PLIC_BASE: u64 = 0x0c00_0000;
pub const SPIKE_PLIC_SIZE: u64 = 0x100_0000;
pub const SPIKE_UART_BASE: u64 = 0x1000_0000;
pub const SPIKE_UART_SIZE: u64 = 0x100;
pub const SPIKE_UART_IRQ: u32 = 1;
/// Instructions per chunk (`sim_t::INTERLEAVE`).
pub const INTERLEAVE: u64 = 5000;
/// `mtime` ticks per chunk (`INTERLEAVE / INSNS_PER_RTC_TICK`).
pub const TICKS_PER_CHUNK: u64 = INTERLEAVE / 100;

/// Spike's reset ROM: reset vector, start pc, then the device tree, padded
/// to a 4 KiB multiple.
#[derive(Clone, Debug)]
pub struct BootRom {
    data: Vec<u8>,
}

impl BootRom {
    pub fn spike(start_pc: u64, dtb: &[u8]) -> Self {
        let words: [u32; 6] = [
            0x0000_0297, // auipc t0, 0
            0x0202_8593, // addi a1, t0, 32 (the device tree)
            0xf140_2573, // csrr a0, mhartid
            0x0182_b283, // ld t0, 24(t0) (the start pc)
            0x0002_8067, // jr t0
            0,
        ];
        let mut data: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        data.extend_from_slice(&start_pc.to_le_bytes());
        data.extend_from_slice(dtb);
        data.resize(data.len().div_ceil(0x1000) * 0x1000, 0);
        Self { data }
    }

    pub fn size(&self) -> u64 {
        self.data.len() as u64
    }
}

impl Device for BootRom {
    fn read(&mut self, offset: u64, size: u8, _wires: &mut Wires) -> Result<u64, BusFault> {
        let start = offset as usize;
        let bytes = self.data.get(start..start + usize::from(size)).ok_or(BusFault)?;
        let mut buf = [0u8; 8];
        buf[..bytes.len()].copy_from_slice(bytes);
        Ok(u64::from_le_bytes(buf))
    }

    // Stores fault, as rom_device_t::store fails in Spike.
    fn write(&mut self, _offset: u64, _size: u8, _value: u64, _wires: &mut Wires) -> Result<(), BusFault> {
        Err(BusFault)
    }

    // Spike's rom_device_t::load copies any length from any offset, and Spike sends
    // a misaligned load within a page to the device whole.
    fn any_alignment(&self) -> bool {
        true
    }
}

const PLIC_NUM_IDS: u32 = 32;
const PLIC_PRIO_MASK: u32 = 0xf;

#[derive(Clone, Debug, Default)]
struct PlicContext {
    mmode: bool,
    enable: u32,
    pending: u32,
    claimed: u32,
    pending_priority: [u8; 32],
    threshold: u8,
}

/// Spike's `plic_t` for one hart with M and S contexts (`riscv/plic.cc`).
#[derive(Clone, Debug)]
struct SpikePlic {
    priority: [u8; 32],
    level: u32,
    contexts: [PlicContext; 2],
}

impl SpikePlic {
    fn new() -> Self {
        let m = PlicContext { mmode: true, ..PlicContext::default() };
        Self { priority: [0; 32], level: 0, contexts: [m, PlicContext::default()] }
    }

    fn best_pending(c: &PlicContext) -> u32 {
        let mut best_id = 0;
        let mut best_prio = 0u8;
        for id in 0..PLIC_NUM_IDS {
            let bit = 1u32 << id;
            if c.pending & bit == 0 || c.claimed & bit != 0 {
                continue;
            }
            if best_id == 0 || best_prio < c.pending_priority[id as usize] {
                best_id = id;
                best_prio = c.pending_priority[id as usize];
            }
        }
        if best_prio <= c.threshold {
            0
        } else {
            best_id
        }
    }

    fn drive(&self, wires: &mut Wires) {
        for c in &self.contexts {
            let line = if c.mmode { bits::MEIP } else { bits::SEIP };
            wires.set_hart_irq(line, Self::best_pending(c) != 0);
        }
    }

    fn set_interrupt_level(&mut self, id: u32, level: bool) {
        if id == 0 || id >= PLIC_NUM_IDS {
            return;
        }
        let bit = 1u32 << id;
        if level {
            self.level |= bit;
        } else {
            self.level &= !bit;
        }
        let prio = self.priority[id as usize];
        // Spike delivers the source to the first context that enables it.
        if let Some(c) = self.contexts.iter_mut().find(|c| c.enable & bit != 0) {
            if level {
                c.pending |= bit;
                c.pending_priority[id as usize] = prio;
            } else {
                c.pending &= !bit;
                c.pending_priority[id as usize] = 0;
                c.claimed &= !bit;
            }
        }
    }

    fn read32(&mut self, offset: u64) -> Result<u32, BusFault> {
        match offset {
            0..0x1000 => {
                let id = (offset >> 2) as u32;
                Ok(if id > 0 && id < PLIC_NUM_IDS { u32::from(self.priority[id as usize]) } else { 0 })
            }
            0x1000..0x2000 => {
                Ok(if offset - 0x1000 < 4 { self.contexts.iter().fold(0, |acc, c| acc | c.pending) } else { 0 })
            }
            0x2000..0x20_0000 => {
                let cntx = ((offset - 0x2000) / 0x80) as usize;
                let off = (offset - 0x2000) % 0x80;
                let c = self.contexts.get(cntx).ok_or(BusFault)?;
                Ok(if off < 4 { c.enable } else { 0 })
            }
            _ => {
                let cntx = ((offset - 0x20_0000) / 0x1000) as usize;
                let off = (offset - 0x20_0000) % 0x1000;
                let c = self.contexts.get_mut(cntx).ok_or(BusFault)?;
                Ok(match off {
                    0 => u32::from(c.threshold),
                    4 => {
                        let best = Self::best_pending(c);
                        if best != 0 {
                            c.claimed |= 1 << best;
                        }
                        best
                    }
                    _ => 0,
                })
            }
        }
    }

    fn write32(&mut self, offset: u64, value: u32) -> Result<(), BusFault> {
        match offset {
            0..0x2000 => {
                let id = (offset >> 2) as u32;
                if id > 0 && id < PLIC_NUM_IDS {
                    self.priority[id as usize] = (value & PLIC_PRIO_MASK) as u8;
                }
                Ok(())
            }
            0x2000..0x20_0000 => {
                let cntx = ((offset - 0x2000) / 0x80) as usize;
                let off = (offset - 0x2000) % 0x80;
                let level = self.level;
                let priority = self.priority;
                let c = self.contexts.get_mut(cntx).ok_or(BusFault)?;
                if off >= 4 {
                    return Ok(());
                }
                let new = value & !1;
                let changed = c.enable ^ new;
                c.enable = new;
                for (id, &prio) in priority.iter().enumerate() {
                    let bit = 1u32 << id;
                    if changed & bit == 0 {
                        continue;
                    }
                    if new & bit != 0 && level & bit != 0 {
                        c.pending |= bit;
                        c.pending_priority[id] = prio;
                    } else if new & bit == 0 {
                        c.pending &= !bit;
                        c.pending_priority[id] = 0;
                        c.claimed &= !bit;
                    }
                }
                Ok(())
            }
            _ => {
                let cntx = ((offset - 0x20_0000) / 0x1000) as usize;
                let off = (offset - 0x20_0000) % 0x1000;
                let c = self.contexts.get_mut(cntx).ok_or(BusFault)?;
                match off {
                    0 => {
                        c.threshold = (value & PLIC_PRIO_MASK) as u8;
                        Ok(())
                    }
                    4 => {
                        if value < PLIC_NUM_IDS && c.enable & (1 << value) != 0 {
                            c.claimed &= !(1 << value);
                        }
                        Ok(())
                    }
                    _ => Err(BusFault),
                }
            }
        }
    }
}

const UART_QUEUE_SIZE: usize = 64;
const IER_RDI: u8 = 0x01;
const IER_THRI: u8 = 0x02;
const IIR_NO_INT: u8 = 0x01;
const IIR_THRI: u8 = 0x02;
const IIR_RDI: u8 = 0x04;
const IIR_TYPE_BITS: u8 = 0xc0;
const FCR_ENABLE_FIFO: u8 = 0x01;
const FCR_CLEAR_RCVR: u8 = 0x02;
const FCR_CLEAR_XMIT: u8 = 0x04;
const LCR_DLAB: u8 = 0x80;
const MCR_LOOP: u8 = 0x10;
const MCR_OUT2: u8 = 0x08;
const LSR_DR: u8 = 0x01;
const LSR_THRE: u8 = 0x20;
const LSR_TEMT: u8 = 0x40;
const MSR_RESET: u8 = 0x80 | 0x20 | 0x10;

/// Spike's `ns16550_t` (`riscv/ns16550.cc`), with `reg-shift = 0` and
/// `reg-io-width = 1`.
#[derive(Clone, Debug)]
struct SpikeUart {
    ier: u8,
    iir: u8,
    fcr: u8,
    lcr: u8,
    mcr: u8,
    lsr: u8,
    msr: u8,
    scr: u8,
    dll: u8,
    dlm: u8,
    rx: VecDeque<u8>,
    host_input: VecDeque<u8>,
    tx: Vec<u8>,
}

impl SpikeUart {
    fn new() -> Self {
        Self {
            ier: 0,
            iir: IIR_NO_INT,
            fcr: 0,
            lcr: 0,
            mcr: MCR_OUT2,
            lsr: LSR_TEMT | LSR_THRE,
            msr: MSR_RESET,
            scr: 0,
            dll: 0x0c,
            dlm: 0,
            rx: VecDeque::new(),
            host_input: VecDeque::new(),
            tx: Vec::new(),
        }
    }

    /// Returns the interrupt level, which Spike passes to the PLIC on every
    /// update.
    fn update_interrupt(&mut self) -> bool {
        if self.fcr & FCR_CLEAR_RCVR != 0 {
            self.fcr &= !FCR_CLEAR_RCVR;
            self.rx.clear();
            self.lsr &= !LSR_DR;
        }
        if self.fcr & FCR_CLEAR_XMIT != 0 {
            self.fcr &= !FCR_CLEAR_XMIT;
            self.lsr |= LSR_TEMT | LSR_THRE;
        }
        let mut interrupts = 0;
        if self.ier & IER_RDI != 0 && self.lsr & LSR_DR != 0 {
            interrupts |= IIR_RDI;
        }
        if self.ier & IER_THRI != 0 && self.lsr & LSR_TEMT != 0 {
            interrupts |= IIR_THRI;
        }
        self.iir = if interrupts == 0 { IIR_NO_INT } else { interrupts };
        if self.ier & IER_THRI == 0 {
            self.lsr |= LSR_TEMT | LSR_THRE;
        }
        interrupts != 0
    }

    fn rx_byte(&mut self) -> u8 {
        let Some(byte) = self.rx.pop_front() else {
            self.lsr &= !LSR_DR;
            return 0;
        };
        if self.rx.is_empty() {
            self.lsr &= !LSR_DR;
        }
        byte
    }

    /// Returns `Some(level)` when Spike would call `update_interrupt`.
    fn read(&mut self, reg: u64) -> (u8, Option<bool>) {
        let dlab = self.lcr & LCR_DLAB != 0;
        match reg & 7 {
            0 => {
                let v = if dlab { self.dll } else { self.rx_byte() };
                (v, Some(self.update_interrupt()))
            }
            1 => (if dlab { self.dlm } else { self.ier }, None),
            2 => (self.iir | IIR_TYPE_BITS, None),
            3 => (self.lcr, None),
            4 => (self.mcr, None),
            5 => (self.lsr, None),
            6 => (self.msr, None),
            _ => (self.scr, None),
        }
    }

    fn write(&mut self, reg: u64, v: u8) -> Option<bool> {
        let dlab = self.lcr & LCR_DLAB != 0;
        match reg & 7 {
            0 => {
                if dlab {
                    self.dll = v;
                } else if self.mcr & MCR_LOOP != 0 {
                    if self.rx.len() < UART_QUEUE_SIZE {
                        self.rx.push_back(v);
                        self.lsr |= LSR_DR;
                    }
                } else {
                    self.lsr |= LSR_TEMT | LSR_THRE;
                    self.tx.push(v);
                }
            }
            1 => {
                if dlab {
                    self.dlm = v;
                } else {
                    self.ier = v & 0x0f;
                }
            }
            2 => self.fcr = v,
            3 => self.lcr = v,
            4 => self.mcr = v,
            5 | 6 => return None,
            _ => {
                self.scr = v;
                return None;
            }
        }
        Some(self.update_interrupt())
    }

    /// Spike's per-chunk tick: receive one host byte when the FIFO is on.
    fn tick(&mut self) -> Option<bool> {
        if self.fcr & FCR_ENABLE_FIFO == 0 || self.mcr & MCR_LOOP != 0 || self.rx.len() >= UART_QUEUE_SIZE {
            return None;
        }
        let byte = self.host_input.pop_front()?;
        self.rx.push_back(byte);
        self.lsr |= LSR_DR;
        Some(self.update_interrupt())
    }
}

/// Spike's PLIC and ns16550 behind one device, mapped from the PLIC base
/// through the end of the UART.
#[derive(Clone, Debug)]
pub struct SpikeSoc {
    plic: SpikePlic,
    uart: SpikeUart,
}

/// Size of the [`SpikeSoc`] region starting at [`SPIKE_PLIC_BASE`].
pub const SPIKE_SOC_SIZE: u64 = SPIKE_UART_BASE + SPIKE_UART_SIZE - SPIKE_PLIC_BASE;
const UART_OFFSET: u64 = SPIKE_UART_BASE - SPIKE_PLIC_BASE;

impl Default for SpikeSoc {
    fn default() -> Self {
        Self::new()
    }
}

impl SpikeSoc {
    pub fn new() -> Self {
        Self { plic: SpikePlic::new(), uart: SpikeUart::new() }
    }

    pub fn push_input(&mut self, bytes: &[u8]) {
        self.uart.host_input.extend(bytes);
    }

    pub fn drain_output_into(&mut self, out: &mut Vec<u8>) {
        out.append(&mut self.uart.tx);
    }

    fn uart_level(&mut self, level: Option<bool>) {
        if let Some(level) = level {
            self.plic.set_interrupt_level(SPIKE_UART_IRQ, level);
        }
    }
}

impl Device for SpikeSoc {
    fn read(&mut self, offset: u64, size: u8, wires: &mut Wires) -> Result<u64, BusFault> {
        let v = if offset < SPIKE_PLIC_SIZE {
            match size {
                4 => u64::from(self.plic.read32(offset)?),
                8 => {
                    let lo = self.plic.read32(offset)?;
                    let hi = self.plic.read32(offset + 4)?;
                    u64::from(lo) | (u64::from(hi) << 32)
                }
                _ => return Err(BusFault),
            }
        } else if (UART_OFFSET..UART_OFFSET + SPIKE_UART_SIZE).contains(&offset) && size == 1 {
            let (v, level) = self.uart.read(offset - UART_OFFSET);
            self.uart_level(level);
            u64::from(v)
        } else {
            return Err(BusFault);
        };
        self.plic.drive(wires);
        Ok(v)
    }

    fn write(&mut self, offset: u64, size: u8, value: u64, wires: &mut Wires) -> Result<(), BusFault> {
        let result = if offset < SPIKE_PLIC_SIZE {
            match size {
                4 => self.plic.write32(offset, value as u32),
                8 => self
                    .plic
                    .write32(offset, value as u32)
                    .and_then(|()| self.plic.write32(offset + 4, (value >> 32) as u32)),
                _ => Err(BusFault),
            }
        } else if (UART_OFFSET..UART_OFFSET + SPIKE_UART_SIZE).contains(&offset) && size == 1 {
            let level = self.uart.write(offset - UART_OFFSET, value as u8);
            self.uart_level(level);
            Ok(())
        } else {
            Err(BusFault)
        };
        self.plic.drive(wires);
        result
    }

    fn tick(&mut self, wires: &mut Wires) {
        let level = self.uart.tick();
        self.uart_level(level);
        self.plic.drive(wires);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UART: u64 = UART_OFFSET;
    const ENABLE_S: u64 = 0x2080;
    const CLAIM_S: u64 = 0x20_1004;

    #[test]
    fn rom_layout_matches_spike() {
        let dtb = [0xd0, 0x0d, 0xfe, 0xed, 1, 2, 3];
        let mut rom = BootRom::spike(0x8000_0000, &dtb);
        let mut w = Wires::default();
        assert_eq!(rom.size(), 0x1000);
        let words: Vec<u64> = (0..6).map(|i| rom.read(4 * i, 4, &mut w).unwrap()).collect();
        assert_eq!(words, vec![0x297, 0x0202_8593, 0xf140_2573, 0x0182_b283, 0x0002_8067, 0]);
        assert_eq!(rom.read(0x18, 8, &mut w), Ok(0x8000_0000));
        assert_eq!(rom.read(0x20, 4, &mut w), Ok(0xedfe_0dd0));
        assert_eq!(rom.read(0xffc, 4, &mut w), Ok(0));
        assert_eq!(rom.read(0x1000, 4, &mut w), Err(BusFault));
        assert_eq!(rom.write(0, 4, 0, &mut w), Err(BusFault), "the ROM is read-only");
    }

    #[test]
    fn uart_reset_values() {
        let mut soc = SpikeSoc::new();
        let mut w = Wires::default();
        assert_eq!(soc.read(UART + 2, 1, &mut w), Ok(0xc1), "IIR always shows FIFO bits");
        assert_eq!(soc.read(UART + 4, 1, &mut w), Ok(0x08), "MCR resets to OUT2");
        assert_eq!(soc.read(UART + 5, 1, &mut w), Ok(0x60));
        assert_eq!(soc.read(UART + 6, 1, &mut w), Ok(0xb0));
        soc.write(UART + 3, 1, 0x80, &mut w).unwrap();
        assert_eq!(soc.read(UART, 1, &mut w), Ok(0x0c), "DLL resets to 0x0c");
        assert_eq!(soc.read(UART + 8 + 3, 1, &mut w), Ok(0x80), "registers repeat every 8 bytes");
        assert_eq!(soc.read(UART + 3, 4, &mut w), Err(BusFault), "byte access only");
    }

    #[test]
    fn uart_thre_interrupt_reaches_plic_in_the_same_access() {
        let mut soc = SpikeSoc::new();
        let mut w = Wires::default();
        soc.write(4, 4, 1, &mut w).unwrap(); // source 1 priority
        soc.write(ENABLE_S, 4, 1 << 1, &mut w).unwrap();
        soc.write(UART + 1, 1, u64::from(IER_THRI), &mut w).unwrap();
        assert_eq!(w.hart_irq & bits::SEIP, bits::SEIP);
        assert_eq!(soc.read(UART + 2, 1, &mut w), Ok(0xc2), "IIR read has no side effect");
        assert_eq!(soc.read(UART + 2, 1, &mut w), Ok(0xc2));
        assert_eq!(soc.read(CLAIM_S, 4, &mut w), Ok(1));
        assert_eq!(w.hart_irq & bits::SEIP, 0);
        assert_eq!(soc.read(0x1000, 4, &mut w), Ok(1 << 1), "claimed sources stay pending");
        soc.write(UART, 1, u64::from(b'A'), &mut w).unwrap();
        assert_eq!(w.hart_irq & bits::SEIP, 0, "still claimed");
        soc.write(CLAIM_S, 4, 1, &mut w).unwrap();
        assert_eq!(w.hart_irq & bits::SEIP, bits::SEIP);
        soc.write(UART + 1, 1, 0, &mut w).unwrap();
        assert_eq!(w.hart_irq & bits::SEIP, 0, "level low clears pending");
        let mut out = Vec::new();
        soc.drain_output_into(&mut out);
        assert_eq!(out, b"A");
    }

    #[test]
    fn uart_rdi_and_thri_combine_in_iir() {
        let mut soc = SpikeSoc::new();
        let mut w = Wires::default();
        soc.write(UART + 2, 1, u64::from(FCR_ENABLE_FIFO), &mut w).unwrap();
        soc.write(UART + 1, 1, u64::from(IER_RDI | IER_THRI), &mut w).unwrap();
        soc.push_input(b"xy");
        soc.tick(&mut w);
        assert_eq!(soc.read(UART + 2, 1, &mut w), Ok(0xc6));
        assert_eq!(soc.read(UART + 5, 1, &mut w), Ok(0x61));
        assert_eq!(soc.read(UART, 1, &mut w), Ok(u64::from(b'x')));
        assert_eq!(soc.read(UART + 5, 1, &mut w), Ok(0x60), "one byte per tick");
        soc.tick(&mut w);
        assert_eq!(soc.read(UART, 1, &mut w), Ok(u64::from(b'y')));
    }

    #[test]
    fn plic_semantics() {
        let mut soc = SpikeSoc::new();
        let mut w = Wires::default();
        soc.write(4, 4, 0xff, &mut w).unwrap();
        assert_eq!(soc.read(4, 4, &mut w), Ok(0xf), "4 priority bits");
        soc.write(0x2000, 4, 0xffff_ffff, &mut w).unwrap();
        assert_eq!(soc.read(0x2000, 4, &mut w), Ok(0xffff_fffe), "M context enable");
        assert_eq!(soc.read(0x2100, 4, &mut w), Err(BusFault), "no third context");
        assert_eq!(soc.read(0x20_2000, 4, &mut w), Err(BusFault));
        assert_eq!(soc.write(0x20_0008, 4, 0, &mut w), Err(BusFault), "reserved context word");
        assert_eq!(soc.read(0x20_0008, 4, &mut w), Ok(0));
        assert_eq!(soc.read(0, 2, &mut w), Err(BusFault), "no subword access");
        soc.write(0x20_0000, 4, 0xf, &mut w).unwrap();
        assert_eq!(soc.read(0x20_0000, 4, &mut w), Ok(0xf));
        assert_eq!(soc.read(0x0400_0000 - 4, 4, &mut w), Err(BusFault), "gap between PLIC and UART");
    }
}
