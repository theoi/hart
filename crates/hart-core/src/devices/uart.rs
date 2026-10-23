//! NS16550A UART, enough for Linux's 8250 driver and OpenSBI's uart8250.
//!
//! Transmission is instantaneous: a byte written to THR goes straight to the
//! output buffer and THR is empty again at once. Received bytes come from an
//! unbounded host input queue, so host input is never dropped (an FCR
//! receive-FIFO reset does not discard it). Registers use `reg-shift = 0` and
//! byte access.
//!
//! Host input enters the receive FIFO only while the receive-data interrupt
//! (`IER.RDI`) is enabled; until then it waits in the host queue. Linux's 8250
//! driver reads RBR twice while it opens the port, before it enables that
//! interrupt, so on a real 16550 the first keys typed during boot are lost. A
//! guest that polls LSR with `IER.RDI` clear therefore sees no host input.
//! Loopback bytes go straight to the FIFO.
//!
//! The UART drives its PLIC source in [`Wires::irq_sources`] after every
//! access and on [`Device::tick`] (call `tick` after [`Uart::push_input`]).

use crate::bus::{BusFault, Device};
use crate::irq::Wires;
use std::collections::VecDeque;

pub const UART_BASE: u64 = 0x1000_0000;
pub const UART_SIZE: u64 = 0x100;
/// PLIC source for the UART, as on QEMU virt.
pub const UART_IRQ: u32 = 10;
pub const UART_CLOCK_HZ: u32 = 3_686_400;

const IER_RDI: u8 = 0x01;
const IER_THRI: u8 = 0x02;

const IIR_NO_INT: u8 = 0x01;
const IIR_THRI: u8 = 0x02;
const IIR_RDI: u8 = 0x04;
const IIR_FIFO_ENABLED: u8 = 0xc0;

const FCR_ENABLE_FIFO: u8 = 0x01;

const LCR_DLAB: u8 = 0x80;
const MCR_LOOP: u8 = 0x10;

const LSR_DR: u8 = 0x01;
const LSR_THRE: u8 = 0x20;
const LSR_TEMT: u8 = 0x40;

const MSR_CTS: u8 = 0x10;
const MSR_DSR: u8 = 0x20;
const MSR_RI: u8 = 0x40;
const MSR_DCD: u8 = 0x80;

#[derive(Clone, Debug)]
pub struct Uart {
    irq_source: u32,
    ier: u8,
    lcr: u8,
    mcr: u8,
    scr: u8,
    fifo_enabled: bool,
    dll: u8,
    dlm: u8,
    thre_pending: bool,
    /// Host input not yet moved to `rx` (see the module docs).
    host: VecDeque<u8>,
    rx: VecDeque<u8>,
    tx: Vec<u8>,
}

impl Default for Uart {
    fn default() -> Self {
        Self::new(UART_IRQ)
    }
}

impl Uart {
    /// A UART wired to PLIC source `irq_source`.
    pub fn new(irq_source: u32) -> Self {
        Self {
            irq_source,
            ier: 0,
            lcr: 0,
            mcr: 0,
            scr: 0,
            fifo_enabled: false,
            dll: 0,
            dlm: 0,
            thre_pending: false,
            host: VecDeque::new(),
            rx: VecDeque::new(),
            tx: Vec::new(),
        }
    }

    /// Queues bytes typed by the host for the guest to receive.
    pub fn push_input(&mut self, bytes: &[u8]) {
        self.host.extend(bytes);
        self.offer_input();
    }

    /// Bytes queued for the guest that it has not read yet.
    pub fn pending_input(&self) -> usize {
        self.host.len() + self.rx.len()
    }

    fn offer_input(&mut self) {
        if self.ier & IER_RDI != 0 {
            self.rx.append(&mut self.host);
        }
    }

    /// Bytes the guest has transmitted since the last drain.
    pub fn output(&self) -> &[u8] {
        &self.tx
    }

    /// Takes the bytes the guest has transmitted.
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.tx)
    }

    /// Appends the transmitted bytes to `out` and clears them.
    pub fn drain_output_into(&mut self, out: &mut Vec<u8>) {
        out.append(&mut self.tx);
    }

    /// The interrupt line level.
    pub fn irq(&self) -> bool {
        self.interrupt_id() != IIR_NO_INT
    }

    // Line-status and modem-status interrupts never fire: no line errors
    // are generated and the modem lines never change.
    fn interrupt_id(&self) -> u8 {
        if self.ier & IER_RDI != 0 && !self.rx.is_empty() {
            IIR_RDI
        } else if self.ier & IER_THRI != 0 && self.thre_pending {
            IIR_THRI
        } else {
            IIR_NO_INT
        }
    }

    fn dlab(&self) -> bool {
        self.lcr & LCR_DLAB != 0
    }

    fn lsr(&self) -> u8 {
        let dr = if self.rx.is_empty() { 0 } else { LSR_DR };
        dr | LSR_THRE | LSR_TEMT
    }

    fn msr(&self) -> u8 {
        if self.mcr & MCR_LOOP != 0 {
            let m = self.mcr;
            let mut v = 0;
            if m & 0x02 != 0 {
                v |= MSR_CTS;
            }
            if m & 0x01 != 0 {
                v |= MSR_DSR;
            }
            if m & 0x04 != 0 {
                v |= MSR_RI;
            }
            if m & 0x08 != 0 {
                v |= MSR_DCD;
            }
            v
        } else {
            MSR_CTS | MSR_DSR | MSR_DCD
        }
    }

    fn transmit(&mut self, byte: u8) {
        if self.mcr & MCR_LOOP != 0 {
            self.rx.push_back(byte);
        } else {
            self.tx.push(byte);
        }
        // THR empties at once, which re-raises the THRE interrupt.
        self.thre_pending = true;
    }

    /// Register read without driving the interrupt line.
    pub fn read_reg(&mut self, offset: u64) -> u8 {
        match offset {
            0 if self.dlab() => self.dll,
            0 => self.rx.pop_front().unwrap_or(0),
            1 if self.dlab() => self.dlm,
            1 => self.ier,
            2 => {
                let id = self.interrupt_id();
                if id == IIR_THRI {
                    self.thre_pending = false;
                }
                id | if self.fifo_enabled { IIR_FIFO_ENABLED } else { 0 }
            }
            3 => self.lcr,
            4 => self.mcr,
            5 => self.lsr(),
            6 => self.msr(),
            7 => self.scr,
            _ => 0,
        }
    }

    /// Register write without driving the interrupt line.
    pub fn write_reg(&mut self, offset: u64, v: u8) {
        match offset {
            0 if self.dlab() => self.dll = v,
            0 => self.transmit(v),
            1 if self.dlab() => self.dlm = v,
            1 => {
                let old = self.ier;
                self.ier = v & 0x0f;
                if (old ^ self.ier) & IER_THRI != 0 {
                    // Enabling THRI with THR empty raises it at once;
                    // disabling it drops any pending THRE interrupt.
                    self.thre_pending = self.ier & IER_THRI != 0;
                }
                self.offer_input();
            }
            2 => self.fifo_enabled = v & FCR_ENABLE_FIFO != 0,
            3 => self.lcr = v,
            4 => self.mcr = v & 0x1f,
            7 => self.scr = v,
            _ => {}
        }
    }
}

impl Device for Uart {
    fn read(&mut self, offset: u64, _size: u8, wires: &mut Wires) -> Result<u64, BusFault> {
        let v = self.read_reg(offset);
        wires.set_irq_source(self.irq_source, self.irq());
        Ok(u64::from(v))
    }

    fn write(&mut self, offset: u64, _size: u8, value: u64, wires: &mut Wires) -> Result<(), BusFault> {
        self.write_reg(offset, value as u8);
        wires.set_irq_source(self.irq_source, self.irq());
        Ok(())
    }

    fn tick(&mut self, wires: &mut Wires) {
        wires.set_irq_source(self.irq_source, self.irq());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RBR: u64 = 0;
    const IER: u64 = 1;
    const IIR: u64 = 2;
    const LCR: u64 = 3;
    const MCR: u64 = 4;
    const LSR: u64 = 5;
    const MSR: u64 = 6;
    const SCR: u64 = 7;

    #[test]
    fn transmit_goes_to_output() {
        let mut u = Uart::default();
        assert_eq!(u.read_reg(LSR) & (LSR_THRE | LSR_TEMT), LSR_THRE | LSR_TEMT);
        for b in b"hi\n" {
            u.write_reg(RBR, *b);
        }
        assert_eq!(u.take_output(), b"hi\n");
        assert!(u.output().is_empty());
        assert!(!u.irq(), "no interrupts enabled");
    }

    #[test]
    fn receive_data_and_interrupt() {
        let mut u = Uart::default();
        u.write_reg(IER, IER_RDI);
        assert_eq!(u.read_reg(LSR) & LSR_DR, 0);
        assert!(!u.irq());
        u.push_input(b"ab");
        assert_eq!(u.read_reg(LSR) & LSR_DR, LSR_DR);
        assert!(u.irq());
        assert_eq!(u.read_reg(IIR) & 0x0f, IIR_RDI);
        assert_eq!(u.read_reg(RBR), b'a');
        assert_eq!(u.read_reg(RBR), b'b');
        assert!(!u.irq());
        assert_eq!(u.read_reg(IIR) & 0x0f, IIR_NO_INT);
        assert_eq!(u.read_reg(RBR), 0);
    }

    #[test]
    fn thre_interrupt_follows_8250_startup_test() {
        // Mirrors serial8250_THRE_test and the TXEN check in
        // serial8250_do_startup.
        let mut u = Uart::default();
        u.write_reg(IER, IER_THRI);
        assert!(u.irq());
        let iir1 = u.read_reg(IIR);
        assert_eq!(iir1 & 0x0f, IIR_THRI);
        assert!(!u.irq(), "reading IIR clears THRE");
        u.write_reg(IER, 0);
        u.write_reg(IER, IER_THRI);
        let iir = u.read_reg(IIR);
        assert_eq!(iir & IIR_NO_INT, 0, "THRE reasserted when re-enabled");
        u.write_reg(IER, 0);
        assert!(!u.irq());
    }

    #[test]
    fn thre_reasserts_after_each_write() {
        let mut u = Uart::default();
        u.write_reg(IER, IER_THRI);
        assert_eq!(u.read_reg(IIR) & 0x0f, IIR_THRI);
        assert!(!u.irq());
        u.write_reg(RBR, b'x');
        assert!(u.irq());
    }

    #[test]
    fn receive_has_priority_over_thre() {
        let mut u = Uart::default();
        u.write_reg(IER, IER_THRI | IER_RDI);
        u.push_input(b"z");
        assert_eq!(u.read_reg(IIR) & 0x0f, IIR_RDI);
        u.read_reg(RBR);
        assert_eq!(u.read_reg(IIR) & 0x0f, IIR_THRI);
    }

    #[test]
    fn divisor_latch_and_scratch() {
        let mut u = Uart::default();
        u.write_reg(LCR, 0x83);
        u.write_reg(RBR, 0x01);
        u.write_reg(IER, 0x00);
        assert_eq!(u.read_reg(RBR), 1);
        assert_eq!(u.read_reg(IER), 0);
        u.write_reg(LCR, 0x03);
        assert!(u.output().is_empty(), "DLL write is not a transmit");
        assert_eq!(u.read_reg(LCR), 3);
        u.write_reg(SCR, 0x5a);
        assert_eq!(u.read_reg(SCR), 0x5a);
    }

    #[test]
    fn fifo_flag_in_iir() {
        let mut u = Uart::default();
        assert_eq!(u.read_reg(IIR), IIR_NO_INT);
        u.write_reg(IIR, 0x07);
        assert_eq!(u.read_reg(IIR), IIR_FIFO_ENABLED | IIR_NO_INT);
    }

    #[test]
    fn host_input_survives_fifo_reset() {
        let mut u = Uart::default();
        u.write_reg(IER, IER_RDI);
        u.push_input(b"q");
        u.write_reg(IIR, 0x07);
        assert_eq!(u.read_reg(RBR), b'q');
    }

    #[test]
    fn input_typed_before_the_port_opens_survives_8250_startup() {
        let mut u = Uart::default();
        u.push_input(b"sleep 1\r");
        assert_eq!(u.read_reg(LSR) & LSR_DR, 0, "held until IER.RDI is enabled");
        // serial8250_do_startup: clear the FIFOs, then read LSR, RX, IIR, MSR
        // twice around the THRE and TXEN tests, then enable RLSI | RDI.
        u.write_reg(IIR, 0x07);
        u.write_reg(IIR, 0x00);
        for _ in 0..2 {
            for reg in [LSR, RBR, IIR, MSR] {
                u.read_reg(reg);
            }
            u.write_reg(IER, IER_THRI);
            u.read_reg(IIR);
            u.write_reg(IER, 0);
        }
        u.write_reg(IER, IER_RDI | 0x04);
        let mut got = Vec::new();
        while u.read_reg(LSR) & LSR_DR != 0 {
            got.push(u.read_reg(RBR));
        }
        assert_eq!(got, b"sleep 1\r");
        assert_eq!(u.pending_input(), 0);
    }

    #[test]
    fn input_waits_while_rdi_is_disabled() {
        let mut u = Uart::default();
        u.write_reg(IER, IER_RDI);
        u.push_input(b"a");
        // The 8250 console write path masks IER while it prints.
        u.write_reg(IER, 0);
        u.push_input(b"b");
        assert_eq!(u.pending_input(), 2);
        assert_eq!(u.read_reg(RBR), b'a', "bytes already in the FIFO stay readable");
        assert_eq!(u.read_reg(LSR) & LSR_DR, 0);
        u.write_reg(IER, IER_RDI);
        assert_eq!(u.read_reg(RBR), b'b');
    }

    #[test]
    fn device_drives_plic_source() {
        let mut u = Uart::new(3);
        let mut w = Wires::default();
        u.write(IER, 1, u64::from(IER_RDI), &mut w).unwrap();
        assert!(!w.irq_source(3));
        u.push_input(b"k");
        u.tick(&mut w);
        assert!(w.irq_source(3));
        assert_eq!(u.read(RBR, 1, &mut w), Ok(u64::from(b'k')));
        assert!(!w.irq_source(3));
    }

    #[test]
    fn loopback() {
        let mut u = Uart::default();
        u.write_reg(MCR, MCR_LOOP | 0x0b);
        assert_eq!(u.read_reg(MSR), MSR_CTS | MSR_DSR | MSR_DCD);
        u.write_reg(RBR, 0x42);
        assert!(u.output().is_empty());
        assert_eq!(u.read_reg(RBR), 0x42);
        u.write_reg(MCR, 0x0b);
        assert_eq!(u.read_reg(MSR), MSR_CTS | MSR_DSR | MSR_DCD);
        assert_ne!(u.read_reg(LSR), 0xff);
    }
}
