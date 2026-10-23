#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Generate the CSR probe program (csrprobe.S).

Phase 1 reads every CSR address 0x000-0xfff in M-mode, then in S-mode, then in
U-mode. Each read is a single `csrrs a0, csr, x0`; a trap handler skips
instructions that trap, so the commit log shows which reads retire and what
they return. Phase 2 (M-mode) writes all-ones and then zero to each writable
CSR in WRITE_SET, reading back after each write, and restores the original
value, which exposes the WARL masks.

Run the result on Spike and on hart with commit logging and compare the logs
with compare.py; csrmap.py turns the Spike log into a readable map.
"""

import sys

# CSRs whose write behavior is probed. Excluded on purpose: misa (writes can
# disable C/F/D under the probe itself), pmpcfg lock bits (see below), and
# mtvec/stvec are written last-and-restored so the handler stays reachable.
WRITE_SET = [
    0x001, 0x002, 0x003,                     # fflags frm fcsr
    0x100, 0x104, 0x105, 0x106, 0x10a,       # sstatus sie stvec scounteren senvcfg
    0x140, 0x141, 0x142, 0x143, 0x144,       # sscratch sepc scause stval sip
    0x180,                                   # satp
    0x300, 0x302, 0x303, 0x304, 0x306,       # mstatus medeleg mideleg mie mcounteren
    0x30a, 0x320,                            # menvcfg mcountinhibit
    0x340, 0x341, 0x342, 0x343, 0x344,       # mscratch mepc mcause mtval mip
    0x5a8,                                   # scontext
    0x7a0, 0x7a1, 0x7a2, 0x7a3, 0x7a4, 0x7a8,  # tselect tdata1-3 tinfo mcontext
    0xb00, 0xb02,                            # mcycle minstret
] + list(range(0x323, 0x340))                # mhpmevent3-31
WRITE_SET += [0xb03 + i for i in range(29)]  # mhpmcounter3-31
PMPCFG = [0x3a0 + i for i in range(0, 16, 2)]
PMPADDR = [0x3b0 + i for i in range(64)]

# Interrupt-enable and status bits that must not take effect while probing.
SAFE_LAST = [0x305]  # mtvec


def emit(out):
    w = out.write
    w("""\
    .section .text.init, "ax"
    .globl _start
_start:
    la      t0, trap
    csrw    mtvec, t0
    li      s0, 0          # phase: 0 = M reads, 1 = S reads, 2 = U reads, 3 = writes
""")
    w("phase_reads:\n")
    for a in range(0x1000):
        w(f"    csrrs   a0, 0x{a:03x}, x0\n")
    # Advance phase: from M via mret to S, from S/U via ecall back to M first.
    w("""\
    addi    s0, s0, 1
    li      t0, 3
    beq     s0, t0, phase_writes_from_lower
    ecall                  # back to M; the handler drops to the next mode
    j       phase_reads

phase_writes_from_lower:
    ecall
""")
    w("phase_writes:\n")
    w("    li      t1, 0x2000\n    csrs    mstatus, t1   # FS = Initial so fcsr is accessible\n")
    for a in WRITE_SET + PMPADDR:
        w(f"    csrrs   s1, 0x{a:03x}, x0\n")
        w(f"    li      t1, -1\n    csrw    0x{a:03x}, t1\n    csrrs   a0, 0x{a:03x}, x0\n")
        w(f"    csrw    0x{a:03x}, x0\n    csrrs   a0, 0x{a:03x}, x0\n")
        w(f"    csrw    0x{a:03x}, s1\n")
    # pmpcfg: never set L (bit 7 of each byte), which would also bind M-mode.
    for a in PMPCFG:
        w(f"    csrrs   s1, 0x{a:03x}, x0\n")
        w(f"    li      t1, 0x7f7f7f7f7f7f7f7f\n    csrw    0x{a:03x}, t1\n    csrrs   a0, 0x{a:03x}, x0\n")
        w(f"    csrw    0x{a:03x}, x0\n    csrrs   a0, 0x{a:03x}, x0\n")
        w(f"    csrw    0x{a:03x}, s1\n")
    # mstatus: write with MIE/SIE clear so no interrupt is taken, then restore.
    for a in [0x300, 0x100]:
        w(f"    csrrs   s1, 0x{a:03x}, x0\n")
        w(f"    li      t1, -11\n    csrw    0x{a:03x}, t1\n    csrrs   a0, 0x{a:03x}, x0\n")
        w(f"    csrw    0x{a:03x}, s1\n")
    for a in SAFE_LAST + [0x105]:
        w(f"    csrrs   s1, 0x{a:03x}, x0\n")
        w(f"    li      t1, -1\n    csrw    0x{a:03x}, t1\n    csrrs   a0, 0x{a:03x}, x0\n")
        w(f"    li      t1, -2\n    csrw    0x{a:03x}, t1\n    csrrs   a0, 0x{a:03x}, x0\n")
        w(f"    csrw    0x{a:03x}, s1\n")
    w("""\
    li      t0, 1
    la      t1, tohost
    sd      t0, 0(t1)
1:  j       1b

    .balign 4
trap:
    csrr    t2, mcause
    li      t3, 8
    bltu    t2, t3, skip   # exceptions other than ecall: skip the instruction
    li      t3, 11
    bgtu    t2, t3, skip
    # ecall from any mode: enter the mode for phase s0 (0 M, 1 S, 2 U, 3 M).
    # PMP entry 0 grants S and U access to all memory; it is set here, after
    # the M-mode read phase, so that phase still shows the reset values.
    li      t3, -1
    csrw    pmpaddr0, t3
    li      t3, 0x1f
    csrw    pmpcfg0, t3
    li      t3, 0x1800
    csrc    mstatus, t3
    li      t3, 1
    beq     s0, t3, to_s
    li      t3, 2
    beq     s0, t3, to_u
    li      t3, 0x1800     # MPP = M
    csrs    mstatus, t3
    j       ret_next
to_s:
    li      t3, 0x0800     # MPP = S
    csrs    mstatus, t3
    j       ret_next
to_u:                      # MPP = U (already clear)
ret_next:
    csrr    t2, mepc
    addi    t2, t2, 4
    csrw    mepc, t2
    mret
skip:
    csrr    t2, mepc
    addi    t2, t2, 4
    csrw    mepc, t2
    mret

    .section .tohost, "aw", @progbits
    .balign 8
    .globl tohost
tohost: .dword 0
    .balign 8
    .globl fromhost
fromhost: .dword 0
""")


if __name__ == "__main__":
    emit(sys.stdout)
