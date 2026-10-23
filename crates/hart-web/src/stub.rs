//! A scripted stand-in for the emulator, used to develop the web front end.
//!
//! It replays a recorded-style OpenSBI and Linux boot log at a pace tied to a
//! synthetic instruction count, then runs a tiny shell that echoes input and
//! answers a few commands. It burns real CPU time per "instruction" so the
//! worker's time slicing and speed readout behave like they will with the
//! real machine.

use std::collections::VecDeque;

use crate::RunState;

/// Busy-work rounds per synthetic instruction, tuned for roughly 100 MIPS.
const ROUNDS_PER_INSN: u32 = 8;

const OPENSBI_LOG: &str = r"
OpenSBI v1.8.1
   ____                    _____ ____ _____
  / __ \                  / ____|  _ \_   _|
 | |  | |_ __   ___ _ __ | (___ | |_) || |
 | |  | | '_ \ / _ \ '_ \ \___ \|  _ < | |
 | |__| | |_) |  __/ | | |____) | |_) || |_
  \____/| .__/ \___|_| |_|_____/|____/_____|
        | |
        |_|

Platform Name               : hart,virt
Platform Features           : medeleg
Platform HART Count         : 1
Platform IPI Device         : aclint-mswi
Platform Timer Device       : aclint-mtimer @ 10000000Hz
Platform Console Device     : uart8250
Platform HSM Device         : ---
Platform PMU Device         : ---
Platform Reboot Device      : syscon-reboot
Platform Shutdown Device    : syscon-poweroff
Firmware Base               : 0x80000000
Firmware Size               : 327 KB
Runtime SBI Version         : 3.0

Domain0 Name                : root
Domain0 Boot HART           : 0
Domain0 HARTs               : 0*
Domain0 Next Address        : 0x0000000080200000
Domain0 Next Arg1           : 0x0000000088000000
Domain0 Next Mode           : S-mode
Domain0 SysReset            : yes

Boot HART ID                : 0
Boot HART Domain            : root
Boot HART Priv Version      : v1.12
Boot HART Base ISA          : rv64imafdc
Boot HART ISA Extensions    : zicntr
Boot HART PMP Count         : 16
Boot HART PMP Granularity   : 2 bits
Boot HART MIDELEG           : 0x0000000000000222
Boot HART MEDELEG           : 0x000000000000b109
";

const KERNEL_LOG: &str = "\
[    0.000000] Linux version 6.18.58 (hart@buildroot) (riscv64-buildroot-linux-musl-gcc (Buildroot 2026.08.1) 14.3.0, GNU ld (GNU Binutils) 2.44) #1 Fri Oct 23 07:05:38 UTC 2026
[    0.000000] Machine model: hart,virt
[    0.000000] SBI specification v3.0 detected
[    0.000000] SBI implementation ID=0x1 Version=0x10008
[    0.000000] SBI TIME extension detected
[    0.000000] SBI IPI extension detected
[    0.000000] SBI RFENCE extension detected
[    0.000000] SBI SRST extension detected
[    0.000000] OF: reserved mem: 0x0000000080000000..0x000000008003ffff (256 KiB) nomap non-reusable mmode_resv0@80000000
[    0.000000] Zone ranges:
[    0.000000]   DMA32    [mem 0x0000000080000000-0x0000000087ffffff]
[    0.000000]   Normal   empty
[    0.000000] Initmem setup node 0 [mem 0x0000000080000000-0x0000000087ffffff]
[    0.000000] riscv: base ISA extensions acdfim
[    0.000000] riscv: ELF capabilities acdfim
[    0.000000] Kernel command line: console=ttyS0
[    0.000000] Dentry cache hash table entries: 16384 (order: 5, 131072 bytes, linear)
[    0.000000] Inode-cache hash table entries: 8192 (order: 4, 65536 bytes, linear)
[    0.000000] Built 1 zonelists, mobility grouping on.  Total pages: 32768
[    0.000000] SLUB: HWalign=64, Order=0-3, MinObjects=0, CPUs=1, Nodes=1
[    0.000000] NR_IRQS: 64, nr_irqs: 64, preallocated irqs: 0
[    0.000000] riscv-intc: 64 local interrupts mapped
[    0.000000] plic: plic@c000000: mapped 96 interrupts with 1 handlers for 2 contexts.
[    0.000000] clocksource: riscv_clocksource: mask: 0xffffffffffffffff max_cycles: 0x24e6a1710, max_idle_ns: 440795202120 ns
[    0.000062] sched_clock: 64 bits at 10MHz, resolution 100ns, wraps every 4398046511100ns
[    0.001187] Console: colour dummy device 80x25
[    0.001544] Calibrating delay loop (skipped), value calculated using timer frequency.. 20.00 BogoMIPS (lpj=40000)
[    0.002093] pid_max: default: 32768 minimum: 301
[    0.003410] Mount-cache hash table entries: 512 (order: 0, 4096 bytes, linear)
[    0.013210] ASID allocator disabled (0 bits)
[    0.022690] devtmpfs: initialized
[    0.031512] clocksource: jiffies: mask: 0xffffffff max_cycles: 0xffffffff, max_idle_ns: 7645041785100000 ns
[    0.032301] futex hash table entries: 256 (order: 2, 16384 bytes, linear)
[    0.036719] NET: Registered PF_NETLINK/PF_ROUTE protocol family
[    0.090111] clocksource: Switched to clocksource riscv_clocksource
[    0.120774] NET: Registered PF_INET protocol family
[    0.141822] Trying to unpack rootfs image as initramfs...
[    0.162337] workingset: timestamp_bits=62 max_order=15 bucket_order=0
[    0.211603] Serial: 8250/16550 driver, 4 ports, IRQ sharing disabled
[    0.216911] 10000000.serial: ttyS0 at MMIO 0x10000000 (irq = 10, base_baud = 230400) is a 16550A
[    0.217732] printk: legacy console [ttyS0] enabled
[    0.342780] Freeing initrd memory: 6492K
[    0.381040] clk: Disabling unused clocks
[    0.392851] Freeing unused kernel image (initmem) memory: 2208K
[    0.393914] Run /init as init process
";

const USER_LOG: &str = "\
Starting syslogd: OK
Starting klogd: OK
Running sysctl: OK
Saving 256 bits of non-creditable seed for next boot
Starting network: OK
Starting crond: OK
";

const PROMPT: &str = "~ # ";
const BANNER: &str = "\r\nWelcome to hart!\r\n";

const CPUINFO: &str = "\
processor\t: 0
hart\t\t: 0
isa\t\t: rv64imafdc_zicntr_zicsr_zifencei_zihpm_zaamo_zalrsc_zca_zcd
mmu\t\t: sv39
mvendorid\t: 0x0
marchid\t\t: 0x0
mimpid\t\t: 0x0
hart isa\t: rv64imafdc_zicntr_zicsr_zifencei_zihpm_zaamo_zalrsc_zca_zcd

";

enum Event {
    Output(Vec<u8>),
    ShellReady,
    PowerOff,
    Reboot,
}

pub struct StubMachine {
    instret: u64,
    rng: u64,
    events: VecDeque<(u64, Event)>,
    out: Vec<u8>,
    input: VecDeque<u8>,
    line: Vec<u8>,
    shell_ready: bool,
    in_escape: bool,
    state: RunState,
}

/// Converts `\n` to `\r\n`, as the guest tty's `onlcr` setting does.
fn crlf(s: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(s.len() + s.len() / 16);
    for b in s.bytes() {
        if b == b'\n' {
            v.push(b'\r');
        }
        v.push(b);
    }
    v
}

fn kernel_timestamp(line: &str) -> Option<f64> {
    let inner = line.strip_prefix('[')?.split(']').next()?;
    inner.trim().parse().ok()
}

/// The guest's demo output, so the stub passes the shared command script.
const MANDELBROT: &str = include_str!("../../../tests/script/expected/mandelbrot.txt");

impl StubMachine {
    pub const NAME: &'static str = "stub";

    pub fn new(
        firmware: &[u8],
        kernel: &[u8],
        _dtb: Option<&[u8]>,
        ram_mib: u32,
        _bootargs: Option<&str>,
    ) -> Result<Self, String> {
        if firmware.is_empty() || kernel.is_empty() {
            return Err("firmware and kernel images must not be empty".into());
        }
        if ram_mib < 16 {
            return Err(format!("{ram_mib} MiB of RAM is too little to boot Linux"));
        }
        let mut events = VecDeque::new();
        let mut at = 2_000_000u64;
        for line in OPENSBI_LOG.lines() {
            at += 40_000;
            events.push_back((at, Event::Output(crlf(&format!("{line}\n")))));
        }
        let kernel_base = at + 20_000_000;
        for line in KERNEL_LOG.lines() {
            let ts = kernel_timestamp(line).unwrap_or(0.0);
            at = at.max(kernel_base + (ts * 600_000_000.0) as u64) + 20_000;
            events.push_back((at, Event::Output(crlf(&format!("{line}\n")))));
        }
        for line in USER_LOG.lines() {
            at += 9_000_000;
            events.push_back((at, Event::Output(crlf(&format!("{line}\n")))));
        }
        at += 2_000_000;
        let mut tail = BANNER.as_bytes().to_vec();
        tail.extend_from_slice(PROMPT.as_bytes());
        events.push_back((at, Event::Output(tail)));
        events.push_back((at, Event::ShellReady));
        Ok(StubMachine {
            instret: 0,
            rng: 0x9e37_79b9_7f4a_7c15 ^ kernel.len() as u64,
            events,
            out: Vec::new(),
            input: VecDeque::new(),
            line: Vec::new(),
            shell_ready: false,
            in_escape: false,
            state: RunState::Running,
        })
    }

    pub fn run(&mut self, max_insns: u32, _now_ms: f64) -> u32 {
        if matches!(self.state, RunState::PoweredOff | RunState::RebootRequested) {
            return 0;
        }
        let max = u64::from(max_insns);
        let mut done = 0u64;
        while done < max {
            if self.shell_ready && !self.input.is_empty() {
                self.process_input();
            }
            let Some(&(target, _)) = self.events.front() else { break };
            let chunk = (max - done).min(target.saturating_sub(self.instret));
            self.burn(chunk);
            done += chunk;
            self.instret += chunk;
            self.release_due_events();
            if matches!(self.state, RunState::PoweredOff | RunState::RebootRequested) {
                break;
            }
        }
        if self.state == RunState::Running || self.state == RunState::Idle {
            self.state =
                if self.events.is_empty() && self.input.is_empty() { RunState::Idle } else { RunState::Running };
        }
        done as u32
    }

    pub fn push_input(&mut self, bytes: &[u8]) {
        self.input.extend(bytes.iter().copied());
        if self.state == RunState::Idle {
            self.state = RunState::Running;
        }
    }

    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    pub fn instret(&self) -> u64 {
        self.instret
    }

    pub fn state(&self) -> RunState {
        self.state
    }

    pub fn idle_ms(&mut self) -> f64 {
        -1.0
    }

    fn burn(&mut self, insns: u64) {
        let mut x = self.rng;
        for _ in 0..insns * u64::from(ROUNDS_PER_INSN) {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
        }
        self.rng = std::hint::black_box(x);
    }

    fn release_due_events(&mut self) {
        while let Some(&(at, _)) = self.events.front() {
            if at > self.instret {
                break;
            }
            let (_, ev) = self.events.pop_front().expect("front exists");
            match ev {
                Event::Output(bytes) => self.out.extend_from_slice(&bytes),
                Event::ShellReady => self.shell_ready = true,
                Event::PowerOff => self.state = RunState::PoweredOff,
                Event::Reboot => self.state = RunState::RebootRequested,
            }
        }
    }

    fn schedule(&mut self, delay: u64, ev: Event) {
        let at = self.events.back().map_or(self.instret, |e| e.0).max(self.instret) + delay;
        self.events.push_back((at, ev));
    }

    fn process_input(&mut self) {
        while let Some(b) = self.input.pop_front() {
            if self.in_escape {
                // Swallow CSI sequences such as arrow keys up to the final byte.
                if (0x40..=0x7e).contains(&b) && b != b'[' {
                    self.in_escape = false;
                }
                continue;
            }
            match b {
                0x1b => self.in_escape = true,
                b'\r' | b'\n' => {
                    self.out.extend_from_slice(b"\r\n");
                    let line = String::from_utf8_lossy(&std::mem::take(&mut self.line)).into_owned();
                    self.execute(line.trim());
                    // The shell reads no more input until the command finishes.
                    return;
                }
                0x7f | 0x08 => {
                    if self.line.pop().is_some() {
                        self.out.extend_from_slice(b"\x08 \x08");
                    }
                }
                0x03 => {
                    self.line.clear();
                    self.out.extend_from_slice(b"^C\r\n");
                    self.out.extend_from_slice(PROMPT.as_bytes());
                }
                0x15 => {
                    for _ in 0..self.line.len() {
                        self.out.extend_from_slice(b"\x08 \x08");
                    }
                    self.line.clear();
                }
                0x20..=0x7e => {
                    self.line.push(b);
                    self.out.push(b);
                }
                _ => {}
            }
        }
    }

    fn execute(&mut self, cmd: &str) {
        let reply = |s: &str| Event::Output(crlf(s));
        let (cost, text): (u64, String) = match cmd {
            "" => (10_000, String::new()),
            "uname -a" => {
                (400_000, "Linux hart 6.18.58 #1 SMP Fri Oct 23 07:05:38 UTC 2026 riscv64 GNU/Linux\n".into())
            }
            "uname" => (300_000, "Linux\n".into()),
            "cat /proc/cpuinfo" => (600_000, CPUINFO.into()),
            "awk 'BEGIN{print atan2(0,-1)}'" => (900_000, "3.14159\n".into()),
            "mandelbrot" => (40_000_000, MANDELBROT.into()),
            "help" => (
                200_000,
                "This is the stub machine. Try: uname -a, cat /proc/cpuinfo, mandelbrot, poweroff, reboot\n".into(),
            ),
            "clear" => (100_000, "\x1b[H\x1b[2J".into()),
            "poweroff" | "reboot" => {
                let reboot = cmd == "reboot";
                self.schedule(
                    5_000_000,
                    reply(
                        "The system is going down NOW!\nSent SIGTERM to all processes\nSent SIGKILL to all processes\n",
                    ),
                );
                let last = if reboot { "reboot: Restarting system\n" } else { "reboot: Power down\n" };
                self.schedule(20_000_000, reply(&format!("[   42.000000] {last}")));
                self.schedule(1_000, if reboot { Event::Reboot } else { Event::PowerOff });
                return;
            }
            _ if cmd.starts_with("echo ") => (200_000, format!("{}\n", &cmd[5..])),
            _ => {
                let name = cmd.split_whitespace().next().unwrap_or(cmd);
                (300_000, format!("-sh: {name}: not found\n"))
            }
        };
        let mut bytes = crlf(&text);
        bytes.extend_from_slice(PROMPT.as_bytes());
        self.schedule(cost, Event::Output(bytes));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boot() -> StubMachine {
        let mut m = StubMachine::new(&[1], &[1], None, 128, None).unwrap();
        let mut out = Vec::new();
        while !m.shell_ready {
            m.run(50_000_000, 0.0);
            out.extend(m.take_output());
        }
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("OpenSBI v1.8.1"));
        assert!(text.ends_with("Starting crond: OK\r\n\r\nWelcome to hart!\r\n~ # "));
        m
    }

    fn command(m: &mut StubMachine, input: &str) -> String {
        m.push_input(input.as_bytes());
        let mut out = Vec::new();
        for _ in 0..100 {
            m.run(50_000_000, 0.0);
            out.extend(m.take_output());
            if m.state() == RunState::Idle {
                break;
            }
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn echoes_and_answers_commands() {
        let mut m = boot();
        assert_eq!(m.state(), RunState::Idle);
        let out = command(&mut m, "uname -a\r");
        assert!(out.starts_with("uname -a\r\nLinux hart 6.18.58"));
        assert!(out.ends_with("riscv64 GNU/Linux\r\n~ # "));
        let out = command(&mut m, "ab\x7fc\r");
        assert_eq!(out, "ab\x08 \x08c\r\n-sh: ac: not found\r\n~ # ");
    }

    #[test]
    fn poweroff_stops_the_machine() {
        let mut m = boot();
        let out = command(&mut m, "poweroff\r");
        assert!(out.contains("reboot: Power down"));
        assert_eq!(m.state(), RunState::PoweredOff);
        assert_eq!(m.run(1000, 0.0), 0);
    }
}
