#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Summarize a CSR-probe commit log (see gen_csrprobe.py) as a CSR map.

    csrmap.py ELF LOG [--encoding riscv-isa-sim/riscv/encoding.h] > csrmap.txt

For each CSR address that is readable in at least one mode, prints the value
read in M, S, and U mode ("-" where the read trapped). Then, for each CSR in
the write phase, prints the values read back after each probe write.
"""

import argparse
import re
import subprocess
import sys

HEAD = re.compile(r"core\s+\d+: (\d) 0x([0-9a-f]+) \(0x([0-9a-f]+)\)(.*)$")
XVAL = re.compile(r"x(\d+)\s+0x([0-9a-f]+)")
MODE = {3: 0, 1: 1, 0: 2}


def symbols(elf, nm):
    out = subprocess.run([nm, elf], capture_output=True, text=True, check=True).stdout
    return {f[2]: int(f[0], 16) for f in (ln.split() for ln in out.splitlines()) if len(f) == 3}


def csr_names(path):
    names = {}
    if path:
        for m in re.finditer(r"#define CSR_(\w+) (0x[0-9a-f]+)", open(path).read()):
            names.setdefault(int(m.group(2), 16), m.group(1).lower())
    return names


def xreg(rest, n):
    for m in XVAL.finditer(rest):
        if int(m.group(1)) == n:
            return int(m.group(2), 16)
    return 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("elf")
    ap.add_argument("log")
    ap.add_argument("--encoding")
    ap.add_argument("--nm", default="riscv64-unknown-elf-nm")
    args = ap.parse_args()
    sym = symbols(args.elf, args.nm)
    names = csr_names(args.encoding)
    rd_base, wr_base, wr_end = sym["phase_reads"], sym["phase_writes"], sym["trap"]
    reads = [{}, {}, {}]
    writes = []
    for line in open(args.log):
        m = HEAD.match(line)
        if not m:
            continue
        priv, pc, insn, rest = int(m.group(1)), int(m.group(2), 16), int(m.group(3), 16), m.group(4)
        if rd_base <= pc < rd_base + 0x4000:
            addr = (pc - rd_base) // 4
            reads[MODE[priv]][addr] = xreg(rest, 10)
        elif wr_base <= pc < wr_end:
            if insn & 0xfffff == 0x024f3:      # csrrs s1, csr, x0: save, starts a CSR
                writes.append((insn >> 20, []))
            elif insn & 0xfffff == 0x02573 and writes:  # csrrs a0, csr, x0: read back
                writes[-1][1].append(xreg(rest, 10))
    allr = sorted(set(reads[0]) | set(reads[1]) | set(reads[2]))
    print("# CSR read map: value read in each mode, '-' = the read trapped")
    print(f"# {'addr':6} {'name':16} {'M':18} {'S':18} {'U':18}")
    for a in allr:
        cols = [f"0x{reads[p][a]:016x}" if a in reads[p] else "-" for p in range(3)]
        print(f"  0x{a:03x}  {names.get(a, '?'):16} {cols[0]:18} {cols[1]:18} {cols[2]:18}")
    print()
    print("# CSR write probe (M-mode): value read back after each write, in order.")
    print("#   most CSRs: write -1, then 0;  mstatus/sstatus: write -11 (MIE, SIE clear);")
    print("#   mtvec/stvec: write -1, then -2;  pmpcfg: write 0x7f7f7f7f7f7f7f7f, then 0")
    for a, vals in writes:
        print(f"  0x{a:03x}  {names.get(a, '?'):16} " + " ".join(f"0x{v:016x}" for v in vals))


if __name__ == "__main__":
    sys.exit(main())
