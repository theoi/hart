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
```

The self-test checks that identical logs match, that reordered register
entries and interleaved console text still match, and that each injected
mutation (pc, instruction, privilege, x/f/CSR value, missing or extra register
entry, load address, store address or data, dropped line, extra line) is
reported at the mutated line.
