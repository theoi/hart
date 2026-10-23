# Implementation choices

Where the RISC-V specifications leave behavior to the implementation, hart makes the
choices below. Spike's choices are matched where practical, so differential testing
against Spike needs no special cases.

This file is filled in as each area lands.

## CPU

| Area | Choice |
| --- | --- |
| `misa` | `RV64ACDFIMSU` (0x800000000014112d), read-only. |
| `mvendorid`, `marchid`, `mimpid`, `mhartid`, `mconfigptr` | All 0. |
