//! The virt-style board: memory map, device wiring, image loading, and the
//! run loop.
//!
//! [`Board`] is the embedding API shared by the native CLI and the web
//! build. It uses no host I/O, threads, or clocks: the embedder feeds console
//! input, drains console output, and supplies host time through
//! [`TimeSource::Host`] when the guest should see real time.
//!
//! Memory map (QEMU virt compatible):
//!
//! | Address       | Device                                  |
//! | ------------- | --------------------------------------- |
//! | `0x0010_0000` | syscon (SiFive test finisher)            |
//! | `0x0200_0000` | CLINT                                   |
//! | `0x0c00_0000` | PLIC (UART on source 10)                 |
//! | `0x1000_0000` | NS16550A UART                           |
//! | `0x8000_0000` | RAM: firmware, kernel at `+0x20_0000`    |
//!
//! Boot follows QEMU: the hart starts in M-mode at the firmware entry with
//! `a0` = hart ID 0, `a1` = device tree address (near the top of RAM), and
//! `a2` = a `fw_dynamic_info` block, so both OpenSBI `fw_jump` and
//! `fw_dynamic` boot.

pub mod devicetree;
pub mod spike;

use crate::bus::{Bus, DeviceId};
use crate::cpu::{Cpu, CpuConfig, StepResult};
use crate::devices::clint::{Clint, CLINT_BASE, CLINT_SIZE};
use crate::devices::htif::Htif;
use crate::devices::plic::{Plic, PLIC_BASE, PLIC_SIZE};
use crate::devices::sig::{InterruptTrigger, SIG_BASE, SIG_SIZE};
use crate::devices::syscon::{Syscon, SYSCON_BASE, SYSCON_SIZE};
use crate::devices::uart::{Uart, UART_BASE, UART_IRQ, UART_SIZE};
use crate::fdt::{Fdt, FdtError};
use crate::irq::Halt;
use crate::loader::{self, LoadError};
use devicetree::{DeviceTreeConfig, TIMEBASE_HZ};
use spike::{
    BootRom, SpikeSoc, INTERLEAVE, SPIKE_CLINT_SIZE, SPIKE_PLIC_BASE, SPIKE_ROM_BASE, SPIKE_SOC_SIZE, TICKS_PER_CHUNK,
};
use std::fmt;

pub const RAM_BASE: u64 = 0x8000_0000;
pub const DEFAULT_RAM_SIZE: u64 = 256 << 20;
/// Where the kernel `Image` is loaded (OpenSBI's default jump address).
pub const KERNEL_ADDR: u64 = RAM_BASE + 0x20_0000;
pub const DEFAULT_BOOTARGS: &str = "console=ttyS0";

const FW_DYNAMIC_MAGIC: u64 = 0x4942_534f;
const FW_DYNAMIC_VERSION: u64 = 2;
const NEXT_MODE_S: u64 = 1;

/// Instructions per run-loop batch. Devices tick, time advances, and HTIF
/// is polled between batches.
const BATCH: u64 = 10_000;

const DTB_ALIGN: u64 = 2 << 20;

/// How the machine timer (`mtime`, 10 MHz) advances.
pub enum TimeSource {
    /// One tick every `insns_per_tick` instructions. Deterministic: the same
    /// inputs give the same run. While the hart waits in WFI, time jumps to
    /// the next timer interrupt.
    Instructions { insns_per_tick: u64 },
    /// Host time: the closure returns a monotonic clock in nanoseconds.
    Host(Box<dyn FnMut() -> u64>),
}

impl fmt::Debug for TimeSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Instructions { insns_per_tick } => {
                f.debug_struct("Instructions").field("insns_per_tick", insns_per_tick).finish()
            }
            Self::Host(_) => f.write_str("Host(..)"),
        }
    }
}

impl Default for TimeSource {
    /// 100 instructions per tick: 10 MHz at 1 GIPS, as in Spike.
    fn default() -> Self {
        Self::Instructions { insns_per_tick: 100 }
    }
}

#[derive(Debug)]
pub struct BoardConfig {
    pub ram_size: u64,
    /// Kernel command line for a generated device tree.
    pub bootargs: String,
    pub time: TimeSource,
}

impl Default for BoardConfig {
    fn default() -> Self {
        Self { ram_size: DEFAULT_RAM_SIZE, bootargs: DEFAULT_BOOTARGS.to_string(), time: TimeSource::default() }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum BoardError {
    Load(LoadError),
    Dtb(FdtError),
    /// The images do not fit in RAM together.
    Layout(String),
}

impl fmt::Display for BoardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Load(e) => write!(f, "loading image: {e}"),
            Self::Dtb(e) => write!(f, "device tree: {e}"),
            Self::Layout(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for BoardError {}

impl From<LoadError> for BoardError {
    fn from(e: LoadError) -> Self {
        Self::Load(e)
    }
}

/// Whether the guest is still running, and why it stopped if not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerState {
    Running,
    /// The guest powered off (syscon).
    PowerOff,
    /// The guest asked for a reboot. Build a new [`Board`] to restart.
    Reboot,
    /// A test exit (HTIF or syscon failure code). 0 means pass.
    Exit(u64),
}

/// Where boot images were placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootLayout {
    pub entry: u64,
    pub kernel: Option<(u64, u64)>,
    pub dtb: (u64, u64),
    pub fw_dynamic_info: u64,
}

/// Which machine the board models.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    /// The QEMU-virt-compatible board that boots Linux.
    Virt,
    /// Bare-metal tests: HTIF, CLINT, and the test interrupt trigger.
    Test,
    /// Spike-compatible lockstep profile (see [`spike`]).
    Spike,
}

#[derive(Clone, Copy, Debug)]
enum Console {
    Uart(DeviceId),
    Spike(DeviceId),
}

pub struct Board {
    pub cpu: Cpu,
    pub bus: Bus,
    profile: Profile,
    console: Console,
    clint: DeviceId,
    htif: Htif,
    time: TimeSource,
    insn_carry: u64,
    host_last_ns: Option<u64>,
    host_carry: u128,
    instret: u64,
    dtb: Vec<u8>,
    layout: BootLayout,
    /// Test profile: a WFI that nothing could ever wake (no enabled
    /// interrupt, no timer ahead) completes instead of stalling, as the spec
    /// allows.
    resume_stuck_wfi: bool,
}

fn align_down(v: u64, a: u64) -> u64 {
    v & !(a - 1)
}

/// The size a RISC-V Linux `Image` occupies once loaded, from its header,
/// or the file size for other images.
pub fn kernel_footprint(image: &[u8]) -> u64 {
    let file = image.len() as u64;
    if image.len() >= 64 && &image[48..56] == b"RISCV\0\0\0" {
        let size = u64::from_le_bytes(image[16..24].try_into().unwrap());
        size.max(file)
    } else {
        file
    }
}

impl Board {
    /// Builds the virt board and loads `firmware` (raw binary at the start
    /// of RAM, or an ELF), an optional kernel `Image`, and either the given
    /// device tree or a generated one.
    pub fn new(
        config: BoardConfig,
        firmware: &[u8],
        kernel: Option<&[u8]>,
        dtb: Option<&[u8]>,
    ) -> Result<Self, BoardError> {
        let mut bus = Bus::new(RAM_BASE, config.ram_size as usize);
        let clint = bus.add_device(CLINT_BASE, CLINT_SIZE, Box::new(Clint::new()));
        bus.add_device(SYSCON_BASE, SYSCON_SIZE, Box::new(Syscon::new()));
        let uart = bus.add_device(UART_BASE, UART_SIZE, Box::new(Uart::new(UART_IRQ)));
        // The PLIC goes last so a tick samples the levels the other devices
        // just drove.
        bus.add_device(PLIC_BASE, PLIC_SIZE, Box::new(Plic::default()));

        let entry = if loader::is_elf(firmware) {
            loader::load_elf(&mut bus, firmware)?.entry
        } else {
            loader::load_raw(&mut bus, RAM_BASE, firmware)?;
            RAM_BASE
        };
        let ram_end = RAM_BASE + config.ram_size;
        let mut low_limit = RAM_BASE + firmware.len() as u64;
        let kernel_range = match kernel {
            Some(image) => {
                let end = KERNEL_ADDR + kernel_footprint(image);
                if end > ram_end {
                    return Err(BoardError::Layout(format!(
                        "the kernel needs RAM up to {end:#x}, but RAM ends at {ram_end:#x}"
                    )));
                }
                loader::load_raw(&mut bus, KERNEL_ADDR, image)?;
                low_limit = low_limit.max(end);
                Some((KERNEL_ADDR, end))
            }
            None => None,
        };

        let dtb = match dtb {
            Some(blob) => {
                Fdt::parse(blob).map_err(BoardError::Dtb)?;
                blob.to_vec()
            }
            None => {
                let mut dt = DeviceTreeConfig::new(RAM_BASE, config.ram_size);
                dt.bootargs = config.bootargs.clone();
                devicetree::virt_device_tree(&dt)
            }
        };
        // As QEMU does: the device tree at the top of RAM, 2 MiB aligned,
        // with fw_dynamic_info just below it.
        let dtb_addr = align_down(ram_end.saturating_sub(dtb.len() as u64), DTB_ALIGN);
        let info_addr = dtb_addr.saturating_sub(0x1000);
        if info_addr < low_limit {
            return Err(BoardError::Layout(format!(
                "no room for the device tree: images end at {low_limit:#x}, RAM ends at {ram_end:#x}"
            )));
        }
        loader::load_raw(&mut bus, dtb_addr, &dtb)?;
        let info: Vec<u8> = [FW_DYNAMIC_MAGIC, FW_DYNAMIC_VERSION, KERNEL_ADDR, NEXT_MODE_S, 0, 0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        loader::load_raw(&mut bus, info_addr, &info)?;

        let mut cpu = Cpu::new(entry);
        cpu.x[10] = 0;
        cpu.x[11] = dtb_addr;
        cpu.x[12] = info_addr;

        let layout = BootLayout {
            entry,
            kernel: kernel_range,
            dtb: (dtb_addr, dtb_addr + dtb.len() as u64),
            fw_dynamic_info: info_addr,
        };
        bus.tick();
        Ok(Self {
            cpu,
            bus,
            profile: Profile::Virt,
            console: Console::Uart(uart),
            clint,
            htif: Htif::disabled(),
            time: config.time,
            insn_carry: 0,
            host_last_ns: None,
            host_carry: 0,
            instret: 0,
            dtb,
            layout,
            resume_stuck_wfi: false,
        })
    }

    /// Builds the bare-metal test profile and loads a test ELF: RAM at
    /// 0x8000_0000, CLINT, UART, syscon, the test interrupt trigger (SIG) at
    /// the PLIC's address, and HTIF at the ELF's `tohost`/`fromhost`
    /// symbols. No PLIC and no device tree. The hart starts at the ELF entry
    /// in M-mode with all registers zero.
    pub fn new_test(config: BoardConfig, elf: &[u8]) -> Result<Self, BoardError> {
        let mut bus = Bus::new(RAM_BASE, config.ram_size as usize);
        let clint = bus.add_device(CLINT_BASE, CLINT_SIZE, Box::new(Clint::new()));
        bus.add_device(SYSCON_BASE, SYSCON_SIZE, Box::new(Syscon::new()));
        let uart = bus.add_device(UART_BASE, UART_SIZE, Box::new(Uart::new(UART_IRQ)));
        bus.add_device(SIG_BASE, SIG_SIZE, Box::new(InterruptTrigger::new()));
        let loaded = loader::load_elf(&mut bus, elf)?;
        let htif = Htif::from_elf(&loaded).unwrap_or_else(Htif::disabled);
        let layout = BootLayout { entry: loaded.entry, kernel: None, dtb: (0, 0), fw_dynamic_info: 0 };
        bus.tick();
        Ok(Self {
            cpu: Cpu::new(loaded.entry),
            bus,
            profile: Profile::Test,
            console: Console::Uart(uart),
            clint,
            htif,
            time: config.time,
            insn_carry: 0,
            host_last_ns: None,
            host_carry: 0,
            instret: 0,
            dtb: Vec::new(),
            layout,
            resume_stuck_wfi: true,
        })
    }

    /// Builds the Spike lockstep profile (see [`spike`]): `firmware` is the
    /// OpenSBI ELF (`fw_jump.elf`), `kernel` goes to 0x8020_0000, and `dtb`
    /// (the same blob given to Spike with `--dtb`) goes into the reset ROM.
    pub fn new_spike(
        config: BoardConfig,
        firmware: &[u8],
        kernel: Option<&[u8]>,
        dtb: &[u8],
    ) -> Result<Self, BoardError> {
        Fdt::parse(dtb).map_err(BoardError::Dtb)?;
        let mut bus = Bus::new(RAM_BASE, config.ram_size as usize);
        let rom = BootRom::spike(RAM_BASE, dtb);
        bus.add_device(SPIKE_ROM_BASE, rom.size(), Box::new(rom));
        let clint = bus.add_device(CLINT_BASE, SPIKE_CLINT_SIZE, Box::new(Clint::new()));
        let soc = bus.add_device(SPIKE_PLIC_BASE, SPIKE_SOC_SIZE, Box::new(SpikeSoc::new()));
        let loaded = loader::load_elf(&mut bus, firmware)?;
        let htif = Htif::from_elf(&loaded).unwrap_or_else(Htif::disabled);
        let kernel_range = match kernel {
            Some(image) => {
                loader::load_raw(&mut bus, KERNEL_ADDR, image)?;
                Some((KERNEL_ADDR, KERNEL_ADDR + image.len() as u64))
            }
            None => None,
        };
        let mut cpu = Cpu::new(SPIKE_ROM_BASE);
        cpu.set_config(CpuConfig::spike());
        bus.tick();
        let rom_dtb = SPIKE_ROM_BASE + 0x20;
        Ok(Self {
            cpu,
            bus,
            profile: Profile::Spike,
            console: Console::Spike(soc),
            clint,
            htif,
            time: TimeSource::Instructions { insns_per_tick: 100 },
            insn_carry: 0,
            host_last_ns: None,
            host_carry: 0,
            instret: 0,
            dtb: dtb.to_vec(),
            layout: BootLayout {
                entry: SPIKE_ROM_BASE,
                kernel: kernel_range,
                dtb: (rom_dtb, rom_dtb + dtb.len() as u64),
                fw_dynamic_info: 0,
            },
            resume_stuck_wfi: false,
        })
    }

    pub fn profile(&self) -> Profile {
        self.profile
    }

    /// Whether HTIF is connected (test profile with a `tohost` symbol).
    pub fn has_htif(&self) -> bool {
        self.htif.is_enabled()
    }

    /// The device tree blob given to the firmware.
    pub fn dtb(&self) -> &[u8] {
        &self.dtb
    }

    pub fn layout(&self) -> BootLayout {
        self.layout
    }

    /// Instructions retired since the board was built.
    pub fn instret(&self) -> u64 {
        self.instret
    }

    /// The machine timer, in 10 MHz ticks.
    pub fn mtime(&self) -> u64 {
        self.bus.wires.mtime
    }

    pub fn power_state(&self) -> PowerState {
        match self.bus.wires.halt {
            None => PowerState::Running,
            Some(Halt::PowerOff) => PowerState::PowerOff,
            Some(Halt::Reboot) => PowerState::Reboot,
            Some(Halt::TestExit(code)) => PowerState::Exit(code),
        }
    }

    /// Whether the hart is stalled in WFI with nothing pending.
    pub fn is_waiting(&self) -> bool {
        self.cpu.waiting
    }

    /// While the hart waits in WFI, the host nanoseconds until the timer
    /// interrupt that could wake it, or `None` if no timer is armed. Console
    /// input can also wake it.
    pub fn ns_until_timer(&mut self) -> Option<u64> {
        let mtime = self.bus.wires.mtime;
        let clint = self.bus.device::<Clint>(self.clint)?;
        if clint.mtimecmp() == u64::MAX {
            return None;
        }
        let ticks = clint.ticks_until_mtip(mtime);
        Some(((u128::from(ticks) * 1_000_000_000) / u128::from(TIMEBASE_HZ)).min(u128::from(u64::MAX)) as u64)
    }

    /// Queues console input for the guest.
    pub fn push_input(&mut self, bytes: &[u8]) {
        match self.console {
            Console::Uart(id) => {
                if let Some(uart) = self.bus.device_mut::<Uart>(id) {
                    uart.push_input(bytes);
                }
            }
            Console::Spike(id) => {
                if let Some(soc) = self.bus.device_mut::<SpikeSoc>(id) {
                    soc.push_input(bytes);
                }
            }
        }
        self.htif.push_console_input(bytes);
        if self.profile != Profile::Spike {
            // Let the UART raise its line now; in lockstep, input waits for
            // the next chunk boundary as in Spike.
            self.bus.tick();
        }
    }

    /// Takes the console output produced since the last call.
    pub fn take_output(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        self.drain_output(&mut out);
        out
    }

    /// Appends the console output produced since the last call to `out`.
    pub fn drain_output(&mut self, out: &mut Vec<u8>) {
        match self.console {
            Console::Uart(id) => {
                if let Some(uart) = self.bus.device_mut::<Uart>(id) {
                    uart.drain_output_into(out);
                }
            }
            Console::Spike(id) => {
                if let Some(soc) = self.bus.device_mut::<SpikeSoc>(id) {
                    soc.drain_output_into(out);
                }
            }
        }
        out.extend(self.htif.take_console_output());
    }

    /// Runs up to `max` instruction steps. Stops early when the guest halts,
    /// or, with host time, when the hart waits in WFI (sleep until
    /// [`Board::ns_until_timer`] or new input, then call again). Returns the
    /// number of instructions retired.
    pub fn run(&mut self, max: u64) -> u64 {
        if self.profile == Profile::Spike {
            return self.run_lockstep(max);
        }
        let start = self.instret;
        let mut steps_done = 0;
        if let TimeSource::Host(_) = self.time {
            self.advance_host_time();
        }
        while steps_done < max && self.bus.wires.halt.is_none() {
            let n = (max - steps_done).min(BATCH);
            let retired = self.cpu.run(&mut self.bus, n);
            self.instret += retired;
            let waiting = self.cpu.waiting;
            let steps = if waiting { retired } else { n };
            steps_done += steps;
            match self.time {
                TimeSource::Instructions { insns_per_tick } => {
                    self.insn_carry += steps;
                    self.bus.wires.mtime = self.bus.wires.mtime.wrapping_add(self.insn_carry / insns_per_tick);
                    self.insn_carry %= insns_per_tick;
                }
                TimeSource::Host(_) => self.advance_host_time(),
            }
            self.bus.tick();
            self.htif.poll_bus(&mut self.bus);
            if waiting && !self.skip_idle_time() {
                if !self.resume_stuck_wfi {
                    break;
                }
                self.cpu.waiting = false;
            }
        }
        self.instret - start
    }

    /// The Spike profile's run loop (`htif_t::run` around `sim_t::step`).
    fn run_lockstep(&mut self, max: u64) -> u64 {
        let start = self.instret;
        let mut steps = 0;
        while steps < max && self.bus.wires.halt.is_none() {
            if !self.htif.check_tohost(&mut self.bus) {
                steps += self.lockstep_chunk();
            }
            self.htif.service(&mut self.bus);
            self.htif.sync_halt(&mut self.bus);
        }
        self.instret - start
    }

    /// One `processor_t::step(INTERLEAVE)` plus the device tick that
    /// follows it. Returns the steps taken (instructions and traps).
    fn lockstep_chunk(&mut self) -> u64 {
        self.cpu.irq_poll = true;
        let mut retired = 0;
        let mut steps = 0;
        while retired < INTERLEAVE {
            steps += 1;
            match self.cpu.step(&mut self.bus) {
                StepResult::Retired => {
                    retired += 1;
                    if self.cpu.wfi_retired {
                        break;
                    }
                }
                StepResult::Trapped(_) | StepResult::Waiting => break,
            }
        }
        self.instret += retired;
        self.cpu.reservation = None;
        self.bus.wires.mtime = self.bus.wires.mtime.wrapping_add(TICKS_PER_CHUNK);
        self.bus.tick();
        steps.max(1)
    }

    /// With instruction time, jumps `mtime` to the next timer interrupt.
    /// Returns false when nothing can wake the hart without host action.
    fn skip_idle_time(&mut self) -> bool {
        if !matches!(self.time, TimeSource::Instructions { .. }) {
            return false;
        }
        let mtime = self.bus.wires.mtime;
        let Some(ticks) = self.bus.device::<Clint>(self.clint).map(|c| c.ticks_until_mtip(mtime)) else {
            return false;
        };
        if ticks == 0 {
            return false;
        }
        self.bus.wires.mtime = mtime.wrapping_add(ticks);
        self.bus.tick();
        true
    }

    fn advance_host_time(&mut self) {
        let TimeSource::Host(clock) = &mut self.time else {
            return;
        };
        let now = clock();
        let last = *self.host_last_ns.get_or_insert(now);
        if now <= last {
            return;
        }
        self.host_last_ns = Some(now);
        let scaled = u128::from(now - last) * u128::from(TIMEBASE_HZ) + self.host_carry;
        self.host_carry = scaled % 1_000_000_000;
        let ticks = (scaled / 1_000_000_000) as u64;
        self.bus.wires.mtime = self.bus.wires.mtime.wrapping_add(ticks);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    const MIB: u64 = 1 << 20;

    fn fake_image(image_size: u64) -> Vec<u8> {
        let mut img = vec![0u8; 4096];
        img[16..24].copy_from_slice(&image_size.to_le_bytes());
        img[48..56].copy_from_slice(b"RISCV\0\0\0");
        img[56..60].copy_from_slice(b"RSC\x05");
        img
    }

    #[test]
    fn boot_registers_and_layout() {
        let fw = vec![0x13u8; 1024];
        let kernel = fake_image(0x197_d000);
        let mut board = Board::new(BoardConfig::default(), &fw, Some(&kernel), None).unwrap();
        let layout = board.layout();
        assert_eq!(board.cpu.pc, RAM_BASE);
        assert_eq!(layout.entry, RAM_BASE);
        assert_eq!(board.cpu.x[10], 0);
        assert_eq!(board.cpu.x[11], 0x8fe0_0000, "2 MiB below the top of 256 MiB");
        assert_eq!(layout.dtb.0, 0x8fe0_0000);
        assert_eq!(board.cpu.x[12], layout.fw_dynamic_info);
        assert_eq!(layout.kernel, Some((KERNEL_ADDR, KERNEL_ADDR + 0x197_d000)));
        assert_eq!(board.bus.read(RAM_BASE, 4), Ok(0x1313_1313));
        assert_eq!(board.bus.read(KERNEL_ADDR + 48, 8), Ok(u64::from_le_bytes(*b"RISCV\0\0\0")));

        let mut dtb = vec![0u8; board.dtb().len()];
        board.bus.read_bytes(layout.dtb.0, &mut dtb).unwrap();
        assert_eq!(dtb, board.dtb());
        let fdt = Fdt::parse(&dtb).unwrap();
        assert_eq!(
            fdt.root.find("/memory@80000000").unwrap().property_cells("reg"),
            Some(vec![0, 0x8000_0000, 0, 0x1000_0000])
        );
        assert_eq!(fdt.root.find("/chosen").unwrap().property_string("bootargs"), Some("console=ttyS0"));
    }

    #[test]
    fn fw_dynamic_info_block() {
        let mut board = Board::new(BoardConfig::default(), &[0; 64], None, None).unwrap();
        let info = board.layout().fw_dynamic_info;
        let word = |b: &mut Board, i: u64| b.bus.read(info + 8 * i, 8).unwrap();
        assert_eq!(word(&mut board, 0), 0x4942_534f);
        assert_eq!(word(&mut board, 1), 2);
        assert_eq!(word(&mut board, 2), KERNEL_ADDR);
        assert_eq!(word(&mut board, 3), 1);
        assert_eq!(word(&mut board, 4), 0);
        assert_eq!(word(&mut board, 5), 0);
    }

    #[test]
    fn custom_dtb_is_used_verbatim_and_validated() {
        let blob = devicetree::virt_device_tree(&DeviceTreeConfig::new(RAM_BASE, 64 * MIB));
        let board = Board::new(BoardConfig::default(), &[0; 64], None, Some(&blob)).unwrap();
        assert_eq!(board.dtb(), &blob[..]);
        let err = Board::new(BoardConfig::default(), &[0; 64], None, Some(b"nope")).err().unwrap();
        assert!(matches!(err, BoardError::Dtb(_)));
    }

    #[test]
    fn images_that_do_not_fit_are_rejected() {
        let config = BoardConfig { ram_size: 16 * MIB, ..BoardConfig::default() };
        let err = Board::new(config, &[0; 64], Some(&fake_image(32 * MIB)), None).err().unwrap();
        assert!(matches!(err, BoardError::Layout(_)), "{err:?}");
        let config = BoardConfig { ram_size: 4 * MIB, ..BoardConfig::default() };
        let err = Board::new(config, &[0; 64], Some(&fake_image(MIB)), None).err().unwrap();
        assert!(matches!(err, BoardError::Layout(_)), "{err:?}");
    }

    #[test]
    fn kernel_footprint_uses_image_header() {
        assert_eq!(kernel_footprint(&fake_image(0x100_0000)), 0x100_0000);
        assert_eq!(kernel_footprint(&[0u8; 100]), 100);
    }

    #[test]
    fn instruction_time_advances_with_steps() {
        let config = BoardConfig { time: TimeSource::Instructions { insns_per_tick: 100 }, ..BoardConfig::default() };
        let mut board = Board::new(config, &[0; 64], None, None).unwrap();
        board.run(25_050);
        assert_eq!(board.mtime(), 250);
        board.run(50);
        assert_eq!(board.mtime(), 251);
    }

    #[test]
    fn host_time_follows_clock() {
        let now = Rc::new(Cell::new(1_000u64));
        let clock = now.clone();
        let config = BoardConfig { time: TimeSource::Host(Box::new(move || clock.get())), ..BoardConfig::default() };
        let mut board = Board::new(config, &[0; 64], None, None).unwrap();
        board.run(10);
        assert_eq!(board.mtime(), 0);
        now.set(1_000 + 1_000_050);
        board.run(10);
        assert_eq!(board.mtime(), 10_000, "1 ms at 10 MHz");
        now.set(1_000 + 1_000_100);
        board.run(10);
        assert_eq!(board.mtime(), 10_001, "sub-tick remainders carry over");
    }

    /// A minimal ELF with one segment holding `code` at 0x8000_0000 and a
    /// `tohost` symbol at 0x8000_1000.
    fn test_elf(code: &[u32]) -> Vec<u8> {
        let code: Vec<u8> = code.iter().flat_map(|w| w.to_le_bytes()).collect();
        let strtab = b"\0tohost\0";
        let mut symtab = vec![0u8; 48];
        symtab[24..28].copy_from_slice(&1u32.to_le_bytes());
        symtab[32..40].copy_from_slice(&0x8000_1000u64.to_le_bytes());
        let code_off = 64 + 56;
        let sym_off = code_off + code.len();
        let str_off = sym_off + symtab.len();
        let sh_off = (str_off + strtab.len() + 7) & !7;
        let mut b = vec![0u8; sh_off + 3 * 64];
        b[0..4].copy_from_slice(b"\x7fELF");
        (b[4], b[5], b[6]) = (2, 1, 1);
        b[16..18].copy_from_slice(&2u16.to_le_bytes());
        b[18..20].copy_from_slice(&243u16.to_le_bytes());
        b[24..32].copy_from_slice(&RAM_BASE.to_le_bytes());
        b[32..40].copy_from_slice(&64u64.to_le_bytes());
        b[40..48].copy_from_slice(&(sh_off as u64).to_le_bytes());
        b[54..56].copy_from_slice(&56u16.to_le_bytes());
        b[56..58].copy_from_slice(&1u16.to_le_bytes());
        b[58..60].copy_from_slice(&64u16.to_le_bytes());
        b[60..62].copy_from_slice(&3u16.to_le_bytes());
        b[64..68].copy_from_slice(&1u32.to_le_bytes());
        b[72..80].copy_from_slice(&(code_off as u64).to_le_bytes());
        b[80..88].copy_from_slice(&RAM_BASE.to_le_bytes());
        b[88..96].copy_from_slice(&RAM_BASE.to_le_bytes());
        b[96..104].copy_from_slice(&(code.len() as u64).to_le_bytes());
        b[104..112].copy_from_slice(&0x2000u64.to_le_bytes());
        b[code_off..sym_off].copy_from_slice(&code);
        b[sym_off..str_off].copy_from_slice(&symtab);
        b[str_off..str_off + strtab.len()].copy_from_slice(strtab);
        let sh1 = sh_off + 64;
        b[sh1 + 4..sh1 + 8].copy_from_slice(&2u32.to_le_bytes());
        b[sh1 + 24..sh1 + 32].copy_from_slice(&(sym_off as u64).to_le_bytes());
        b[sh1 + 32..sh1 + 40].copy_from_slice(&(symtab.len() as u64).to_le_bytes());
        b[sh1 + 40..sh1 + 44].copy_from_slice(&2u32.to_le_bytes());
        let sh2 = sh_off + 128;
        b[sh2 + 4..sh2 + 8].copy_from_slice(&3u32.to_le_bytes());
        b[sh2 + 24..sh2 + 32].copy_from_slice(&(str_off as u64).to_le_bytes());
        b[sh2 + 32..sh2 + 40].copy_from_slice(&(strtab.len() as u64).to_le_bytes());
        b
    }

    /// Writes `value` to tohost (0x8000_1000) from code placed `offset`
    /// bytes into RAM: auipc t0, 1; addi t0, t0, -offset; li t1, value;
    /// sd t1, 0(t0); j .
    fn exit_program(value: u32, offset: u32) -> Vec<u32> {
        let neg = offset.wrapping_neg() & 0xfff;
        vec![0x0000_1297, (neg << 20) | 0x0002_8293, (value << 20) | 0x0000_0313, 0x0062_b023, 0x0000_006f]
    }

    #[test]
    fn test_profile_reports_htif_exit() {
        let config = BoardConfig { ram_size: MIB, ..BoardConfig::default() };
        let mut board = Board::new_test(config, &test_elf(&exit_program(1, 0))).unwrap();
        assert!(board.has_htif());
        board.run(100_000);
        assert_eq!(board.power_state(), PowerState::Exit(0));

        let config = BoardConfig { ram_size: MIB, ..BoardConfig::default() };
        let mut board = Board::new_test(config, &test_elf(&exit_program(7, 0))).unwrap();
        board.run(100_000);
        assert_eq!(board.power_state(), PowerState::Exit(3));
    }

    #[test]
    fn test_profile_maps_sig_and_no_plic() {
        let config = BoardConfig { ram_size: MIB, ..BoardConfig::default() };
        let mut board = Board::new_test(config, &test_elf(&exit_program(1, 0))).unwrap();
        assert_eq!(board.bus.read(SIG_BASE, 4), Ok(0x0001_0001));
        board.bus.write(SIG_BASE + 4, 4, (1 << 31) | (1 << 1)).unwrap();
        assert_eq!(board.bus.wires.test_irq, crate::irq::bits::SSIP);
        assert!(board.bus.read(SIG_BASE + 0x20_0004, 4).is_err(), "no PLIC context registers");
        assert!(board.bus.read(0, 4).is_err(), "address 0 is unmapped");
    }

    #[test]
    fn test_profile_wfi_that_cannot_wake_completes() {
        // wfi; then exit with value 1.
        let mut code = vec![0x1050_0073];
        code.extend(exit_program(1, 4));
        let config = BoardConfig { ram_size: MIB, ..BoardConfig::default() };
        let mut board = Board::new_test(config, &test_elf(&code)).unwrap();
        board.bus.write(CLINT_BASE + 0x4000, 8, u64::MAX).unwrap();
        board.run(100_000);
        assert_eq!(board.power_state(), PowerState::Exit(0));
    }

    fn spike_board(code: &[u32]) -> Board {
        let dtb = devicetree::virt_device_tree(&DeviceTreeConfig::new(RAM_BASE, MIB));
        let fw = test_elf(code);
        let config = BoardConfig { ram_size: MIB, ..BoardConfig::default() };
        Board::new_spike(config, &fw, None, &dtb).unwrap()
    }

    #[test]
    fn spike_profile_starts_in_the_reset_rom() {
        let mut board = spike_board(&[0x0000_006f]);
        assert_eq!(board.profile(), Profile::Spike);
        assert_eq!(board.cpu.pc, 0x1000);
        assert_eq!(board.cpu.config.marchid, 5);
        assert_eq!(board.bus.wires.hart_irq & crate::irq::bits::MTIP, crate::irq::bits::MTIP, "MTIP from reset");
        // The five ROM instructions, then the firmware's `j .`.
        board.run(5);
        assert_eq!(board.cpu.pc, RAM_BASE);
        assert_eq!(board.cpu.x[10], 0);
        assert_eq!(board.cpu.x[11], 0x1020);
    }

    #[test]
    fn spike_chunks_advance_mtime_by_50() {
        let mut board = spike_board(&[0x0000_006f]);
        board.run(12_000);
        // Chunks of 5000 steps: 0..5000, 5000..10000, 10000..15000.
        assert_eq!(board.mtime(), 150);
        assert_eq!(board.instret(), 15_000);
    }

    #[test]
    fn spike_trap_ends_the_chunk() {
        // mtvec = 0x8000_0010 (auipc t0, 0; addi t0, t0, 16; csrw mtvec, t0),
        // then ecall forever: each ecall traps to the handler, which is
        // another ecall.
        let code = [0x0000_0297, 0x0102_8293, 0x3052_9073, 0x0000_0013, 0x0000_0073];
        let mut board = spike_board(&code);
        board.run(5 + 4);
        let before = board.mtime();
        board.run(10);
        assert_eq!(board.mtime() - before, 10 * TICKS_PER_CHUNK, "one chunk per trap");
    }

    #[test]
    fn console_queues_and_power_state() {
        let mut board = Board::new(BoardConfig::default(), &[0; 64], None, None).unwrap();
        assert_eq!(board.power_state(), PowerState::Running);
        board.push_input(b"ls\n");
        assert_eq!(board.bus.read(UART_BASE + 5, 1).map(|lsr| lsr & 1), Ok(1));
        assert_eq!(board.bus.read(UART_BASE, 1), Ok(u64::from(b'l')));
        board.bus.write(UART_BASE, 1, u64::from(b'X')).unwrap();
        assert_eq!(board.take_output(), b"X");
        assert!(board.take_output().is_empty());
        board.bus.write(SYSCON_BASE, 4, 0x5555).unwrap();
        assert_eq!(board.power_state(), PowerState::PowerOff);
        assert_eq!(board.run(1000), 0, "a halted board does not run");
    }
}
