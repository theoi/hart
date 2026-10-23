# ACT4: official RISC-V architectural tests for hart

This directory is the instrument for promise acceptance requirement 2. It generates the
official RISC-V architectural certification tests (riscv-arch-test, `act4` branch) for a
hart configuration, has the Sail reference model compute the expected results, and runs
every resulting self-checking ELF on `hart --elf`.

| File | Purpose |
| --- | --- |
| `config/hart-rv64gc/` | ACT4 DUT configuration, derived from `config/sail/sail-RVA20S64` at the pin: `test_config.yaml`, UDB config `hart-rv64gc.yaml`, Sail config `sail.json`, `rvmodel_macros.h`, `link.ld` |
| `exclusions.tsv` | Every upstream sail-RVA20S64 test that is not generated for hart, with the reason |
| `setup.sh` | Installs the pinned toolchain into `$ACT4_HOME` (default `~/.cache/hart-act4`) |
| `act4.py` | Generates the ELFs and runs them on Sail and hart; writes per-test tables |

## Pins

| Component | Pin |
| --- | --- |
| riscv-arch-test | `act4` branch, commit `46339ac578a5ff9214ef74175344687456659d90` |
| Sail RISC-V model | 0.16, prebuilt `sail-riscv-Linux-x86_64.tar.gz` (sha256 `f9bb8a32…6bee`) |
| Compiler | riscv-collab prebuilt `riscv64-elf-ubuntu-24.04-gcc.tar.xz`, release 2026.10.21: GCC 16.1.0, binutils 2.47 (sha256 `c7d08d4c…bfd2`) |
| mise | v2026.10.17 (sha256 `fe6dd17e…e8a`); ACT4's `.mise.toml` then pins uv 0.13.4, Ruby 3.4.11, Bundler 4.0.23, and its Gemfile.lock pins the `udb` 0.1.18 and `udb-gen` 0.1.17 gems |

`setup.sh` holds the full URLs and checksums.

## Reproduce

Linux x86_64, about 8 GB of disk (mostly the compiler and the ACT4 build directory). From the repository root:

```bash
tests/act4/setup.sh                                # ~2 min: toolchain into ~/.cache/hart-act4
cargo build --release                              # target/release/hart
python3 tests/act4/act4.py generate --fast         # ~5 min: hart-rv64gc and sail-RVA20S64 ELFs
python3 tests/act4/act4.py --out act4-results all  # ~1 min: checks and runs, see below
```

`all` does these steps and writes the results to `act4-results/`:

1. `exclusions-check.md`: compares the generated hart-rv64gc set with the upstream
   sail-RVA20S64 set. It fails if an upstream test is missing from the hart set and
   not listed in `exclusions.tsv`, or if a listed test was generated anyway.
2. `negative-sail.md`, `negative-hart.md`: the negative control. For one ELF per suite
   (22 ELFs), it flips bit 0 of every doubleword of the expected-results region
   (`begin_signature`..`end_signature`) and requires the simulator to report
   `TEST FAILED` with a nonzero exit.
3. `results-sail.md`: every hart-rv64gc ELF on `sail_riscv_sim --config config/hart-rv64gc/sail.json`.
   This is the self-check of the instrument: the macros and config are the ones the ELFs were
   built with, so every ELF must pass on Sail.
4. `results-hart.md`: every hart-rv64gc ELF on `hart --elf <elf> --timebase insns:1 --timeout 120`.

Each step also writes a `.tsv` table; per-test console logs are in `logs/`, and
`provenance.json` records the hart commit, the `tests/act4` tree, and the tool versions.
Individual steps: `act4.py exclusions`, `act4.py run --sim hart|sail [--filter REGEX]`,
`act4.py negative --sim hart|sail`. `act4.py --help` lists the options.

**Pass criterion** (per ELF, as in ACT4's `run_tests.py`): exit status 0, at least one
`RVCP-SUMMARY:` line, and every such line reads `TEST PASSED` for the ELF's own `.S` file.
A timeout, a nonzero exit, `TEST FAILED`, `SIGRUN`, or a missing summary is a failure.

### Expected counts at the pins

| Item | Count |
| --- | ---: |
| ELFs generated for hart-rv64gc | 592 |
| ELFs generated for upstream sail-RVA20S64 | 631 |
| Excluded (all Sv48: Sv 29, SvPMP 4, SvSm 4, Svade 2) | 39 |
| hart-rv64gc ELFs passing on Sail (self-check) | 592 |
| Negative control detected (Sail, hart) | 22, 22 |

Suites in the hart set (from `work/hart-rv64gc/test_list.md`): D 116, F 84, I 51, PMPSm 40,
Zca 32, Sv 29, Sm 28, InterruptsSm 21, Zicclsm 20, Zaamo 18, M 13, PMPZca 13, InterruptsS 11,
PMPS 11, PMPU 11, S 9, U 9, Zicsr 6, SvSm 6, Zmmul 5, ExceptionsSm 4, SvPMP 4, Zalrsc 4,
Zcd 4, ExceptionsSvSm 4, ExceptionsSvZaamoSm 3, ExceptionsSvZalrscSm 3, Svbare 3, Svade 2,
Zicntr 2, Zihpm 2, ExceptionsSv 2, ExceptionsSvZaamo 2, ExceptionsSvZalrsc 2, and one each of
ExceptionsF, ExceptionsS, ExceptionsU, ExceptionsZaamo, ExceptionsZalrsc, ExceptionsZc, PMPF,
PMPZaamo, PMPZalrsc, Ssccptr, Sstvala, Sstvecd, Ssu64xl, ZicntrS, ZicntrSm, ZicntrU, ZicsrF,
Zifencei. The interrupt suites drive the test-only SIG device and the CLINT.

## Exclusions

ACT4 selects tests from the extensions and parameters a configuration declares. The hart
configuration declares every extension of sail-RVA20S64 except Sv48, which the promise lists
as a non-goal and hart does not implement. That removes exactly the 39 `*_sv48_*` tests,
listed one per line in `exclusions.tsv`. No test is excluded by hand, no ACT4
`EXCLUDE_EXTENSIONS` filter is used, and `act4.py exclusions` checks the list against both
generated sets on every run.

## Configuration review map

Upstream differences: `diff -ru $ACT4_HOME/riscv-arch-test/config/sail/sail-RVA20S64 tests/act4/config/hart-rv64gc`.
"IC" is `docs/implementation-choices.md`; line numbers are at commit 9699d7b. Values not
listed are unchanged from sail-RVA20S64 and either match hart (shown) or do not apply to an
extension hart lacks (V, H, Zicbo*, Sscofpmf, Smstateen, Sdtrig, and so on, all disabled).

### Extensions (`hart-rv64gc.yaml` implemented_extensions, `sail.json` extensions)

| Declared | hart source |
| --- | --- |
| I, M (and its subset Zmmul), A = Zaamo + Zalrsc, F, D, C = Zca + Zcd, Zicsr, Zifencei, Zicntr, Zihpm, Sm, S, U | Promise CPU ISA list; IC L13 `misa` = `RV64ACDFIMSU`; IC L43 (Zihpm counters hardwired to zero, allowed by the promise) |
| Sv39 and Svbare; **Sv48 removed** (Sail `Sv48.supported` false) | IC L73: `satp.MODE` Bare and Sv39 only |
| Svade | IC L75: A/D bits fault, hardware never writes PTEs |
| Zicclsm | IC L62: misaligned loads and stores in hardware for RAM |
| Ziccamoa, Ziccif, Ziccrse, Za128rs | RAM supports AMOs (A for RAM), coherent instruction fetch (IC L66), and LR/SC with a one-address reservation (IC L65) |
| Ssccptr | IC L88: page-table walks read RAM (checked as S-mode loads) |
| Sstvala | IC L52–55: `xtval` holds the instruction bits or the faulting address |
| Sstvecd | IC L25: `stvec` Direct mode with any 4-byte-aligned base |
| Ssu64xl | IC L21: `UXL` = 2 |
| Sm and S version 1.13.0; Sail `privileged_isa_version` 1.13 | Needed for medeleg bit 19 (hardware error), which IC L28 makes writable; Sail allows bit 19 only from priv 1.13 |

### UDB parameters (`hart-rv64gc.yaml`)

| Parameter | Value | hart source |
| --- | --- | --- |
| `MEDELEG_DELEGATABLE` | bits 1–9, 12, 13, 15, 19 (0x8b3fe); **changed**: bit 0 off, bit 19 on | IC L28 |
| `MIDELEG_DELEGATABLE` | bits 1, 5, 9 | IC L29 |
| `MISALIGNED_AMO` false; `LRSC_MISALIGNED_BEHAVIOR` "always raise access fault" | | IC L64 |
| `LRSC_FAIL_ON_NON_EXACT_LRSC` | true (**changed**) | IC L65: SC succeeds only at the LR's physical address |
| `LRSC_RESERVATION_STRATEGY` exact access; `LRSC_FAIL_ON_VA_SYNONYM` false | | IC L65: the reservation is a physical address |
| `MUTABLE_MISA_*` | false | IC L13: `misa` read-only |
| `HW_MSTATUS_FS_DIRTY_UPDATE` precise; `MSTATUS_FS_LEGAL_VALUES` [0,1,2,3]; `MSTATUS_VS_LEGAL_VALUES` [0] | | IC L23, L107 |
| `UXLEN`, `SXLEN` | [64] | IC L21 |
| `*_ENDIANNESS` | little | IC L24 |
| `MEI/MSI/MTI/SEI/SSI/STI_INTR_IMPL` | true | IC L30 (all six `mie` bits writable), L31 |
| `ASID_WIDTH` | 16 | IC L74 |
| `SATP_MODE_BARE` | true | IC L73 |
| `MTVEC_MODES`, `STVEC_MODES` [0,1]; base alignment 4 | | IC L25 |
| `MTVEC_ILLEGAL_WRITE_BEHAVIOR` | custom (**changed** from retain) | IC L25: mode 2/3 become 0/1, which is not "retain" |
| `REPORT_VA_IN_*TVAL_ON_*` true; `REPORT_ENCODING_IN_*TVAL_ON_ILLEGAL_INSTRUCTION` true; `*TVAL_WIDTH` 64 | | IC L52–55 (misaligned exceptions are never raised: L56, L62) |
| `TIME_CSR_IMPLEMENTED` | true | IC L46 |
| `MCOUNTENABLE_EN`, `SCOUNTENABLE_EN` | all 32 | IC L45 |
| `HPM_COUNTER_EN` | all false (**changed**); `HPM_COUNTERn_WIDTH` and `HPM_EVENTS` removed because UDB defines them only for enabled counters | IC L43 |
| `MCOUNTINHIBIT_IMPLEMENTED` true; `COUNTINHIBIT_EN` CY, IR, HPM3–31 (not TM) | | IC L42 |
| `WFI_U_MODE` false; `WFI_FINITE` true | | IC L67; IC L135 (test profile: a WFI that nothing could wake completes) |
| `TRAP_ON_ILLEGAL_WLRL` false | | IC L27 (`mcause` stores any value) |
| `TRAP_ON_UNIMPLEMENTED_CSR`, `TRAP_ON_UNIMPLEMENTED_INSTRUCTION`, `TRAP_ON_RESERVED_INSTRUCTION` | true | IC L34; Sail `Ssstrict` |
| `MARCHID_IMPLEMENTED`, `MIMPID_IMPLEMENTED` | false (**changed**; `ARCH_ID_VALUE`, `IMP_ID_VALUE` removed); `VENDOR_ID_BANK`/`OFFSET` 0 | IC L14: 0/0/0 by default; `hart --elf` uses the default `CpuConfig` (`Board::new_test`) |
| `CONFIG_PTR_ADDRESS` | 0 | IC L15 |
| `MISALIGNED_LDST` true; `MISALIGNED_LDST_EXCEPTION_PRIORITY` low; `MISALIGNED_SPLIT_STRATEGY` sequential_bytes | | IC L62: no misaligned exceptions; the lowest faulting address is reported. hart checks both pages before writing anything; no generated test distinguishes this (see limits below) |
| `NUM_PMP_ENTRIES` 16, `NUM_USABLE_PMP_ENTRIES` 16, `PMP_GRANULARITY` 2 (4 bytes), `PMP_TOR/NA4/NAPOT_SUPPORTED` true | | IC L83, L85 |
| `PHYS_ADDR_WIDTH` | 56 | Not stated in IC; `pmpaddr` holds PA bits 55:2 (`crates/hart-core/src/cpu/pmp.rs`) |
| `MISALIGNED_MAX_ATOMICITY_GRANULE_SIZE` 4096, `PMA_GRANULARITY` 3 | unchanged | Not used for test selection or generation; atomicity is unobservable on a single hart |

### Sail configuration (`sail.json`)

| Field | Value | hart source |
| --- | --- | --- |
| `base.medeleg.delegatable_bits` | 0x8B3FE (**changed**) | IC L28 |
| `base.mideleg.delegatable_bits` | 0x222 (**changed**; was 0x2222, LCOFI without Sscofpmf) | IC L29 |
| `base.writable_hpm_counters` | 0 (**changed**) | IC L43 |
| `base.mcountinhibit.writable_bits` | 0xFFFF_FFFD | IC L42 |
| `base.mcounteren/scounteren_writable_bits` | all ones | IC L45 |
| `base.writable_fiom` | true | IC L32 |
| `base.mtvec`, `base.stvec` | direct and vectored, 4-byte alignment | IC L25 |
| `base.xtval_nonzero` | all true except reserved | IC L52–55 |
| `base.mstatus` | FS four-state, VS off | IC L23 |
| `base.privileged_isa_version` | 1.13 (**changed**) | IC L28 (bit 19) |
| `base.reserved_behavior.fcsr_rm` | Fcsr_RM_Illegal (**changed** from Fatal) | IC L109: DYN with `frm` 5–7 raises illegal instruction |
| `memory.physaddr_bits` | 56 | `pmp.rs` (see above) |
| `memory.asidlen` | 16 | IC L74 |
| `memory.pmp` | 16 entries, all usable, grain 0, TOR/NA4/NAPOT | IC L83, L85 |
| `memory.misaligned.exceptions` | scalar loads/stores none; AMO and LR/SC AccessFault | IC L62, L64 |
| `memory.regions` RAM | 0x8000_0000, 2 GiB, main memory | IC L135 (test profile RAM, 2 GiB default) and `link.ld` |
| `memory.regions` MMIO | 0x0200_0000–0x1200_0000, misaligned scalar access AccessFault (**changed**) | IC L120, L63 |
| `memory.regions` ROM | **removed** | `hart --elf` has nothing at 0x1000 (IC L135) |
| `platform.vendorid/archid/impid/hartid` | 0 | IC L14, L15 |
| `platform.reservation` | 8-byte set, `require_exact_reservation_addr` true (**changed**), `invalidate_on_same_hart_store` false | IC L65 |
| `platform.clint` | base 0x0200_0000 | IC L125 |
| `platform.simple_interrupt_generator` | base 0x0C00_0000 | IC L136 |
| `platform.interrupts`, `extensions.S.interrupts` | all supported | IC L30–31 |
| `platform.clock_frequency` | 10 MHz (**changed**) | IC L123 |
| `platform.instructions_per_tick` | 1 (**changed**) | Sail increments `mcycle` once per tick, so 1 models IC L40 (CPI 1); hart runs with `--timebase insns:1` (IC L123) so `mtime` matches too |
| `platform.wfi_is_nop` false; `wfi_available_to_user_mode` false; `max_time_to_wait` 200 | | IC L67, L135 (see limits below) |
| `extensions.F.fflags_dirty_policy` | Fflags_Dirty_Flag (**changed**) | IC L107: FS becomes Dirty when an instruction raises any flag, even an already-set one |
| `extensions.Sv48.supported` | false (**changed**) | IC L73 |
| `Zicbo*`, `Svpbmt`, `Sstc`, `Svadu`, `Svnapot`, `Sdtrig` (0 triggers) | disabled | IC L32, L33, L76 |

### Macros and linker script

| Item | Choice | hart source |
| --- | --- | --- |
| `RVMODEL_HALT_PASS/FAIL` | One 64-bit `sd` of 1 (pass) or 3 (fail) to `tohost`, after `tohost` reads 0 | IC L134: HTIF polled between batches; device 0 exit code `payload >> 1` |
| `RVMODEL_IO_WRITE_STR` | Per byte: wait until `tohost` reads 0, then one `sd` of `(0x0101 << 48) \| byte` | IC L134: device 1 command 1. hart clears `tohost` only when it polls, so without the wait, back-to-back bytes would overwrite each other |
| `RVMODEL_ACCESS_FAULT_ADDRESS` | 0 | IC L119: address 0 unmapped |
| `RVMODEL_MTIME_ADDRESS`, `RVMODEL_MTIMECMP_ADDRESS`, `RVMODEL_MSIP_ADDRESS` | 0x0200_BFF8, 0x0200_4000, 0x0200_0000 | IC L125 |
| `RVMODEL_SET/CLR_MEXT/SEXT/SSW_INT` | SIG PLATFORM register (+4): bit 31 = level, bits 11/9/1 | IC L136 |
| `RVMODEL_MAX_CYCLES_PER_TIMER_TICK` 1, `RVMODEL_TIMER_INT_SOON_DELAY` 20000, `RVMODEL_INTERRUPT_LATENCY` 10 | | One tick per instruction (above). hart updates `mtime`, and so MTIP, between batches of at most 10,000 instructions (`BATCH` in `crates/hart-core/src/board.rs`), so the timer idle loop (about 40,000 instructions) covers the delay plus one batch. SIG and CLINT writes reach `mip` at once, and interrupts are polled before every instruction (IC L97) |
| `link.ld` | unchanged: RAM 0x8000_0000, 2 GiB | IC L135 |

The same macros, linker script, and `sail.json` are used for the Sail run that computes the
expected results and for the ELFs that run on hart.

### Known modelling limits

Sail has no option for a few hart choices. Each is listed with how it is handled:

- **PMP R=0, W=1** (IC L86): hart clears W and keeps X; Sail's closest option
  (`PMP_ClearPermissions`) clears X as well. The two differ only when X = 1.
- **`mtvec`/`stvec` mode 2 or 3** (IC L25): hart clears bit 1; Sail can keep the old mode
  (`Xtvec_Ignore`) or stop. The two differ on such writes.
- **WFI**: hart stalls until `mip & mie` is nonzero, completes when nothing could wake it,
  and skips `mtime` ahead to the timer otherwise (IC L67, L123, L135). Sail waits at most
  `max_time_to_wait` ticks. Both are allowed by the specification.
- **Misaligned store that faults on its second page** (IC L62): hart writes no bytes; Sail
  splits the access.

If a generated test exercises one of these differences, it fails on hart, and the failure is
reported and resolved like any other: hart changes, or this configuration changes if Sail
gains a matching option. Nothing is excluded on these grounds.
