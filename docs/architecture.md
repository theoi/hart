# hart architecture

hart is a single-hart RV64GC (IMAFDC + Zicsr, Zifencei, Zicntr) machine with M/S/U modes,
Sv39, and 16 PMP entries, plus a virt-style board. This document describes the crate
layout, the core interfaces, and which builder owns each module.

## Crates

| Crate | Purpose | Owner |
| --- | --- | --- |
| `crates/hart-core` | CPU, MMU, PMP, FPU, bus, devices, FDT, board, tracer. **No third-party dependencies.** | shared, per module below |
| `crates/hart-cli` | The native `hart` binary (terminal console, image loading, trace options). | platform |
| `crates/hart-web` | wasm-bindgen wrapper used by the site in `web/`. | web |

## hart-core modules

| Module | Contents | Owner |
| --- | --- | --- |
| `bus` | `Bus` (RAM + MMIO dispatch), `Device` trait, `Ram`, `DeviceId`, `BusFault` | CPU |
| `irq` | `Wires`: interrupt lines, PLIC source levels, `mtime`, halt requests | CPU |
| `cpu` | `Cpu` state, `step`/`run`, memory access helpers, trap entry | CPU |
| `cpu::decode`, `cpu::exec` | decoder (including all C encodings) and integer/system execution | CPU |
| `cpu::csr` | CSR file, counters, delegation, interrupt selection, xRET | CPU |
| `cpu::mmu`, `cpu::pmp` | Sv39 + software TLB, PMP | CPU |
| `cpu::trap` | `Exception`, `ExceptionCause`, `Interrupt`, `Trap` | CPU |
| `cpu::fp` | **FP seam**: `FpState` (f registers, frm, fflags), `execute`, FP CSRs | FPU |
| `softfloat` | IEEE 754 binary32/binary64 arithmetic | FPU |
| `trace` | Spike `--log-commits` format commit log (`Commit`, `TraceSink`, `SpikeCommitLog`) | CPU |
| `devices` | CLINT, PLIC, NS16550A UART, syscon, HTIF (`devices::htif`) | platform |
| `loader` | ELF and raw image loading | platform |
| `fdt` | device-tree blob writer | platform |
| `board` | memory map, device wiring, run loop | platform |

Implementation choices that the spec leaves open are recorded in
[`implementation-choices.md`](implementation-choices.md).

## Interfaces

### Bus and devices

```rust
pub trait Device: Any {
    fn read(&mut self, offset: u64, size: u8, wires: &mut Wires) -> Result<u64, BusFault>;
    fn write(&mut self, offset: u64, size: u8, value: u64, wires: &mut Wires) -> Result<(), BusFault>;
    fn tick(&mut self, _wires: &mut Wires) {}
}
```

- `Bus::new(ram_base, ram_size)` creates RAM; `Bus::add_device(base, size, Box<dyn Device>)`
  maps a device and returns a `DeviceId`. `device_mut::<T>(id)` / `find_device_mut::<T>()`
  give typed access (for example, to drain the UART's output buffer).
- `Bus::read/write(paddr, size, ..)` are physical accesses. RAM is checked first and accepts
  any alignment. Device accesses must be naturally aligned and fully inside the region,
  otherwise they fail with `BusFault`, which the CPU reports as an access fault.
- Offsets passed to devices are relative to the device base. `size` is 1, 2, 4, or 8.
- `Bus::write_bytes/read_bytes` copy images into and out of RAM.
- `Bus::tick()` calls `tick` on every device. The board calls it between instruction batches.

### Interrupt lines and time (`Wires`)

Devices never hold references to the CPU or each other. Every device call gets
`&mut Wires`:

- `hart_irq`: lines into the hart in `mip` layout. CLINT drives MSIP/MTIP, the PLIC drives
  MEIP/SEIP. The CPU ORs these into `mip`, so software sees `mip = csr.mip | hart_irq`.
- `irq_sources`: PLIC input levels, bit `n` = source `n`. The UART raises its source; the
  PLIC reads the levels (on its own accesses and on `tick`) and drives MEIP/SEIP.
- `mtime`: the machine timer. The `time` CSR reads it. The board's run loop advances it;
  the CLINT compares it with `mtimecmp`.
- `halt`: set by syscon or HTIF to stop the run loop (`PowerOff`, `Reboot`, `TestExit(code)`).

### CPU

```rust
let mut cpu = Cpu::new(reset_pc);       // M-mode, all state zero
cpu.x[10] = hartid; cpu.x[11] = dtb_addr;
let retired = cpu.run(&mut bus, 10_000); // stops early in WFI
if cpu.waiting { /* fast-forward mtime to the next timer event */ }
```

- `Cpu::step(&mut bus) -> StepResult` takes a pending interrupt or executes one instruction.
- Interrupts are sampled before every instruction from `csr.mie`, `csr.mip`, and
  `bus.wires.hart_irq`, so a device line change takes effect on the next step.
- Public state: `x`, `pc`, `mode`, `csr`, `fp`, `mmu`, `pmp`, `reservation`, `waiting`.
- Memory helpers: `cpu.load(bus, vaddr, size)`, `cpu.store(bus, vaddr, size, value)`,
  `cpu.translate(bus, vaddr, size, access)`. They translate through Sv39, check PMP, support
  misaligned addresses, and record trace entries.

### Traps

`Exception { cause: ExceptionCause, tval }`, `Interrupt` (cause code = `mip` bit), and
`Trap::{Exception, Interrupt}`. Execution functions return `Result<_, Exception>`;
`Cpu::take_trap` performs delegation (`medeleg`/`mideleg`) and trap entry.

### FP seam (`cpu::fp`)

- The decoder dispatches LOAD-FP, STORE-FP, OP-FP, and the four FMA opcodes to
  `fp::execute(cpu, bus, insn32)`. C.FLD/C.FSD/C.FLDSP/C.FSDSP are expanded to 32-bit form
  first. `execute` must not change `pc`; the CPU advances it on `Ok`.
- `fp::read_csr`/`fp::write_csr` implement `fflags`, `frm`, and `fcsr`; the CSR file
  dispatches to them.
- Hooks: `cpu.mstatus_fs()`, `cpu.set_mstatus_fs(fs)` (SD is derived), `cpu.set_f(rd, bits)`,
  `cpu.f(rs)`, `cpu.x(rs)`, `cpu.set_x(rd, v)`, `cpu.load`, `cpu.store`, `cpu.log_csr_write`.
- `misa` reports F and D.

### Commit-log tracer

`cpu.set_tracer(Some(Box::new(SpikeCommitLog::new(writer))))` writes one line per retired
instruction in Spike's `--log-commits` format: mode, pc, instruction bits, register writes
(`xN`, `fN`, `cNNN_name`), load addresses, and store addresses with data. Addresses are
physical. Custom sinks implement `TraceSink` (`commit` per instruction, `trap` per trap).

## Branch and CI workflow

Work happens on short-lived branches, rebased onto `main` and fast-forwarded once
`cargo build` and `cargo test` pass. CI (`.github/workflows/ci.yml`) runs build, test, and a
wasm32 build on pushes to `main` and on pull requests.
