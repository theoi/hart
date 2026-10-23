# hart

hart is a RISC-V computer written from scratch in Rust. It emulates a single-hart RV64GC
machine on a virt-style board and boots unmodified upstream software: OpenSBI firmware, a
Linux LTS kernel, and a BusyBox shell. It runs natively in a terminal, and in a browser tab
through WebAssembly.

**Live demo:** <https://theoi.github.io/hart/> <!-- RESULT: confirm the live demo URL once GitHub Pages is enabled -->

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
| `-m MIB` | Set the RAM size (default 256 MiB). |

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
| [`docs`](docs) | Architecture and implementation choices. |
| [`tests/riscv-tests`](tests/riscv-tests) | Build script and runner for riscv-tests. |
| [`tests/testfloat`](tests/testfloat) | Soft-float conformance against Berkeley TestFloat. |
| [`tests/diff`](tests/diff) | Differential testing against Spike: commit-log comparator and lockstep profile. |
| [`tests/boot`](tests/boot) | Native boot acceptance test. |
| [`tests/web`](tests/web) | Playwright browser tests. |
| [`tests/script`](tests/script) | The command script that the boot and browser tests type into the guest. |
| [`tests/bench`](tests/bench) | Boot speed benchmark. |
| [`.github/workflows`](.github/workflows) | CI and the GitHub Pages deployment. |

<!-- TODO: add guest/ (guest image build) and tests/act4 (ACT4 config and runner) when they land. -->

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
<!-- RESULT: evaluated commit SHA -->. Each check can be rerun from this repository.

| Check | What it shows | Result |
| --- | --- | --- |
| [riscv-tests](https://github.com/riscv-software-src/riscv-tests) | Every rv64ui, rv64um, rv64ua, rv64uc, rv64uf, rv64ud, rv64mi, and rv64si `-p` test at commit `3df703c` passes. | <!-- RESULT: tests passed / total --> |
| [RISC-V Architectural Tests (ACT4)](https://github.com/riscv-non-isa/riscv-arch-test) | Every test generated for hart's configuration passes. Expected results come from the Sail RISC-V model v0.16. | <!-- RESULT: ACT4 commit; tests passed / generated; excluded groups and reasons --> |
| Differential testing against [Spike](https://github.com/riscv-software-src/riscv-isa-sim) | hart and Spike (commit `2c300cd`) produce the same per-instruction commit log for randomized programs and for the Linux boot. | <!-- RESULT: programs and instructions compared; boot comparison scope and instruction count; divergences --> |
| [Berkeley TestFloat](http://www.jhauser.us/arithmetic/TestFloat.html) | The soft-float library matches TestFloat bit for bit, including the exception flags, in all five rounding modes. | <!-- RESULT: vectors per level; mismatches --> |
| Native boot | From a clean checkout with the published images, hart boots to the shell and the command script produces the expected output. | <!-- RESULT: pass/fail; boot time --> |
| Browser boot | Headless Chromium boots to the shell prompt and runs the command script; Firefox runs the same functional test. | <!-- RESULT: Chromium median boot time over N runs; Firefox result --> |
| Independent review | A reviewer who did not build or validate hart checked the deliverables against the project's requirements. | <!-- RESULT: review outcome --> |

To rerun the checks:

```sh
tests/riscv-tests/build.sh && tests/riscv-tests/run.sh rv64ui rv64um rv64ua rv64uc rv64uf rv64ud rv64mi rv64si
tests/testfloat/build.sh && tests/testfloat/run.sh          # level 1; see tests/testfloat/README.md for level 2
tests/boot/boot_test.py                                     # native boot and command script
```

The riscv-tests build needs `riscv64-unknown-elf-gcc` (on Ubuntu, `apt install
gcc-riscv64-unknown-elf`) and `autoconf`. The Spike and browser instructions are in
[`tests/diff`](tests/diff/README.md) and [`tests/web`](tests/web/README.md).
<!-- RESULT: add the ACT4 rerun command when tests/act4 lands -->

## Guest software

The guest images contain unmodified upstream software, built from source with a pinned
Buildroot release: OpenSBI (generic platform, `fw_jump`), a Linux LTS kernel, BusyBox, and
the musl C library, plus a small Mandelbrot renderer that exercises the FPU.
<!-- RESULT: exact versions of Buildroot, OpenSBI, Linux, BusyBox, and musl -->

The images, their checksums, and the complete corresponding source with license notices are
published as assets of the
[`guest-v1` release](https://github.com/theoi/hart/releases/tag/guest-v1).
<!-- RESULT: direct link to the guest-v1 source bundle and license notices -->
Each component keeps its own license: Linux and BusyBox are GPL-2.0, musl is MIT, and
OpenSBI is BSD-2-Clause. hart's license below does not cover the guest software.

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
