#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Random bare-metal RV64GC test-program generator for Spike differential tests.

    rvgen.py SEED > prog.S

Each seed deterministically selects a profile (execution mode M, S, or U;
bare or Sv39 translation; trap delegation; mstatus, counter-enable, and FP
state) and emits a random instruction stream covering integer, M, A, F, D,
and C instructions, misaligned accesses, CSR accesses, traps (ecall, ebreak,
illegal and reserved encodings, privilege violations, page and access faults),
and software and timer interrupts.

Termination is guaranteed: control flow only moves forward, every instruction
that may trap is preceded by `la x31, <next>`, and the trap handlers resume at
x31. The program ends with `ecall` while x31 == 0, and the M-mode handler then
writes 1 to the HTIF tohost word.

Reserved registers: x27-x29 hold data-region base addresses, x30 is handler
scratch, and x31 is the trap resume address. Random instructions never write
them.
"""

import random
import sys

GEN_VERSION = 2

TEXT = 0x80000000
TOHOST = 0x80010000
ROOT_PT = 0x80011000
L1_PT = 0x80012000
L0_PT = 0x80013000
DATA = 0x80020000           # 8 data pages
NPAGES = 8
MEGA_PA = 0x80200000        # 2 MiB megapage target
CLINT_MSIP = 0x02000000
CLINT_MTIMECMP = 0x02004000
CLINT_MTIME = 0x0200BFF8
DATA_VA = 0x40000000        # VPN2 = 1: L1 table, VPN1 0 -> L0 (4 KiB pages), VPN1 1 -> megapage
MEGA_VA = 0x40200000
USER_ALIAS = 0x40000000     # U-mode code runs at PA + this (gigapage VPN2 = 3)

PTE_V, PTE_R, PTE_W, PTE_X, PTE_U, PTE_G, PTE_A, PTE_D = (1 << i for i in range(8))

INT_POOL = [0, 1, 2, 3, 0x7f, 0x80, 0xff, 0x7fff, 0x8000, 0xffff, 0x7fffffff, 0x80000000,
            0xffffffff, 0xffffffff80000000, 0x7fffffffffffffff, 0x8000000000000000,
            0xffffffffffffffff, 0xfffffffffffffffe, 0x5555555555555555, 0xaaaaaaaaaaaaaaaa]
F64_POOL = [0x0000000000000000, 0x8000000000000000, 0x3ff0000000000000, 0xbff0000000000000,
            0x7ff0000000000000, 0xfff0000000000000, 0x7ff8000000000000, 0x7ff4000000000000,
            0xfff8000000000001, 0x0000000000000001, 0x000fffffffffffff, 0x0010000000000000,
            0x7fefffffffffffff, 0x4340000000000000, 0xc3e0000000000000, 0x41dfffffffc00000,
            0x3fe0000000000000, 0x400921fb54442d18]
F32_POOL = [0x00000000, 0x80000000, 0x3f800000, 0xbf800000, 0x7f800000, 0xff800000,
            0x7fc00000, 0x7fa00000, 0xffc00001, 0x00000001, 0x007fffff, 0x00800000,
            0x7f7fffff, 0x4f000000, 0xcf000000, 0x5f000000, 0x3f000000, 0x40490fdb]

ALU_RR = ["add", "sub", "sll", "slt", "sltu", "xor", "srl", "sra", "or", "and",
          "addw", "subw", "sllw", "srlw", "sraw",
          "mul", "mulh", "mulhsu", "mulhu", "div", "divu", "rem", "remu",
          "mulw", "divw", "divuw", "remw", "remuw"]
ALU_RI = ["addi", "slti", "sltiu", "xori", "ori", "andi", "addiw"]
SHIFT_I = [("slli", 64), ("srli", 64), ("srai", 64), ("slliw", 32), ("srliw", 32), ("sraiw", 32)]
BRANCH = ["beq", "bne", "blt", "bge", "bltu", "bgeu"]
LOADS = [("lb", 1), ("lh", 2), ("lw", 4), ("ld", 8), ("lbu", 1), ("lhu", 2), ("lwu", 4)]
STORES = [("sb", 1), ("sh", 2), ("sw", 4), ("sd", 8)]
AMO_OPS = ["amoswap", "amoadd", "amoxor", "amoand", "amoor", "amomin", "amomax", "amominu", "amomaxu"]
ORDER = ["", ".aq", ".rl", ".aqrl"]
RMS = ["rne", "rtz", "rdn", "rup", "rmm", "dyn"]
FP_RRR = ["fadd", "fsub", "fmul", "fdiv"]
FP_RR = ["fmin", "fmax", "fsgnj", "fsgnjn", "fsgnjx"]
FP_FMA = ["fmadd", "fmsub", "fnmsub", "fnmadd"]
FP_CMP = ["feq", "flt", "fle"]

# Raw encodings that must raise illegal-instruction (or are reserved) in RV64GC.
ILLEGAL_32 = [0x00000000 | 0x3, 0xffffffff, 0x0000707f, 0x02008033 | (0x7f << 25),
              0x00007053 | (1 << 25),                 # fadd.d with rm=7 handled via frm? (OP-FP rm field)
              0x0000505b, 0x0000605b,                 # custom-2 opcode
              0x00005053 | (0x00 << 25) | (5 << 12),  # fadd.s rm=5 (reserved)
              0x00006053 | (6 << 12),                 # fadd.s rm=6 (reserved)
              0x10200073 | (1 << 15),                 # sret with rs1 != 0 (reserved)
              0x7b200073,                             # dret (debug mode only)
              0x0000302f | (0x1f << 27),              # AMO with reserved funct5
              0x0000001b | (1 << 12) | (1 << 25),     # slliw with shamt[5] set
              0xc0001073]                             # csrw cycle, x0 (read-only CSR write)
ILLEGAL_16 = [0x0000,          # c.unimp / c.addi4spn imm=0
              0x6001 | (2 << 7),  # c.lui x2 (c.addi16sp) with imm=0: reserved
              0x6081,          # c.lui x1, 0: reserved (imm=0)
              0x2001,          # c.addiw x0: reserved
              0x8002,          # c.jr x0: reserved
              0x9c41 | (1 << 12) & 0,  # c.subw form (legal) -- replaced below
              0x8000 | (0 << 2)]       # c.srli with shamt 0: HINT? (legal hint)
ILLEGAL_16 = [x for x in ILLEGAL_16 if x not in (0x9c41,)]
HINT_32 = ["addi x0, x5, 7", "slti x0, x6, 1", "add x0, x7, x8", "slli x0, x9, 3",
           ".word 0x0020000f", ".word 0x0100000f", ".word 0x8330000f",
           "ori x0, x10, 0x20"]
HINT_16 = ["c.nop 5", "c.li x0, 7", "c.mv x0, x9", "c.add x0, x10", "c.slli x0, 3", ".half 0x0081"]

# CSRs that random code may access, per mode. (address, writable in body)
M_CSRS = [0x340, 0x341, 0x342, 0x343, 0x304, 0x344, 0x302, 0x303, 0x306, 0x106, 0x320,
          0x30a, 0x10a, 0x140, 0x141, 0x142, 0x143, 0x104, 0x144, 0x100, 0x300, 0x180,
          0x001, 0x002, 0x003, 0xb00, 0xb02, 0xc00, 0xc01, 0xc02, 0x301, 0xf11, 0xf12,
          0xf13, 0xf14, 0xf15, 0x3a0, 0x3a2, 0x3b1, 0x3b2, 0x3b5, 0x3bf, 0x3c0,
          0x323, 0xb03, 0xc03, 0x7a0, 0x5a8, 0x3ef, 0x7c0, 0x800, 0x015]
S_CSRS = [0x140, 0x141, 0x142, 0x143, 0x104, 0x144, 0x100, 0x180, 0x106, 0x10a,
          0x001, 0x002, 0x003, 0xc00, 0xc01, 0xc02, 0xc03, 0xc1f,
          0x300, 0x340, 0x305, 0xf14, 0x5c0, 0x015, 0x800, 0x9c0]
U_CSRS = [0x001, 0x002, 0x003, 0xc00, 0xc01, 0xc02, 0xc04, 0x140, 0x100, 0x800, 0xcc0]
# Writes to these from the body could break the harness itself; read only.
# misa is read-only too: Spike lets writes disable C, F, or D under the program.
NO_WRITE = {0x301, 0x305, 0x105, 0x180, 0x3a0, 0x3a2, 0x3b0}


class Gen:
    def __init__(self, seed):
        self.seed = seed
        self.r = random.Random(seed)
        self.lines = []
        self.nlabel = 0
        r = self.r
        self.mode = r.choices(["M", "S", "U"], weights=[20, 45, 35])[0]
        self.sv39 = self.mode != "M" and r.random() < 0.75
        self.fs_on = r.random() < 0.92
        self.interrupts = r.random() < 0.3

    # --- helpers -------------------------------------------------------
    def emit(self, s):
        self.lines.append("    " + s)

    def label(self):
        self.nlabel += 1
        return f"L{self.nlabel}"

    def place(self, lab):
        self.lines.append(f"{lab}:")

    def xr(self):        # destination register (never reserved)
        return f"x{self.r.randrange(0, 27)}"

    def xs(self):        # source register (may be a base register)
        return f"x{self.r.randrange(0, 30)}"

    def fr(self):
        return f"f{self.r.randrange(32)}"

    def resume(self):
        """Point x31 at the next instruction so a trap resumes there."""
        lab = self.label()
        self.emit(f"la x31, {lab}")
        return lab

    def guarded(self, insn):
        lab = self.resume()
        self.emit(insn)
        self.place(lab)

    def imm12(self):
        return self.r.choice([0, 1, -1, 2047, -2048, self.r.randrange(-2048, 2048)])

    # --- instruction classes ------------------------------------------
    def alu(self):
        r = self.r
        k = r.random()
        if k < 0.45:
            self.emit(f"{r.choice(ALU_RR)} {self.xr()}, {self.xs()}, {self.xs()}")
        elif k < 0.75:
            self.emit(f"{r.choice(ALU_RI)} {self.xr()}, {self.xs()}, {self.imm12()}")
        elif k < 0.9:
            op, w = r.choice(SHIFT_I)
            self.emit(f"{op} {self.xr()}, {self.xs()}, {r.randrange(w)}")
        elif k < 0.95:
            self.emit(f"lui {self.xr()}, {r.randrange(1 << 20)}")
        else:
            self.emit(f"auipc {self.xr()}, {r.randrange(1 << 20)}")

    def branch(self):
        r = self.r
        lab = self.label()
        if r.random() < 0.7:
            self.emit(f"{r.choice(BRANCH)} {self.xs()}, {self.xs()}, {lab}")
        elif r.random() < 0.5:
            self.emit(f"jal {self.xr()}, {lab}")
        else:
            t = self.xr()
            if t == "x0":
                t = "x5"
            self.emit(f"la {t}, {lab}")
            self.emit(f"jalr {self.xr()}, {r.choice([0, 1])}({t})")
        for _ in range(r.randrange(1, 3)):
            self.alu()
        self.place(lab)

    def base(self, store=False):
        """Base register for a memory access. Loads occasionally use any
        register; stores always use a data-region base, so they never modify
        code (fetching modified code without fence.i is implementation
        defined) or the page tables."""
        r = self.r
        if store or r.random() < 0.93:
            return r.choice(["x27", "x28", "x29"])
        return self.xs()

    def offset(self, size):
        r = self.r
        off = r.randrange(-2048, 2048 - 8)
        if r.random() < 0.6:
            off -= off % size
        return off

    def mem(self):
        r = self.r
        k = r.random()
        if k < 0.4:
            op, sz = r.choice(LOADS)
            self.guarded(f"{op} {self.xr()}, {self.offset(sz)}({self.base()})")
        elif k < 0.75:
            op, sz = r.choice(STORES)
            self.guarded(f"{op} {self.xs()}, {self.offset(sz)}({self.base(store=True)})")
        elif k < 0.9:
            sz = r.choice([4, 8])
            op = f"{r.choice(AMO_OPS)}.{'w' if sz == 4 else 'd'}{r.choice(ORDER)}"
            a = self.amo_addr(sz)
            self.guarded(f"{op} {self.xr()}, {self.xs()}, ({a})")
        else:
            sz = r.choice([4, 8])
            sfx = "w" if sz == 4 else "d"
            a = self.amo_addr(sz)
            self.guarded(f"lr.{sfx}{r.choice(ORDER)} {self.xr()}, ({a})")
            for _ in range(r.randrange(0, 3)):
                self.alu()
            if r.random() < 0.2:
                a = self.amo_addr(sz)
            self.guarded(f"sc.{sfx}{r.choice(ORDER)} {self.xr()}, {self.xs()}, ({a})")

    def amo_addr(self, size):
        """Materialize an AMO address (mostly aligned) in a temporary register."""
        r = self.r
        t = f"x{r.randrange(5, 27)}"
        off = r.randrange(-2048, 2048 - 8)
        if r.random() < 0.9:
            off -= off % size
        self.emit(f"addi {t}, {r.choice(['x27', 'x28', 'x29'])}, {off}")
        return t

    def fp(self):
        r = self.r
        p = r.choice(["s", "d"])
        rm = r.choice(RMS)
        k = r.random()
        if k < 0.3:
            ins = f"{r.choice(FP_RRR)}.{p} {self.fr()}, {self.fr()}, {self.fr()}, {rm}"
        elif k < 0.38:
            ins = f"fsqrt.{p} {self.fr()}, {self.fr()}, {rm}"
        elif k < 0.5:
            ins = f"{r.choice(FP_RR)}.{p} {self.fr()}, {self.fr()}, {self.fr()}"
        elif k < 0.62:
            ins = f"{r.choice(FP_FMA)}.{p} {self.fr()}, {self.fr()}, {self.fr()}, {self.fr()}, {rm}"
        elif k < 0.7:
            ins = f"{r.choice(FP_CMP)}.{p} {self.xr()}, {self.fr()}, {self.fr()}"
        elif k < 0.74:
            ins = f"fclass.{p} {self.xr()}, {self.fr()}"
        elif k < 0.82:
            it = r.choice(["w", "wu", "l", "lu"])
            ins = f"fcvt.{it}.{p} {self.xr()}, {self.fr()}, {rm}"
        elif k < 0.9:
            it = r.choice(["w", "wu", "l", "lu"])
            exact = p == "d" and it in ("w", "wu")
            ins = f"fcvt.{p}.{it} {self.fr()}, {self.xs()}" + ("" if exact else f", {rm}")
        elif k < 0.93:
            ins = (f"fcvt.s.d {self.fr()}, {self.fr()}, {rm}" if p == "s"
                   else f"fcvt.d.s {self.fr()}, {self.fr()}")
        elif k < 0.97:
            ins = r.choice([f"fmv.x.w {self.xr()}, {self.fr()}", f"fmv.w.x {self.fr()}, {self.xs()}",
                            f"fmv.x.d {self.xr()}, {self.fr()}", f"fmv.d.x {self.fr()}, {self.xs()}"])
        else:
            op, sz = r.choice([("flw", 4), ("fld", 8), ("fsw", 4), ("fsd", 8)])
            ins = f"{op} {self.fr()}, {self.offset(sz)}({self.base(store=op.startswith('fs'))})"
        self.guarded(ins)

    def csr(self):
        r = self.r
        table = {"M": M_CSRS, "S": S_CSRS, "U": U_CSRS}[self.mode]
        c = r.choice(table) if r.random() < 0.95 else r.randrange(0x1000)
        op = r.choice(["csrrw", "csrrs", "csrrc", "csrrwi", "csrrsi", "csrrci"])
        if c in NO_WRITE or (c == 0x300 and self.mode == "M" and r.random() < 0.5):
            op = "csrrs"
            src = "x0"
        elif op.endswith("i"):
            src = str(r.randrange(32))
        else:
            src = self.xs()
        if self.mode != "M" and c in (0x100,) and op in ("csrrw", "csrrwi"):
            op = op.replace("w", "s", 1)  # keep sstatus.SPP/SIE mostly intact
        self.guarded(f"{op} {self.xr()}, 0x{c:03x}, {src}")

    def fcsr_write(self):
        r = self.r
        v = r.randrange(8) if r.random() < 0.15 else r.randrange(5)
        self.guarded(f"csrwi frm, {v}")

    def system(self):
        r = self.r
        k = r.random()
        if k < 0.25:
            self.guarded("ecall")
        elif k < 0.4:
            self.guarded(r.choice(["ebreak", "c.ebreak"]))
        elif k < 0.55:
            self.guarded(f".word 0x{r.choice(ILLEGAL_32):08x}")
        elif k < 0.65:
            self.guarded(f".half 0x{r.choice(ILLEGAL_16):04x}")
        elif k < 0.75:
            ins = {"M": ["fence", "fence.i", "fence rw, rw", "sfence.vma", "sfence.vma x0, x5"],
                   "S": ["mret", "fence.i", "sfence.vma", "sfence.vma x6, x0", "wfi"],
                   "U": ["mret", "sret", "wfi", "sfence.vma", "fence.i"]}[self.mode]
            pick = r.choice(ins)
            if pick == "wfi" and self.mode == "S" and not self.tw:
                pick = "fence"
            self.guarded(pick)
        elif k < 0.85:
            self.emit(r.choice(HINT_32))
        else:
            self.emit(r.choice(HINT_16))

    def interrupt_event(self):
        r = self.r
        k = r.random()
        if self.mode == "M":
            if k < 0.35:
                self.guarded(f"csrsi mip, {r.choice([2, 2, 0])}")
            elif k < 0.6:
                self.guarded(f"csrs mie, {self.xs()}")
            elif k < 0.75:
                self.guarded("csrsi mstatus, 8")
            elif k < 0.9:
                # Arm the timer a little in the future: mtimecmp = mtime + delta.
                self.emit(f"li x30, 0x{CLINT_MTIME:x}")
                self.emit("ld x30, 0(x30)")
                self.emit(f"addi x30, x30, {r.choice([1, 20, 50, 51, 100, 300])}")
                self.emit(f"li x31, 0x{CLINT_MTIMECMP:x}")
                self.emit("sd x30, 0(x31)")
            else:
                self.emit(f"li x30, 0x{CLINT_MSIP:x}")
                self.emit("li x31, 1")
                self.emit("sw x31, 0(x30)")
        elif self.mode == "S":
            if k < 0.5:
                self.guarded("csrsi sip, 2")
            elif k < 0.8:
                self.guarded(f"csrsi sie, {r.choice([2, 2, 0x0])}")
            else:
                self.guarded("csrsi sstatus, 2")

    def body(self, n):
        r = self.r
        weights = {"alu": 30, "branch": 8, "mem": 22, "fp": 18 if self.fs_on else 3,
                   "csr": 9, "system": 6, "frm": 1, "irq": 4 if self.interrupts else 0}
        kinds = list(weights)
        for _ in range(n):
            k = r.choices(kinds, weights=[weights[x] for x in kinds])[0]
            {"alu": self.alu, "branch": self.branch, "mem": self.mem, "fp": self.fp,
             "csr": self.csr, "system": self.system, "frm": self.fcsr_write,
             "irq": self.interrupt_event}[k]()

    # --- page tables and data -----------------------------------------
    def page_tables(self):
        r = self.r
        user = self.mode == "U"
        root = [0] * 512
        code_flags = PTE_V | PTE_R | PTE_W | PTE_X | PTE_A | PTE_D
        root[2] = ((TEXT >> 12) << 10) | code_flags                       # S code (U=0)
        root[3] = ((TEXT >> 12) << 10) | code_flags | PTE_U               # U code alias
        root[1] = ((L1_PT >> 12) << 10) | PTE_V
        l1 = [0] * 512
        l1[0] = ((L0_PT >> 12) << 10) | PTE_V
        mflags = PTE_V | PTE_R | PTE_W | PTE_A | PTE_D | (PTE_U if user else 0)
        mpa = MEGA_PA if r.random() < 0.85 else MEGA_PA + 0x1000   # misaligned superpage
        l1[1] = ((mpa >> 12) << 10) | mflags
        l0 = [0] * 512
        normal = PTE_V | PTE_R | PTE_W | PTE_A | PTE_D | (PTE_U if user else 0)
        choices = [normal, normal, PTE_V | PTE_R | PTE_A | (PTE_U if user else 0),
                   PTE_V | PTE_X | PTE_A | (PTE_U if user else 0),
                   PTE_V | PTE_R | PTE_W | PTE_D | (PTE_U if user else 0),          # A = 0
                   PTE_V | PTE_R | PTE_W | PTE_A | (PTE_U if user else 0),          # D = 0
                   PTE_V | PTE_W | PTE_A | PTE_D,                                    # W without R
                   normal ^ PTE_U, PTE_R | PTE_W | PTE_A | PTE_D,                    # V = 0
                   normal | PTE_X | PTE_G]
        for i in range(NPAGES):
            flags = normal if i < 2 else r.choice(choices)
            l0[i] = (((DATA + i * 0x1000) >> 12) << 10) | flags
        return root, l1, l0

    def bases(self):
        r = self.r
        if self.sv39:
            pages = [DATA_VA + i * 0x1000 for i in range(NPAGES)] + [MEGA_VA + 0x800]
        else:
            pages = [DATA + i * 0x1000 for i in range(NPAGES)] + [MEGA_PA + 0x800]
        return [pages[0] + 0x800, r.choice(pages[1:]), r.choice(pages)]

    # --- whole program --------------------------------------------------
    def program(self):
        r = self.r
        self.tw = r.random() < 0.5
        n = r.randrange(300, 900)
        w = self.lines.append
        w(f"# rvgen v{GEN_VERSION} seed {self.seed}: mode={self.mode} sv39={self.sv39} "
          f"fs={self.fs_on} irq={self.interrupts}")
        # No linker relaxation: data and page tables sit at fixed .org offsets.
        w("    .option norelax")
        w("    .section .text.init, \"ax\", @progbits")
        w("    .globl _start")
        w("_start:")
        self.emit("la x30, m_trap")
        self.emit("csrw mtvec, x30")
        self.emit("li x30, -1")
        self.emit("csrw pmpaddr0, x30")
        self.emit("li x30, 0x1f")
        self.emit("csrw pmpcfg0, x30")
        self.emit("li x30, 0x6000")
        self.emit("csrs mstatus, x30")            # FS = Dirty while loading FP state
        for i in range(32):
            if r.random() < 0.5:
                v = r.choice(F64_POOL) if r.random() < 0.7 else r.getrandbits(64)
            else:
                s = r.choice(F32_POOL) if r.random() < 0.7 else r.getrandbits(32)
                v = (0xffffffff00000000 | s) if r.random() < 0.9 else (r.getrandbits(32) << 32) | s
            self.emit(f"li x30, 0x{v:016x}")
            self.emit(f"fmv.d.x f{i}, x30")
        self.emit(f"li x30, {r.randrange(5) << 5}")
        self.emit("csrw fcsr, x30")
        deleg_ok = [0, 3, 4, 5, 6, 7, 8, 12, 13, 15, 2, 1]
        medeleg = sum(1 << c for c in deleg_ok if r.random() < 0.4) if self.mode != "M" else 0
        mideleg = sum(1 << c for c in (1, 5, 9) if r.random() < 0.5) if self.mode != "M" else 0
        tsr = r.random() < 0.2
        if tsr:
            # The S handler's sret would trap to M, whose mret then resumes in
            # S-mode at a U-mode address: delegate nothing.
            medeleg = mideleg = 0
        self.emit(f"li x30, 0x{medeleg:x}")
        self.emit("csrw medeleg, x30")
        self.emit(f"li x30, 0x{mideleg:x}")
        self.emit("csrw mideleg, x30")
        self.emit(f"li x30, 0x{r.getrandbits(32) if r.random() < 0.7 else 0xffffffff:x}")
        self.emit("csrw mcounteren, x30")
        self.emit(f"li x30, 0x{r.getrandbits(32) if r.random() < 0.7 else 0xffffffff:x}")
        self.emit("csrw scounteren, x30")
        self.emit("la x30, s_trap")
        self.emit("csrw stvec, x30")
        if self.sv39:
            self.emit(f"li x30, 0x{(8 << 60) | (ROOT_PT >> 12):x}")
            self.emit("csrw satp, x30")
            self.emit("sfence.vma")
        # mstatus: FS, SUM, MXR, TVM, TW, TSR
        fs = r.choice([1, 2, 3]) if self.fs_on else 0
        bits = (fs << 13) | (r.random() < 0.5) << 18 | (r.random() < 0.3) << 19
        bits |= (r.random() < 0.2) << 20 | self.tw << 21 | tsr << 22
        self.emit("li x30, 0x7e6000")
        self.emit("csrc mstatus, x30")
        self.emit(f"li x30, 0x{bits:x}")
        self.emit("csrs mstatus, x30")
        for i in range(1, 27):
            v = r.choice(INT_POOL) if r.random() < 0.6 else r.getrandbits(64)
            self.emit(f"li x{i}, 0x{v:x}")
        for reg, b in zip(("x27", "x28", "x29"), self.bases()):
            self.emit(f"li {reg}, 0x{b:x}")
        if self.mode == "M":
            self.emit("j body")
        else:
            self.emit("li x30, 0x1800")
            self.emit("csrc mstatus, x30")
            if self.mode == "S":
                self.emit("li x30, 0x800")
                self.emit("csrs mstatus, x30")
                self.emit("la x30, body")
            else:
                self.emit("la x30, body")
                if self.sv39:
                    self.emit(f"li x31, 0x{USER_ALIAS:x}")
                    self.emit("add x30, x30, x31")
            self.emit("csrw mepc, x30")
            self.emit("li x31, 0")
            self.emit("mret")
        w("body:")
        self.body(n)
        self.emit("li x31, 0")
        self.emit("ecall")
        self.emit("j .")
        self.handlers()
        self.data()
        return "\n".join(self.lines) + "\n"

    def handlers(self):
        w = self.lines.append
        w(f"""
    .balign 4
m_trap:
    csrr    x30, mcause
    bltz    x30, m_irq
    beqz    x31, m_exit
    csrw    mepc, x31
    mret
m_exit:
    la      x30, tohost
    li      x31, 1
    sd      x31, 0(x30)
1:  j       1b
m_irq:
    csrw    mscratch, x31
    slli    x30, x30, 1
    srli    x30, x30, 1
    li      x31, 1
    sll     x31, x31, x30
    csrc    mie, x31          # disable the source so it cannot storm
    csrc    mip, x31
    li      x31, 7
    bne     x30, x31, 2f
    li      x30, 0x{CLINT_MTIMECMP:x}
    li      x31, -1
    sd      x31, 0(x30)
2:  li      x31, 3
    bne     x30, x31, 3f
    li      x30, 0x{CLINT_MSIP:x}
    sw      x0, 0(x30)
3:  csrr    x31, mscratch
    mret

    .balign 4
s_trap:
    csrr    x30, scause
    bltz    x30, s_irq
    beqz    x31, s_exit
    csrw    sepc, x31
    sret
s_exit:
    ecall                     # ecall from S is never delegated: M handler exits
1:  j       1b
s_irq:
    csrw    sscratch, x31
    slli    x30, x30, 1
    srli    x30, x30, 1
    li      x31, 1
    sll     x31, x31, x30
    csrc    sie, x31
    csrc    sip, x31
    csrr    x31, sscratch
    sret
""")

    def data(self):
        r = self.r
        w = self.lines.append
        w(f"    .org 0x{TOHOST - TEXT:x}")
        w("    .globl tohost\ntohost: .dword 0\n    .globl fromhost\nfromhost: .dword 0")
        root, l1, l0 = self.page_tables()
        for addr, table in ((ROOT_PT, root), (L1_PT, l1), (L0_PT, l0)):
            w(f"    .org 0x{addr - TEXT:x}")
            for i in range(0, 512, 4):
                w("    .dword " + ", ".join(f"0x{v:x}" for v in table[i:i + 4]))
        w(f"    .org 0x{DATA - TEXT:x}")
        for i in range(NPAGES * 512 // 4):
            vals = [r.choice(INT_POOL) if r.random() < 0.3 else r.getrandbits(64) for _ in range(4)]
            w("    .dword " + ", ".join(f"0x{v:x}" for v in vals))


def main():
    seed = int(sys.argv[1])
    sys.stdout.write(Gen(seed).program())


if __name__ == "__main__":
    main()
