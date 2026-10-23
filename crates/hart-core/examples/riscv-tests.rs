//! Developer riscv-tests runner: `riscv-tests [--trace] [--spike] <elf>...`
//!
//! `--spike` uses the Spike-compatible CPU settings (`CpuConfig::spike()`), for
//! running probes whose output is compared with Spike's.
//!
//! Loads each self-checking riscv-tests ELF, runs it on a bare hart (RAM only), and
//! polls HTIF `tohost` the way Spike does. Exit code 0 means pass; otherwise the code is
//! the number of the failing test case.

use hart_core::cpu::{csr, CpuConfig};
use hart_core::devices::htif::Htif;
use hart_core::loader::load_elf;
use hart_core::trace::SpikeCommitLog;
use hart_core::{Bus, Cpu, Halt};
use std::process::ExitCode;

const RAM_BASE: u64 = 0x8000_0000;
const RAM_SIZE: usize = 32 << 20;
const MAX_INSNS: u64 = 50_000_000;
const BATCH: u64 = 5000;

fn run(path: &str, trace: bool, spike: bool) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut bus = Bus::new(RAM_BASE, RAM_SIZE);
    let elf = load_elf(&mut bus, &bytes).map_err(|e| format!("{e:?}"))?;
    let mut htif = Htif::from_elf(&elf).ok_or("no tohost symbol")?;
    let config = if spike { CpuConfig::spike() } else { CpuConfig::default() };
    let mut cpu = Cpu::with_config(elf.entry, config);
    if trace {
        cpu.set_tracer(Some(Box::new(SpikeCommitLog::new(std::io::stderr()))));
    }
    let mut steps = 0;
    while bus.wires.halt.is_none() {
        if steps >= MAX_INSNS {
            return Err(format!("timeout after {steps} steps, pc {:#x}", cpu.pc));
        }
        cpu.irq_poll = true;
        cpu.run(&mut bus, BATCH);
        steps += BATCH;
        htif.poll_bus(&mut bus);
        // A WFI wakes on any locally enabled pending interrupt, even a globally masked one.
        if bus.wires.halt.is_none() && cpu.waiting && csr::mip(&cpu, &bus) & cpu.csr.mie == 0 {
            return Err(format!("stuck in WFI at pc {:#x}", cpu.pc));
        }
    }
    match bus.wires.halt {
        Some(Halt::TestExit(0)) => Ok(()),
        Some(Halt::TestExit(n)) => Err(format!("failed test {n}")),
        other => Err(format!("unexpected halt {other:?}")),
    }
}

fn main() -> ExitCode {
    let mut trace = false;
    let mut spike = false;
    let mut paths = Vec::new();
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--trace" => trace = true,
            "--spike" => spike = true,
            _ => paths.push(a),
        }
    }
    if paths.is_empty() {
        eprintln!("usage: riscv-tests [--trace] [--spike] <elf>...");
        return ExitCode::from(2);
    }
    let mut failed = 0;
    for p in &paths {
        let name = std::path::Path::new(p).file_name().map_or(p.clone(), |n| n.to_string_lossy().into_owned());
        match run(p, trace, spike) {
            Ok(()) => println!("PASS {name}"),
            Err(e) => {
                failed += 1;
                println!("FAIL {name}: {e}");
            }
        }
    }
    println!("{} passed, {} failed", paths.len() - failed, failed);
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
