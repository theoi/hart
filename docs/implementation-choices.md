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

## Floating point

| Area | Choice |
| --- | --- |
| Arithmetic | From-scratch integer soft-float (`softfloat/`), checked against Berkeley TestFloat (`tests/testfloat/`). No host floating point. |
| NaN results | Always the canonical NaN (`0x7fc00000`, `0x7ff8000000000000`), as the spec requires; no payload propagation. |
| Tininess | Detected after rounding, as the spec requires. |
| `mstatus.FS` | Set to Dirty when an instruction writes an f register (loads, moves, and arithmetic, even if the value is unchanged), when it accrues a nonzero set of flags, or on any write to `fflags`, `frm`, or `fcsr`. FP stores, and instructions that write only an integer register and raise no flag, leave FS unchanged. Matches Spike. |
| FS = Off | Every FP instruction, including loads, stores, and moves, and every access to `fflags`, `frm`, and `fcsr` raises illegal instruction. |
| Rounding mode | Every instruction with an `rm` field (arithmetic, square root, FMA, and all `fcvt` forms, including the exact `fcvt.d.s` and `fcvt.d.w[u]`) raises illegal instruction when `rm` is 5 or 6, or when `rm` is 7 (DYN) and `frm` is 5, 6, or 7. `frm` itself accepts any 3-bit value. |
| `fcsr` | Bits 7:5 are `frm`, bits 4:0 are `fflags`; the rest read as zero and ignore writes. |
| NaN-boxing | Single-precision results are written with the upper 32 bits set. Single-precision operands whose upper 32 bits are not all ones read as the canonical NaN. `fmv.x.w` and `fsw` use the raw low 32 bits. |
| FP loads and stores | Misaligned addresses are supported, like integer accesses. |
| Commit log | f registers are logged 64 bits wide. An instruction that raises any flag logs `fflags` with the new accrued value. CSR writes to `fcsr` log `fflags` and `frm` (no `fcsr` entry). FS becoming Dirty logs `mstatus`. |
