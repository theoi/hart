# Differential testing against Spike

Instruments for promise acceptance 3: hart and Spike must produce the same
per-instruction commit log for a fixed set of randomized bare-metal programs
and for the Linux boot.

| file | purpose |
|------|---------|
| `compare.py` | Streams two Spike-format commit logs and reports the first divergence (missing line, extra line, or field mismatch). |
| `selftest.py` | Mutation self-test for `compare.py` (check `diff-harness-sensitivity`). |
| `make_fixture.sh`, `fixtures/` | Spike commit logs that the self-test mutates. |
| `csrprobe/` | CSR existence and WARL probe, plus the Spike CSR map it produces. |
| `link.ld` | Linker script for bare-metal test programs (RAM at 0x80000000, HTIF `tohost`). |
| `gen/rvgen.py`, `run_seeds.sh` | Random RV64GC program generator and the driver that runs its programs on Spike and hart and compares the logs (check `spike-diff-random`). |
| `lockstep/` | Linux boot lockstep: the Spike configuration (`run_spike.sh`, `spike-sv39.dtb`) and a driver that compares both boots as they run (`lockstep.sh`). |

## Reference

Spike `riscv-isa-sim` at commit `2c300cd90eb74bb79f3fcdd58596a6f8d842d01c`, run as:

```sh
spike --isa=rv64imafdc_zicsr_zifencei_zicntr_zihpm_zicclsm_svade \
      --pmpregions=16 --pmpgranularity=4 --triggers=0 --no-dm \
      --log-commits program.elf 2> spike.log
```

- `zicclsm`: misaligned loads and stores are handled in hardware, as in hart.
- `svade`: A and D bits are not updated by hardware; a clear bit faults.
- `--triggers=0 --no-dm`: no debug triggers and no debug module at address 0.
  The trigger CSRs (`tselect`, `tdata1-3`, `tinfo`, `mcontext`, `scontext`)
  still exist and read zero; see `csrprobe/spike-csrmap.txt`.

## Commit-log format

One line per retired instruction, as Spike's `--log-commits` prints it:

```
core   0: <priv> 0x<pc:16> (0x<insn:8 or 4>) [<reg> 0x<value>]... [mem 0x<addr>]... [mem 0x<addr> 0x<data>]...
```

- `priv` is the mode the instruction executed in (0 U, 1 S, 3 M).
- Register entries: `x<n>` and `f<n>` (64-bit values; NaN-boxed singles show
  `0xffffffff` in the upper half), `c<decimal address>_<name>` for CSRs. x0 is
  never listed. The entries on a line are a set: Spike prints them from an
  unordered map, and `compare.py` compares them as a set.
- `mem 0x<addr>` is a load and `mem 0x<addr> 0x<data>` a store, with the data
  as wide as the access. Addresses are **virtual**. An AMO logs its load and
  then its store. A failed SC logs no memory entry.
- An instruction that traps prints no line. Trap entry and interrupts print
  nothing; the next line is the first instruction of the handler.
- CSR side effects that Spike logs (and hart must log the same way): FP
  instructions add `c1_fflags` when they raise a flag; the first write that
  makes `mstatus.FS` dirty adds `c768_mstatus`; `fcsr` writes log `c1_fflags`
  and `c2_frm`; `sstatus` writes log `c768_mstatus`; `sie` writes log `c260_sie`
  and `c772_mie`; `mip`/`sip` writes log `c777_mvip` and `c836_mip`; `mret` and
  `sret` log `c768_mstatus`. `compare.py` drops `c777_mvip` by default.

## Comparator

```sh
compare.py [--start-pc ADDR] [--end-store TOHOST] [--keep-going N] REF TEST
```

`REF` is the Spike log, `TEST` the hart log; either can be a file, `-`, or a
named pipe. `--start-pc 0x80000000` skips each simulator's reset code;
`--end-store` stops at the HTIF `tohost` store so the post-exit spin loop is
not compared. Exit status: 0 match, 1 divergence, 2 malformed log.

## Self-test

```sh
./make_fixture.sh /path/to/spike /path/to/riscv-tests/isa   # regenerate fixtures
./selftest.py                                               # seed 1, 25 mutations per kind
./selftest.py --seed N --per-kind 100 --end-store 0x80010000 fixtures/*.log.gz OUT/*.spike.log
```

Extra logs, such as random-program logs kept by `run_seeds.sh --keep`, can be
added. `--end-store` cuts them after the `tohost` store (0x80010000 for
`rvgen.py` programs), because the spin loop after it is a run of identical
lines in which a dropped or duplicated line cannot be located. Mutations never
touch CSRs that `compare.py` drops (`mvip`).

The self-test checks that identical logs match, that reordered register
entries and interleaved console text still match, and that each injected
mutation (pc, instruction, privilege, x/f/CSR value, missing or extra register
entry, load address, store address or data, dropped line, extra line) is
reported at the mutated line.

## Random programs

```sh
SPIKE=/path/to/spike tests/diff/run_seeds.sh FIRST LAST OUTDIR --hart target/release/hart --jobs 4
```

`gen/rvgen.py SEED` writes one assembly program; the same seed always gives the
same program. Each seed picks a profile: the mode the body runs in (M, S, or
U), bare or Sv39 translation (4 KiB pages with random permission, A and D bits,
a 2 MiB megapage that is sometimes misaligned, and a U-mode alias of the code),
trap delegation, `mstatus` bits (FS, SUM, MXR, TVM, TW, TSR), counter enables,
and whether interrupts are used. The body is 300 to 900 random items: integer,
M, A (including LR/SC pairs and misaligned AMOs), F and D with every rounding
mode and special values, compressed and hint encodings, aligned and misaligned
loads and stores, CSR accesses (including inaccessible and nonexistent CSRs),
ecall, ebreak, illegal and reserved encodings, `mret`/`sret`/`wfi`/`sfence.vma`
in every mode, and software and timer interrupts through `mip`, `sip`, and the
CLINT.

Every program terminates: control flow only moves forward, each instruction
that may trap is preceded by `la x31, <next>`, and the M- and S-mode handlers
resume at `x31`. Stores always use the data-region base registers, so programs
never modify their own code or page tables. The program ends with `ecall` while
`x31` is 0, and the M-mode handler writes 1 to `tohost`.

`run_seeds.sh` builds each program with `riscv64-unknown-elf-gcc` (`CC`), runs it
on Spike with the reference flags plus `-m256 --dtb=lockstep/spike-sv39.dtb`, and
with `--hart` on `hart --profile spike` with the same device tree, so both see
the same reset ROM, CLINT, PLIC, and UART. It compares the logs with
`compare.py --end-store tohost`. `OUTDIR/summary.txt` has one line per seed
(`SEED spike=RC lines=N hart=RC cmp=RC`), and `OUTDIR/elf.sha256` the hashes of
the programs. Logs are kept for failing seeds (all seeds with `--keep`). The
exit status is 0 only if every seed passed.

## Boot lockstep

```sh
SPIKE=/path/to/spike tests/diff/lockstep/lockstep.sh FW_JUMP_ELF IMAGE LIMIT [OUTDIR]
```

`FW_JUMP_ELF` is OpenSBI's `fw_jump.elf` and `IMAGE` the kernel `Image`, for
example from the guest release that `tests/boot/boot_test.py` fetches. The
driver builds hart (`cargo build --release`; set `HART` to use a prebuilt
binary instead) and boots the guest on
Spike (`run_spike.sh`: the reference flags above, 256 MiB, the device tree in
`spike-sv39.dtb`) and on `hart --profile spike` with the same device tree. Both
write their commit logs into named pipes that `compare.py --limit LIMIT` reads,
so a full boot (tens of millions of lines, gigabytes as text) is compared
without being stored; expect roughly 0.7 million lines per second.

`OUTDIR` (default `target/lockstep`) receives `compare.txt`, both consoles
(`spike-console.log`, `hart-console.log`), and each simulator's stderr.
The driver prints the comparator's report and whether the consoles agree, and
exits with the comparator's status. Set `COMPARE_ARGS="--keep-going N"` to list
value-only mismatches and keep going until the first change of control flow.

The guest-v1 images print the shell prompt about 61 million instructions after
reset in this configuration, so a `LIMIT` of 100000000 covers reset to the
prompt with a margin. Neither side gets console input, so after the prompt the
guest idles in its timer loop.
