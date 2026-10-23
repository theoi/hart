//! Developer riscv-tests runner: `riscv-tests [--trace] <elf>...`
//!
//! Loads each self-checking riscv-tests ELF, runs it on a bare hart (RAM only), and
//! polls HTIF `tohost` the way Spike does. Exit code 0 means pass; otherwise the code is
//! the number of the failing test case.

use hart_core::devices::htif::Htif;
use hart_core::loader::load_elf;
use hart_core::trace::SpikeCommitLog;
use hart_core::{Bus, Cpu, Halt};
use std::process::ExitCode;

const RAM_BASE: u64 = 0x8000_0000;
const RAM_SIZE: usize = 32 << 20;
const MAX_INSNS: u64 = 50_000_000;
const BATCH: u64 = 5000;

fn run(path: &str, trace: bool) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut bus = Bus::new(RAM_BASE, RAM_SIZE);
    let elf = load_elf(&mut bus, &bytes).map_err(|e| format!("{e:?}"))?;
    let mut htif = Htif::from_elf(&elf).ok_or("no tohost symbol")?;
    let mut cpu = Cpu::new(elf.entry);
    if trace {
        cpu.set_tracer(Some(Box::new(SpikeCommitLog::new(std::io::stderr()))));
    }
    let mut steps = 0;
    while bus.wires.halt.is_none() {
        if steps >= MAX_INSNS {
            return Err(format!("timeout after {steps} steps, pc {:#x}", cpu.pc));
        }
        cpu.run(&mut bus, BATCH);
        steps += BATCH;
        htif.poll_bus(&mut bus);
        if bus.wires.halt.is_none() && cpu.waiting && cpu.pending_interrupt(&bus).is_none() {
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
    let mut paths = Vec::new();
    for a in std::env::args().skip(1) {
        if a == "--trace" {
            trace = true;
        } else {
            paths.push(a);
        }
    }
    if paths.is_empty() {
        eprintln!("usage: riscv-tests [--trace] <elf>...");
        return ExitCode::from(2);
    }
    let mut failed = 0;
    for p in &paths {
        let name = std::path::Path::new(p).file_name().map_or(p.clone(), |n| n.to_string_lossy().into_owned());
        match run(p, trace) {
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
