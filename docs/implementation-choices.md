# Implementation choices

Where the RISC-V specifications leave behavior to the implementation, hart makes the
choices below. Spike's choices are matched where practical, so that differential testing
against Spike needs no special cases. The reference Spike configuration is
`--isa=rv64imafdc_zicsr_zifencei_zicntr_zihpm_zicclsm_svade --pmpregions=16
--pmpgranularity=4 --triggers=0`.

## Identification

| Item | Choice |
| --- | --- |
| `misa` | `RV64ACDFIMSU` (0x800000000014112d). Read-only; writes are ignored. |
| `mvendorid`, `marchid`, `mimpid` | Set per profile in `CpuConfig`. Default 0/0/0; the lockstep (Spike) profile uses 0/5/0. |
| `mhartid`, `mconfigptr` | 0. |

## Status and trap CSRs

| Item | Choice |
| --- | --- |
| `mstatus.UXL`, `SXL` | 2 (64-bit), read-only. |
| `mstatus.MPP` | WARL. Writing the reserved value 2 stores 0 (U). |
| `mstatus.FS` | All four states writable. `SD` reads 1 when FS is Dirty. `VS` and `XS` are 0. |
| `mstatus.UBE`, `SBE`, `MBE` | 0 (little-endian only). |
| `mtvec`, `stvec` | Direct (0) and Vectored (1) modes. Bit 1 is read-only zero, so writing mode 2 or 3 yields 0 or 1. |
| `mepc`, `sepc` | Bit 0 reads zero (C is always enabled, so IALIGN is 16). |
| `mcause`, `scause` | Any value is stored (WLRL). |
| `medeleg` | Bits 0–9, 12, 13, and 15 are writable. Ecall from M (11) cannot be delegated. |
| `mideleg` | SSIP, STIP, and SEIP are writable. |
| `mie` | MSIE, MTIE, MEIE, SSIE, STIE, and SEIE are writable. |
| `mip` | M-mode writes SSIP, STIP, and SEIP. `sip` writes SSIP only, when delegated. MSIP and MTIP come from the CLINT, MEIP from the PLIC. A read of SEIP returns the software bit ORed with the PLIC line. `csrrs`/`csrrc` on `mip`/`sip` modify only the software-writable bits. |
| `menvcfg`, `senvcfg` | Only FIOM is writable (no Zicbo*, Svpbmt, Sstc, or Svadu). |
| Debug triggers | Not implemented (no Sdtrig). `tselect` and `tdata1`–`tdata3` read 0 and ignore writes. |
| Unimplemented CSRs | Raise illegal instruction. |

## Counters

| Item | Choice |
| --- | --- |
| `mcycle`, `minstret` | Both count retired instructions (CPI 1). A trapping instruction does not retire. WFI retires once. |
| Writes to `mcycle`/`minstret` | The written value takes precedence over that instruction's own increment. |
| `mcountinhibit` | CY, IR, and HPM3–31 are writable; TM is read-only zero. |
| `mhpmcounter3`–`31`, `mhpmevent3`–`31` | Hardwired to zero (writes ignored). `hpmcounterN` read 0 when enabled. |
| `mcounteren`, `scounteren` | All 32 bits writable. |
| `time` | Reads the CLINT `mtime` value from `Wires`. |

## Exception values

| Trap | `xtval` |
| --- | --- |
| Illegal instruction | The instruction bits (16 bits for compressed encodings). |
| Breakpoint (EBREAK, C.EBREAK) | The pc of the instruction. |
| Ecall | 0. |
| Access and page faults | The faulting virtual address. For a fetch that crosses a page, the address of the part that faults. |
| Instruction address misaligned | Never raised (C is always enabled). |

## Memory access

| Item | Choice |
| --- | --- |
| Misaligned loads and stores | Supported in hardware for RAM (Zicclsm), including accesses that cross a page. Both pages are translated and PMP-checked before any byte is accessed. |
| Misaligned access to a device region | Access fault. |
| Misaligned AMO, LR, SC | Access fault: load access fault (5) for LR, store/AMO access fault (7) for SC and AMOs. Matches Spike and Sail's RVA20S64 configuration. |
| LR/SC reservation | The physical address of the LR. SC succeeds only at that address, and any SC clears the reservation. SC is translated as a store first, so it can page-fault even when it would fail. Traps and xRET do not clear the reservation. |
| FENCE, FENCE.I | FENCE is a no-op. FENCE.I invalidates any decoded-instruction cache. |
| WFI | Stalls until `mip & mie` is nonzero, regardless of global interrupt enables. Illegal in U-mode, and in S-mode when `mstatus.TW` = 1. |

## Virtual memory (Sv39)

| Item | Choice |
| --- | --- |
| `satp.MODE` | Bare and Sv39. A write with any other mode leaves `satp` unchanged. |
| `satp.ASID` | 16 bits, writable. The TLB is flushed on every `satp` write, so ASIDs are not used for tagging. |
| A and D bits | Svade: a leaf PTE with A = 0, or D = 0 on a store or AMO, raises a page fault. Hardware never writes PTEs. |
| Reserved PTE bits | Bits 63:54 must be zero (no Svnapot or Svpbmt), otherwise page fault. A non-leaf PTE with A, D, or U set raises a page fault. |
| TLB | 256 entries, direct-mapped by virtual page number. Flushed by SFENCE.VMA (any operands), `satp` writes, PMP writes, and `mstatus` writes that change MPRV, SUM, MXR, or MPP. |

## PMP

| Item | Choice |
| --- | --- |
| Entries | 16. `pmpcfg0` and `pmpcfg2` exist; odd `pmpcfg` CSRs raise illegal instruction. `pmpaddr16`–`63` and the upper `pmpcfg` registers read 0. |
| Granularity | 4 bytes (G = 0). TOR, NA4, and NAPOT are supported. |
| Reserved R=0, W=1 | Stored as R=0, W=0. |
| Locking | Locked entries ignore writes, as does `pmpaddr[i-1]` when entry `i` is locked TOR. |
| Page-table walks | Checked as S-mode loads. |
| Partial matches | An access that only partly overlaps the highest-priority matching entry fails. |
| No match | M-mode succeeds; S/U-mode fails. |

## Interrupts

| Item | Choice |
| --- | --- |
| Priority | Interrupts to M-mode (not delegated) before interrupts to S-mode. Within each: MEI, MSI, MTI, SEI, SSI, STI. |
| Polling, normal profile | Before every instruction. |
| Polling, lockstep profile | Like Spike: at chunk start, after every Zicsr instruction, MRET, and SRET, and immediately before every Zicsr instruction. |

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
