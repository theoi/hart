# hart

hart is a RISC-V computer written from scratch in Rust. It emulates a single-hart RV64GC
machine on a virt-style board and boots unmodified upstream software: OpenSBI firmware, a
Linux LTS kernel, and a BusyBox shell. It runs natively in a terminal, and in a browser tab
through WebAssembly.

**Live demo:** <https://theoi.github.io/hart/> boots Linux to a shell in your browser.

## Features

- **CPU:** RV64IMAFDC (RV64GC) with Zicsr, Zifencei, and Zicntr on one hart.
- **Privileged architecture:** M, S, and U modes; Sv39 virtual memory; 16 PMP entries;
  exceptions and interrupts with delegation to S-mode; the `cycle`, `time`, and `instret`
  counters.
- **Floating point:** a from-scratch IEEE 754 soft-float library for binary32 and binary64,
  built from integer arithmetic only. It covers all five rounding modes, the exception flags,
  fused multiply-add, NaN-boxing, and the RISC-V canonical NaN.
- **Devices:** RAM at `0x80000000`, a CLINT timer, a PLIC, an NS16550A-compatible UART
  console, a syscon power-off and reboot device, and a device tree that hart generates.
- **Boots unmodified OpenSBI and Linux** to an interactive shell.
- **Two front ends:** the native `hart` command, with the guest console in your terminal, and
  a web page that runs the emulator in a Web Worker behind an xterm.js terminal.
- **Written from scratch:** instruction decoding, the CPU, the MMU, PMP, soft-float, the
  devices, and the device-tree writer are all part of this project. `hart-core` has no
  third-party dependencies.
- **Spike-compatible commit log** (`--trace`) for differential testing.

## Quick start

### Native

You need Rust 1.86 or newer.

```sh
git clone https://github.com/theoi/hart.git
cd hart
cargo build --release

# Guest images: OpenSBI (fw_jump.bin) and Linux with its initramfs (Image).
gh release download guest-v1 -R theoi/hart -p fw_jump.bin -p Image -p SHA256SUMS -D images
(cd images && sha256sum --check --ignore-missing SHA256SUMS)

target/release/hart --images images
```

Without the GitHub CLI, download each file with
`curl -LO https://github.com/theoi/hart/releases/download/guest-v1/<file>`.

Linux boots to a `~ # ` prompt. Try `uname -a`, `cat /proc/cpuinfo`, or `mandelbrot`.
Press `Ctrl-a x` to quit; `Ctrl-a h` lists the other console escapes.

Some useful options (`hart --help` lists them all):

| Option | Effect |
| --- | --- |
| `--stats` | Print the instruction count, run time, and speed on exit. |
| `--trace FILE` | Write a Spike `--log-commits` style commit log. |
| `--elf FILE` | Run a bare-metal test ELF on the test profile; exit status 0 means pass. |
| `--timebase insns:N` | Drive the timer from the instruction count, for deterministic runs. |
| `-m MIB` | Set the RAM size (default 256 MiB). guest-v1 needs more than 128 MiB: its fw_jump copies the device tree to `0x88000000`. |

### Web demo

You need Rust with the `wasm32-unknown-unknown` target,
[wasm-pack](https://github.com/wasm-bindgen/wasm-pack) 0.15, and Node 24.

```sh
rustup target add wasm32-unknown-unknown
cd web
npm ci
npm run wasm                            # crates/hart-web -> WebAssembly
npm run images -- --release guest-v1    # fetch, verify, and gzip the guest images
npm run build                           # static site in web/dist
npm run preview                         # serve it at http://localhost:4173/
```

`npm run images -- --dir ../images` uses images you already downloaded. See
[`web/README.md`](web/README.md) for details.

## Repository layout

| Path | Contents |
| --- | --- |
| [`crates/hart-core`](crates/hart-core) | The machine: decoder, CPU, CSRs, MMU and PMP, FP execution and soft-float, bus, devices, device-tree writer, board, and commit-log tracer. |
| [`crates/hart-cli`](crates/hart-cli) | The native `hart` command. |
| [`crates/hart-web`](crates/hart-web) | The wasm-bindgen wrapper the web demo uses. |
| [`web`](web) | The demo site (TypeScript, Vite, xterm.js). |
| [`guest`](guest) | The guest image build: a Buildroot external tree and `build.sh`, which builds OpenSBI, Linux, and the BusyBox root filesystem from pinned sources. |
| [`docs`](docs) | Architecture and implementation choices. |
| [`tests/riscv-tests`](tests/riscv-tests) | Build script and runner for riscv-tests. |
| [`tests/act4`](tests/act4) | hart's configuration and runner for the RISC-V Architectural Tests (ACT4). |
| [`tests/testfloat`](tests/testfloat) | Soft-float conformance against Berkeley TestFloat. |
| [`tests/diff`](tests/diff) | Differential testing against Spike: commit-log comparator and lockstep profile. |
| [`tests/boot`](tests/boot) | Native boot acceptance test. |
| [`tests/web`](tests/web) | Playwright browser tests. |
| [`tests/script`](tests/script) | The command script that the boot and browser tests type into the guest. |
| [`tests/bench`](tests/bench) | Boot speed benchmark. |
| [`.github/workflows`](.github/workflows) | CI and the GitHub Pages deployment. |

## Architecture

hart is an interpreter. `hart-core` models the hart, the memory system (bus, Sv39 MMU, and
PMP), and the devices. It caches decoded instructions per physical page, translates
addresses through separate fetch, load, and store TLBs, and services devices and the timer
between batches of instructions. The `hart` command and the WebAssembly wrapper drive the
same board API.

- [`docs/architecture.md`](docs/architecture.md): the crate and module map, and the core
  interfaces (bus and devices, interrupt lines, CPU, traps, the FP seam, and the tracer).
- [`docs/implementation-choices.md`](docs/implementation-choices.md): every choice the RISC-V
  specifications leave to the implementation, such as WARL fields, misaligned accesses, A/D
  bit handling, and floating-point details.

## How hart was validated

The results below come from the final evaluation of commit
[`3f756f9`](https://github.com/theoi/hart/commit/3f756f95472f84ca4e0139f4c1b71f415181ce2e).
TestFloat ran at commit
[`5978ccd`](https://github.com/theoi/hart/commit/5978ccd7da5c96b2112703b8a47b1d5380ea09f9); the
soft-float and floating-point instruction code (`crates/hart-core/src/softfloat`,
`crates/hart-core/src/cpu/fp`) is unchanged since. Each check can be rerun from this
repository.

| Check | What it shows | Result |
| --- | --- | --- |
| [riscv-tests](https://github.com/riscv-software-src/riscv-tests) | Every rv64ui, rv64um, rv64ua, rv64uc, rv64uf, rv64ud, rv64mi, and rv64si `-p` test at commit `3df703c` passes. | 134 of 134 tests pass. |
| [RISC-V Architectural Tests (ACT4)](https://github.com/riscv-non-isa/riscv-arch-test) | Every test generated for hart's configuration passes. Expected results come from the Sail RISC-V model v0.16. | At `act4` commit `46339ac`, all 592 tests generated for hart's configuration pass, including 32 interrupt and 26 exception tests. All 592 also pass on Sail itself. 39 Sv48 tests are excluded because hart implements Sv39 only. |
| Differential testing against [Spike](https://github.com/riscv-software-src/riscv-isa-sim): randomized programs | hart and Spike (commit `2c300cd`) produce the same per-instruction commit log for randomized bare-metal programs. | The commit logs match for 2,000 freshly generated randomized programs (integer, FP, CSR, traps, Sv39, M/S/U modes, and interrupts), with no CSR excluded from the comparison. |
| Differential testing against Spike: Linux boot | hart and Spike run the guest-v1 boot in lockstep and compare every instruction. | 150,000,000 instructions from reset match, instruction by instruction, through the shell prompt (reached at about 60.8 million instructions). |
| [Berkeley TestFloat](http://www.jhauser.us/arithmetic/TestFloat.html) | The soft-float library matches TestFloat (RISC-V specialization) bit for bit, including the exception flags, in all five rounding modes. | 33,204,430,160 vectors, 0 mismatches in results or flags: level 1 for all 42 functions in all 5 rounding modes; full level 2 for add, sub, mul, div, and sqrt (f32 and f64); and fused multiply-add (all four forms, f32 and f64) on the first 2×10⁹ level-2 vectors in RNE and the first 5×10⁸ in each other mode. |
| Native boot | From a clean checkout with the published images, `tests/boot/boot_test.py` boots hart to the shell and checks the command script's output. | Passes: the prompt appears in 0.42 s under a pseudo-terminal, and the uname, cpuinfo, atan2, and mandelbrot outputs all match. |
| Browser boot | Headless Chromium boots to the shell prompt and runs the command script; Firefox runs the same functional test. | Chromium reaches the prompt in a median of 1.15 s over 5 runs (4-vCPU machine, site served locally), and the full command script passes. Firefox passes too (1.70 s). |

To rerun the checks:

```sh
tests/riscv-tests/build.sh && tests/riscv-tests/run.sh rv64ui rv64um rv64ua rv64uc rv64uf rv64ud rv64mi rv64si
tests/act4/setup.sh && python3 tests/act4/act4.py generate --fast
cargo build --release && python3 tests/act4/act4.py --out act4-results all
tests/testfloat/build.sh && tests/testfloat/run.sh          # level 1; see tests/testfloat/README.md for level 2
tests/boot/boot_test.py                                     # native boot and command script
```

The riscv-tests build needs `riscv64-unknown-elf-gcc` (on Ubuntu, `apt install
gcc-riscv64-unknown-elf`) and `autoconf`. The Spike and browser instructions are in
[`tests/diff`](tests/diff/README.md) and [`tests/web`](tests/web/README.md).
The ACT4 commands need Linux x86_64 and about 8 GB of disk. `setup.sh` installs the pinned
compiler, Sail model, and ACT4 checkout into `~/.cache/hart-act4` (about 2 minutes),
generating the tests takes about 5 minutes, and `all` runs them on Sail and on hart and
writes the per-test results to `act4-results/`; see [`tests/act4`](tests/act4/README.md).

## Guest software

The guest images contain upstream software with no patches written by this project.
[`guest/build.sh`](guest/README.md) builds them from pinned source releases with Buildroot
2026.08.1. Linux and OpenSBI are built from their release tarballs as is; Buildroot's own
stock patches for BusyBox, musl, and GCC are listed in
[`guest/README.md`](guest/README.md#patches):

| Component | Version | License |
| --- | --- | --- |
| OpenSBI (`generic` platform, `fw_jump`) | 1.8.1 | BSD-2-Clause |
| Linux | 6.18.55 (6.18 LTS) | GPL-2.0 with Linux-syscall-note |
| BusyBox | 1.38.0 | GPL-2.0 |
| musl | 1.2.6 | MIT |
| GCC runtime (libgcc) | 15.3.0 | GPL-3.0 with the GCC Runtime Library Exception |

The root filesystem also holds `mandelbrot`, a small demo that renders the Mandelbrot set in
double precision to exercise the FPU (MIT OR Apache-2.0).
[`guest/README.md`](guest/README.md) explains every configuration choice and lists all
licenses.

The [`guest-v1` release](https://github.com/theoi/hart/releases/tag/guest-v1) holds the
images, their checksums, and
[`legal-info.tar`](https://github.com/theoi/hart/releases/download/guest-v1/legal-info.tar)
(346 MB). That bundle has the complete corresponding source and license texts for everything
in the images, plus Buildroot itself and the build scripts. hart's license below does not
cover the guest software.

## License

Copyright (c) 2026 The hart contributors.

hart is licensed under either of

- Apache License, Version 2.0 ([`LICENSE-APACHE`](LICENSE-APACHE)), or
- MIT license ([`LICENSE-MIT`](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in hart by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without
any additional terms or conditions.

The web demo bundles [xterm.js](https://github.com/xtermjs/xterm.js) (MIT). Berkeley
SoftFloat and TestFloat, Spike, the Sail model, riscv-tests, and riscv-arch-test are used only
as test references and are not part of hart.
