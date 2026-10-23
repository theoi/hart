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
"IC" is `docs/implementation-choices.md`; line numbers are at commit 8272f46. Values not
listed are unchanged from sail-RVA20S64 and either match hart (shown) or do not apply to an
extension hart lacks (V, H, Zicbo*, Sscofpmf, Smstateen, and so on, all disabled). Sdtrig is
disabled too; hart's zero-trigger Sdtrig CSRs are covered under the limits below.

### Extensions (`hart-rv64gc.yaml` implemented_extensions, `sail.json` extensions)

| Declared | hart source |
| --- | --- |
| I, M (and its subset Zmmul), A = Zaamo + Zalrsc, F, D, C = Zca + Zcd, Zicsr, Zifencei, Zicntr, Zihpm, Sm, S, U | Promise CPU ISA list; IC L25 `misa` = `RV64ACDFIMSU`; IC L55 (Zihpm counters hardwired to zero, allowed by the promise) |
| Sv39 and Svbare; **Sv48 removed** (Sail `Sv48.supported` false) | IC L89: `satp.MODE` Bare and Sv39 only |
| Svade | IC L91: A/D bits fault, hardware never writes PTEs |
| Zicclsm | IC L77: misaligned loads and stores in hardware for RAM |
| Ziccamoa, Ziccif, Ziccrse, Za128rs | RAM supports AMOs (A for RAM), coherent instruction fetch (IC L82), and LR/SC with a one-address reservation (IC L80) |
| Ssccptr | IC L104, L75: page-table walks are checked as S-mode loads and read only RAM |
| Sstvala | IC L65–68: `xtval` holds the instruction bits or the faulting address |
| Sstvecd | IC L37: `stvec` Direct mode with any 4-byte-aligned base |
| Ssu64xl | IC L33: `UXL` = 2 |
| Sm and S version 1.13.0; Sail `privileged_isa_version` 1.13 | Needed for medeleg bit 19 (hardware error), which IC L40 makes writable; Sail allows bit 19 only from priv 1.13 |

### UDB parameters (`hart-rv64gc.yaml`)

| Parameter | Value | hart source |
| --- | --- | --- |
| `MEDELEG_DELEGATABLE` | bits 1–9, 12, 13, 15, 19 (0x8b3fe); **changed**: bit 0 off, bit 19 on | IC L40 |
| `MIDELEG_DELEGATABLE` | bits 1, 5, 9 | IC L41 |
| `MISALIGNED_AMO` false; `LRSC_MISALIGNED_BEHAVIOR` "always raise access fault" | | IC L79 |
| `LRSC_FAIL_ON_NON_EXACT_LRSC` | true (**changed**) | IC L80: SC succeeds only at the LR's physical address |
| `LRSC_RESERVATION_STRATEGY` exact access; `LRSC_FAIL_ON_VA_SYNONYM` false | | IC L80: the reservation is a physical address |
| `MUTABLE_MISA_*` | false | IC L25: `misa` read-only |
| `HW_MSTATUS_FS_DIRTY_UPDATE` precise; `MSTATUS_FS_LEGAL_VALUES` [0,1,2,3]; `MSTATUS_VS_LEGAL_VALUES` [0] | | IC L35, L123 |
| `UXLEN`, `SXLEN` | [64] | IC L33 |
| `*_ENDIANNESS` | little | IC L36 |
| `MEI/MSI/MTI/SEI/SSI/STI_INTR_IMPL` | true | IC L42 (all six `mie` bits writable), L43 |
| `ASID_WIDTH` | 16 | IC L90 |
| `SATP_MODE_BARE` | true | IC L89 |
| `MTVEC_MODES`, `STVEC_MODES` [0,1]; base alignment 4 | | IC L37 |
| `MTVEC_ILLEGAL_WRITE_BEHAVIOR` | custom (**changed** from retain) | IC L37: mode 2/3 become 0/1, which is not "retain" |
| `REPORT_VA_IN_*TVAL_ON_*` true; `REPORT_ENCODING_IN_*TVAL_ON_ILLEGAL_INSTRUCTION` true; `*TVAL_WIDTH` 64 | | IC L65–68 (misaligned exceptions are never raised: IC L69, L77) |
| `TIME_CSR_IMPLEMENTED` | true | IC L59 |
| `MCOUNTENABLE_EN`, `SCOUNTENABLE_EN` | all 32 | IC L58 |
| `HPM_COUNTER_EN` | all false (**changed**); `HPM_COUNTERn_WIDTH` and `HPM_EVENTS` removed because UDB defines them only for enabled counters | IC L55 |
| `MCOUNTINHIBIT_IMPLEMENTED` true; `COUNTINHIBIT_EN` CY, IR, HPM3–31 (not TM) | | IC L54 |
| `WFI_U_MODE` false; `WFI_FINITE` true | | IC L83; IC L152 (test profile: a WFI that nothing could wake completes) |
| `TRAP_ON_ILLEGAL_WLRL` false | | IC L39 (`mcause` stores any value) |
| `TRAP_ON_UNIMPLEMENTED_CSR`, `TRAP_ON_UNIMPLEMENTED_INSTRUCTION`, `TRAP_ON_RESERVED_INSTRUCTION` | true | IC L46; Sail `Ssstrict` |
| `MARCHID_IMPLEMENTED`, `MIMPID_IMPLEMENTED` | false (**changed**; `ARCH_ID_VALUE`, `IMP_ID_VALUE` removed); `VENDOR_ID_BANK`/`OFFSET` 0 | IC L26: 0/0/0 by default; `hart --elf` uses the default `CpuConfig` (`Board::new_test`) |
| `CONFIG_PTR_ADDRESS` | 0 | IC L27 |
| `MISALIGNED_LDST` true; `MISALIGNED_LDST_EXCEPTION_PRIORITY` low | | IC L77: no misaligned exceptions |
| `MISALIGNED_SPLIT_STRATEGY` | custom (**changed** from sequential_bytes) | IC L77, L68: a misaligned access within one page is performed whole (see `MISALIGNED_MAX_ATOMICITY_GRANULE_SIZE`); for one that crosses a page, both pages are translated and PMP-checked before any byte is accessed, and a fault reports the address of the part that faults. UDB's other option, sequential_bytes, processes the bytes one at a time from the lowest address. At the pin, no ACT4 generator reads this parameter (only the PMPSm testplan mentions it), so it does not change the generated tests |
| `NUM_PMP_ENTRIES` 64 (**changed** from 16); `NUM_USABLE_PMP_ENTRIES` 16 | | IC L99: 16 usable entries; `pmpaddr16`–`63` and the upper `pmpcfg` registers exist and read 0. In UDB, `NUM_PMP_ENTRIES` 16 would make those CSRs nonexistent (illegal instruction) |
| `PMP_GRANULARITY` 2 (4 bytes), `PMP_TOR/NA4/NAPOT_SUPPORTED` true | | IC L99, L101 |
| `PHYS_ADDR_WIDTH` | 56 | IC L76: 56-bit physical addresses; `pmpaddr` holds address bits 55:2 |
| `MISALIGNED_MAX_ATOMICITY_GRANULE_SIZE` | 4096 | IC L77: a misaligned RAM access within one page is one access: one translation and one PMP check over the whole range (a partial overlap faults, IC L105), with `xtval` = the start address (`load_slow`/`store_slow` in `crates/hart-core/src/cpu/mod.rs`). Matches the RAM region's 4 KiB misaligned atomicity granule in `sail.json` |
| `PMA_GRANULARITY` | 3 (unchanged) | Not used for test selection or generation |

### Sail configuration (`sail.json`)

| Field | Value | hart source |
| --- | --- | --- |
| `base.medeleg.delegatable_bits` | 0x8B3FE (**changed**) | IC L40 |
| `base.mideleg.delegatable_bits` | 0x222 (**changed**; was 0x2222, LCOFI without Sscofpmf) | IC L41 |
| `base.writable_hpm_counters` | 0 (**changed**) | IC L55 |
| `base.mcountinhibit.writable_bits` | 0xFFFF_FFFD | IC L54 |
| `base.mcounteren/scounteren_writable_bits` | all ones | IC L58 |
| `base.writable_fiom` | true | IC L44 |
| `base.mtvec`, `base.stvec` | direct and vectored, 4-byte alignment | IC L37 |
| `base.xtval_nonzero` | all true except reserved | IC L65–68 |
| `base.mstatus` | FS four-state, VS off | IC L35 |
| `base.privileged_isa_version` | 1.13 (**changed**) | IC L40 (bit 19) |
| `base.reserved_behavior.fcsr_rm` | Fcsr_RM_Illegal (**changed** from Fatal) | IC L125: DYN with `frm` 5–7 raises illegal instruction |
| `memory.physaddr_bits` | 56 | IC L76 |
| `memory.asidlen` | 16 | IC L90 |
| `memory.pmp` | `count` 64 (**changed** from 16), `usable_count` 16, grain 0, TOR/NA4/NAPOT | IC L99, L101: entries 16–63 exist and read 0 |
| `memory.misaligned.exceptions` | scalar loads/stores none; AMO and LR/SC AccessFault | IC L77, L79 |
| `memory.regions` RAM | 0x8000_0000, 2 GiB, main memory | IC L152 (test profile RAM, 2 GiB default), L75 (RAM is the only main memory), and `link.ld` |
| `memory.regions` RAM `misaligned_atomicity_granule_size_exp` | 12 (**changed** from 0) | IC L77, L105: a misaligned access that stays within a 4 KiB page is performed as one access with one PMP check, as in hart. With 0, Sail split it into aligned chunks and PMP-checked each, which differs for an access that straddles a PMP boundary inside a page |
| `memory.misaligned.default_allowed_within_exp` | 0 (unchanged) | Applies only where the MAG does not: page-crossing RAM accesses (see limits below); misaligned device accesses fault first (IC L78) |
| `memory.regions` devices | One region per test-profile device (**changed** from one region over 0x0200_0000–0x1200_0000): syscon 0x0010_0000 (4 KiB), CLINT 0x0200_0000 (64 KiB), SIG 0x0C00_0000 (4 KiB), UART 0x1000_0000 (4 KiB). Addresses between them are unmapped, as in hart | IC L152, L135; base and size of each device in `Board::new_test` (`crates/hart-core/src/board.rs`). Sail regions are whole pages, so the SIG and UART regions are larger than the devices (see limits below) |
| `memory.regions` device attributes | IO memory, not executable, no AMOs (`AMONone`) or LR/SC (`RsrvNone`), no page-table reads or writes; misaligned scalar access AccessFault (**changed**) | IC L75: a fetch from a device raises an instruction access fault (1), an AMO a store/AMO access fault (7), and a page-table walk whose PTE is not in RAM an access fault of the original access type; LR and SC outside RAM fault (IC L81). IC L136, L78: misaligned device accesses fault (within a page; see limits below for page-crossing accesses) |
| `memory.regions` ROM | **removed** | `hart --elf` has nothing at 0x1000 (IC L152) |
| `platform.vendorid/archid/impid/hartid` | 0 | IC L26, L27 |
| `platform.reservation` | 8-byte set, `require_exact_reservation_addr` true (**changed**), `invalidate_on_same_hart_store` false | IC L80 |
| `platform.clint` | base 0x0200_0000, size 0x1_0000 (**changed** from 0xC_0000) | IC L141; `CLINT_SIZE` in `crates/hart-core/src/devices/clint.rs` |
| `platform.simple_interrupt_generator` | base 0x0C00_0000 | IC L153 |
| `platform.interrupts`, `extensions.S.interrupts` | all supported | IC L42–43 |
| `platform.clock_frequency` | 10 MHz (**changed**) | IC L139 |
| `platform.instructions_per_tick` | 1 (**changed**) | Sail increments `mcycle` once per tick, so 1 models IC L52 (CPI 1); hart runs with `--timebase insns:1` (IC L139) so `mtime` matches too |
| `platform.wfi_is_nop` false; `wfi_available_to_user_mode` false; `max_time_to_wait` 200 | | IC L83, L151 (see limits below) |
| `extensions.F.fflags_dirty_policy` | Fflags_Dirty_Flag (**changed**) | IC L123: FS becomes Dirty when an instruction raises any flag, even an already-set one |
| `extensions.Sv48.supported` | false (**changed**) | IC L89 |
| `Zicbo*`, `Svpbmt`, `Sstc`, `Svadu`, `Svnapot`, `Sdtrig` (0 triggers) | disabled | IC L44, L45, L92; for the trigger CSRs see limits below |

### Macros and linker script

| Item | Choice | hart source |
| --- | --- | --- |
| `RVMODEL_HALT_PASS/FAIL` | One 64-bit `sd` of 1 (pass) or 3 (fail) to `tohost`, after `tohost` reads 0 | IC L151: HTIF polled between batches; device 0 exit code `payload >> 1` |
| `RVMODEL_IO_WRITE_STR` | Per byte: wait until `tohost` reads 0, then one `sd` of `(0x0101 << 48) \| byte` | IC L151: device 1 command 1. hart clears `tohost` only when it polls, so without the wait, back-to-back bytes would overwrite each other |
| `RVMODEL_ACCESS_FAULT_ADDRESS` | 0 | IC L135: address 0 unmapped |
| `RVMODEL_MTIME_ADDRESS`, `RVMODEL_MTIMECMP_ADDRESS`, `RVMODEL_MSIP_ADDRESS` | 0x0200_BFF8, 0x0200_4000, 0x0200_0000 | IC L141 |
| `RVMODEL_SET/CLR_MEXT/SEXT/SSW_INT` | SIG PLATFORM register (+4): bit 31 = level, bits 11/9/1 | IC L153 |
| `RVMODEL_MAX_CYCLES_PER_TIMER_TICK` 1, `RVMODEL_TIMER_INT_SOON_DELAY` 20000, `RVMODEL_INTERRUPT_LATENCY` 10 | | One tick per instruction (above). IC L113: `mtime` and device lines change only during device accesses or between run batches, which are at most 10,000 instructions (`BATCH` in `crates/hart-core/src/board.rs`), so the timer idle loop (about 40,000 instructions) covers the delay plus one batch. SIG and CLINT writes take effect at the access, and the CPU polls after every device access (IC L113) |
| `link.ld` | unchanged: RAM 0x8000_0000, 2 GiB | IC L152 |

The same macros, linker script, and `sail.json` are used for the Sail run that computes the
expected results and for the ELFs that run on hart.

### Known modelling limits

Sail has no option for a few hart choices. Each is listed with how it is handled:

- **PMP R=0, W=1** (IC L102): hart clears W and keeps X; Sail's closest option
  (`PMP_ClearPermissions`) clears X as well. The two differ only when X = 1.
- **`mtvec`/`stvec` mode 2 or 3** (IC L37): hart clears bit 1; Sail can keep the old mode
  (`Xtvec_Ignore`) or stop. The two differ on such writes.
- **WFI**: hart stalls until `mip & mie` is nonzero, completes when nothing could wake it,
  and skips `mtime` ahead to the timer otherwise (IC L83, L139, L151). Sail waits at most
  `max_time_to_wait` ticks. Both are allowed by the specification.
- **Misaligned RAM access that crosses a page** (IC L77): hart translates and PMP-checks the
  part on each page as one access each, before accessing any byte. Sail splits the access into
  naturally aligned chunks and checks and performs them in increasing address order. The two
  can differ when a PMP boundary lies inside one page's part, and in which bytes a store that
  faults on its second page has already written.
- **Misaligned device access that crosses a page** (IC L78): hart splits it into byte
  accesses; Sail's device regions raise an access fault for every misaligned access.
- **Debug-trigger CSRs** (IC L45): hart implements the Sdtrig CSRs with zero triggers, as
  Spike does with `--triggers=0`: `tselect`, `tdata1`–`tdata3`, and `tinfo` read 0 and ignore
  writes, and `scontext` (32 bits) and `mcontext` (13 bits) are read/write registers. No Sail
  setting matches: with `Sdtrig.supported` false these CSRs raise illegal instruction, and
  with it true Sail requires `num_triggers` of at least 1. The configuration declares no
  Sdtrig (absent from `hart-rv64gc.yaml`, disabled in `sail.json`), so ACT4 generates no Sdtrig
  tests.
- **CLINT register accesses** (IC L141): hart's CLINT accepts naturally aligned 1-, 2-, 4-,
  and 8-byte accesses to any part of `msip`, `mtimecmp`, and `mtime`, and reads 0 and ignores
  writes at other offsets (`crates/hart-core/src/devices/clint.rs`). Sail's CLINT accepts only
  4- or 8-byte accesses at the start of a register and 4-byte accesses at the upper half of
  `mtimecmp` and `mtime`; any other access is an access fault.
- **UART, syscon, and the rest of the SIG and UART pages** (IC L143, L144, L152, L153): Sail
  has no model of the test profile's UART (0x1000_0000) or syscon (0x0010_0000), so their
  regions behave as plain memory in Sail. The SIG uses 0x20 bytes and the UART 0x100 bytes of
  their 4 KiB pages; in hart the rest of each page is unmapped (access fault), while in Sail it
  is plain memory. ACT4 tests use HTIF for output and exit, not these devices.

If a generated test exercises one of these differences, it fails on hart, and the failure is
reported and resolved like any other: hart changes, or this configuration changes if Sail
gains a matching option. Nothing is excluded on these grounds.
