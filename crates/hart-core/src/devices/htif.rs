//! Berkeley HTIF host interface (`tohost`/`fromhost`), following Spike's
//! front-end server (riscv-isa-sim `fesvr/htif.cc` and `fesvr/device.cc`).
//!
//! HTIF is not an MMIO device: `tohost` and `fromhost` are ordinary RAM words
//! (usually found through ELF symbols). Like Spike, the host polls them
//! between batches of guest instructions, through [`Htif::poll`].
//!
//! Supported commands, as in Spike:
//! - device 0 (syscall), payload bit 0 set: exit with code `payload >> 1`.
//!   Proxied system calls (payload bit 0 clear) are not supported and are
//!   recorded in [`Htif::unsupported`].
//! - device 1 (console), command 1: write the payload's low byte.
//! - device 1 (console), command 0: read a byte; answered through
//!   `fromhost` with `0x100 | byte` once host input is available.
//!
//! Commands to other devices are ignored, as Spike's null device does.
//!
//! Guests must write `tohost` with a single 64-bit store or with the high
//! word last: a poll between two 32-bit stores would see a partial command,
//! as it would in Spike.
//!
//! Typical bare-metal test loop:
//!
//! ```ignore
//! let elf = loader::load_elf(&mut bus, &bytes)?;
//! let mut htif = Htif::from_elf(&elf).expect("no tohost symbol");
//! let mut cpu = Cpu::new(elf.entry);
//! while bus.wires.halt.is_none() {
//!     cpu.run(&mut bus, 10_000);
//!     htif.poll_bus(&mut bus);
//! }
//! // bus.wires.halt == Some(Halt::TestExit(0)) means pass.
//! ```

use crate::bus::Bus;
use crate::irq::Halt;
use crate::loader::LoadedElf;
use std::collections::VecDeque;

/// Guest memory as seen by the HTIF host. Accesses that fall outside RAM
/// return `None`/`false`.
pub trait HtifMemory {
    fn read_u64(&mut self, addr: u64) -> Option<u64>;
    fn write_u64(&mut self, addr: u64, value: u64) -> bool;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HtifConfig {
    pub tohost: u64,
    /// When `None`, responses are discarded.
    pub fromhost: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct Htif {
    config: Option<HtifConfig>,
    exit: Option<u64>,
    console_out: Vec<u8>,
    console_in: VecDeque<u8>,
    pending_reads: VecDeque<u64>,
    fromhost_queue: VecDeque<u64>,
    unsupported: Option<u64>,
    bad_address: bool,
}

impl Htif {
    /// An HTIF that is not connected to guest memory; [`Htif::poll`] does
    /// nothing.
    pub fn disabled() -> Self {
        Self::default()
    }

    pub fn new(config: HtifConfig) -> Self {
        Self { config: Some(config), ..Self::default() }
    }

    /// An HTIF at the ELF's `tohost`/`fromhost` symbols, if it has `tohost`.
    pub fn from_elf(elf: &LoadedElf) -> Option<Self> {
        Some(Self::new(HtifConfig { tohost: elf.tohost?, fromhost: elf.fromhost }))
    }

    pub fn config(&self) -> Option<&HtifConfig> {
        self.config.as_ref()
    }

    pub fn is_enabled(&self) -> bool {
        self.config.is_some()
    }

    /// The raw exit value (`payload`, bit 0 set) once the guest has exited.
    pub fn exit_value(&self) -> Option<u64> {
        self.exit
    }

    /// The exit code (`payload >> 1`): 0 means pass. For riscv-tests a
    /// non-zero code is the failing test number.
    pub fn exit_code(&self) -> Option<u64> {
        self.exit.map(|v| v >> 1)
    }

    /// The first unsupported command seen (raw `tohost` value), if any.
    pub fn unsupported(&self) -> Option<u64> {
        self.unsupported
    }

    /// Whether the host failed to access `tohost`/`fromhost`.
    pub fn bad_address(&self) -> bool {
        self.bad_address
    }

    pub fn push_console_input(&mut self, bytes: &[u8]) {
        self.console_in.extend(bytes);
    }

    pub fn console_output(&self) -> &[u8] {
        &self.console_out
    }

    pub fn take_console_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.console_out)
    }

    /// Runs one iteration of Spike's HTIF loop: [`Htif::check_tohost`]
    /// then [`Htif::service`]. Returns `true` when a command was found in
    /// `tohost`; Spike runs no guest instructions in such an iteration.
    pub fn poll(&mut self, mem: &mut impl HtifMemory) -> bool {
        let handled = self.check_tohost(mem);
        self.service(mem);
        handled
    }

    /// Reads `tohost`; a non-zero value is cleared and handled. Returns
    /// whether there was a command.
    pub fn check_tohost(&mut self, mem: &mut impl HtifMemory) -> bool {
        let Some(config) = &self.config else {
            return false;
        };
        let tohost_addr = config.tohost;
        let Some(tohost) = mem.read_u64(tohost_addr) else {
            self.bad_address = true;
            return false;
        };
        if tohost == 0 {
            return false;
        }
        mem.write_u64(tohost_addr, 0);
        self.handle(tohost);
        true
    }

    /// Answers pending console reads and delivers one queued response to a
    /// free `fromhost`, as Spike does after each loop iteration.
    pub fn service(&mut self, mem: &mut impl HtifMemory) {
        let Some(config) = &self.config else {
            return;
        };
        let fromhost_addr = config.fromhost;
        self.tick();
        let Some(fromhost) = fromhost_addr else {
            self.fromhost_queue.clear();
            return;
        };
        if let Some(&response) = self.fromhost_queue.front() {
            match mem.read_u64(fromhost) {
                Some(0) => {
                    mem.write_u64(fromhost, response);
                    self.fromhost_queue.pop_front();
                }
                Some(_) => {}
                None => self.bad_address = true,
            }
        }
    }

    /// [`Htif::poll`] against the bus. When the guest exits, sets
    /// `bus.wires.halt` to [`Halt::TestExit`] with the exit code.
    pub fn poll_bus(&mut self, bus: &mut Bus) -> bool {
        let handled = self.poll(bus);
        self.sync_halt(bus);
        handled
    }

    /// Sets `bus.wires.halt` once the guest has exited through HTIF.
    pub fn sync_halt(&self, bus: &mut Bus) {
        if let Some(code) = self.exit_code() {
            bus.wires.halt.get_or_insert(Halt::TestExit(code));
        }
    }

    fn handle(&mut self, tohost: u64) {
        let device = (tohost >> 56) as u8;
        let command = (tohost >> 48) as u8;
        let payload = tohost & ((1 << 48) - 1);
        match (device, command) {
            (0, _) if payload & 1 != 0 => {
                if self.exit.is_none() {
                    self.exit = Some(payload);
                }
            }
            (1, 0) => self.pending_reads.push_back(tohost),
            (1, 1) => self.console_out.push(payload as u8),
            (0 | 1, _) => {
                self.unsupported.get_or_insert(tohost);
            }
            _ => {}
        }
    }

    fn tick(&mut self) {
        if self.pending_reads.is_empty() {
            return;
        }
        if let Some(byte) = self.console_in.pop_front() {
            let request = self.pending_reads.pop_front().unwrap_or_default();
            self.fromhost_queue.push_back((request >> 48 << 48) | 0x100 | u64::from(byte));
        }
    }
}

impl HtifMemory for Bus {
    fn read_u64(&mut self, addr: u64) -> Option<u64> {
        self.read(addr, 8).ok()
    }

    fn write_u64(&mut self, addr: u64, value: u64) -> bool {
        self.write(addr, 8, value).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Mem(HashMap<u64, u64>);

    impl HtifMemory for Mem {
        fn read_u64(&mut self, addr: u64) -> Option<u64> {
            (addr < 0x1000).then(|| self.0.get(&addr).copied().unwrap_or(0))
        }
        fn write_u64(&mut self, addr: u64, value: u64) -> bool {
            if addr >= 0x1000 {
                return false;
            }
            self.0.insert(addr, value);
            true
        }
    }

    const TOHOST: u64 = 0x100;
    const FROMHOST: u64 = 0x108;

    fn htif() -> Htif {
        Htif::new(HtifConfig { tohost: TOHOST, fromhost: Some(FROMHOST) })
    }

    #[test]
    fn exit_pass_and_fail() {
        let mut mem = Mem::default();
        let mut h = htif();
        assert!(!h.poll(&mut mem));
        assert_eq!(h.exit_code(), None);
        mem.0.insert(TOHOST, 1);
        assert!(h.poll(&mut mem));
        assert_eq!(h.exit_code(), Some(0));
        assert_eq!(mem.0[&TOHOST], 0, "host clears tohost");

        let mut h = htif();
        mem.0.insert(TOHOST, (5 << 1) | 1);
        h.poll(&mut mem);
        assert_eq!(h.exit_code(), Some(5));
        assert_eq!(h.exit_value(), Some(11));
    }

    #[test]
    fn console_write() {
        let mut mem = Mem::default();
        let mut h = htif();
        for &b in b"ok" {
            mem.0.insert(TOHOST, (1 << 56) | (1 << 48) | u64::from(b));
            assert!(h.poll(&mut mem));
        }
        assert_eq!(h.take_console_output(), b"ok");
        assert_eq!(mem.0.get(&FROMHOST).copied().unwrap_or(0), 0, "no response to writes");
    }

    #[test]
    fn console_read_waits_for_input_and_free_fromhost() {
        let mut mem = Mem::default();
        let mut h = htif();
        mem.0.insert(TOHOST, 1 << 56);
        h.poll(&mut mem);
        assert_eq!(mem.0.get(&FROMHOST).copied().unwrap_or(0), 0);
        h.push_console_input(b"xy");
        mem.0.insert(FROMHOST, 0xdead);
        h.poll(&mut mem);
        assert_eq!(mem.0[&FROMHOST], 0xdead, "fromhost busy");
        mem.0.insert(FROMHOST, 0);
        h.poll(&mut mem);
        assert_eq!(mem.0[&FROMHOST], (1 << 56) | 0x100 | u64::from(b'x'));
    }

    #[test]
    fn poll_bus_sets_halt() {
        let mut bus = Bus::new(0x8000_0000, 0x2000);
        let mut h = Htif::new(HtifConfig { tohost: 0x8000_1000, fromhost: Some(0x8000_1008) });
        assert!(!h.poll_bus(&mut bus));
        assert_eq!(bus.wires.halt, None);
        bus.write(0x8000_1000, 4, (3 << 1) | 1).unwrap();
        bus.write(0x8000_1004, 4, 0).unwrap();
        assert!(h.poll_bus(&mut bus));
        assert_eq!(bus.wires.halt, Some(Halt::TestExit(3)));
        assert_eq!(bus.read(0x8000_1000, 8), Ok(0));
    }

    #[test]
    fn unsupported_syscall_and_bad_address() {
        let mut mem = Mem::default();
        let mut h = htif();
        mem.0.insert(TOHOST, 0x80);
        h.poll(&mut mem);
        assert_eq!(h.unsupported(), Some(0x80));
        assert_eq!(h.exit_code(), None);

        let mut h = Htif::new(HtifConfig { tohost: 0x2000, fromhost: None });
        h.poll(&mut mem);
        assert!(h.bad_address());

        let mut h = Htif::disabled();
        assert!(!h.poll(&mut mem));
    }
}
