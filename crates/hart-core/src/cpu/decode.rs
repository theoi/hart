//! Instruction decoding, including all compressed (C) encodings.
//!
//! `decode` turns raw instruction bits into a compact [`Insn`] (8 bytes) that the
//! executor dispatches on. Compressed instructions decode directly to the equivalent
//! base operation. FP instructions decode to [`Op::Fp`] with the 32-bit encoding in
//! `imm` (compressed FP loads and stores are expanded first).

/// Length in bytes of the instruction whose low 16 bits are `lo`.
#[inline]
pub fn insn_len(lo: u16) -> u8 {
    if lo & 3 == 3 {
        4
    } else {
        2
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Op {
    Illegal,
    Lui,
    Auipc,
    Jal,
    Jalr,
    Beq,
    Bne,
    Blt,
    Bge,
    Bltu,
    Bgeu,
    Lb,
    Lh,
    Lw,
    Ld,
    Lbu,
    Lhu,
    Lwu,
    Sb,
    Sh,
    Sw,
    Sd,
    Addi,
    Slti,
    Sltiu,
    Xori,
    Ori,
    Andi,
    Slli,
    Srli,
    Srai,
    Addiw,
    Slliw,
    Srliw,
    Sraiw,
    Add,
    Sub,
    Sll,
    Slt,
    Sltu,
    Xor,
    Srl,
    Sra,
    Or,
    And,
    Addw,
    Subw,
    Sllw,
    Srlw,
    Sraw,
    Mul,
    Mulh,
    Mulhsu,
    Mulhu,
    Div,
    Divu,
    Rem,
    Remu,
    Mulw,
    Divw,
    Divuw,
    Remw,
    Remuw,
    LrW,
    ScW,
    AmoswapW,
    AmoaddW,
    AmoxorW,
    AmoandW,
    AmoorW,
    AmominW,
    AmomaxW,
    AmominuW,
    AmomaxuW,
    LrD,
    ScD,
    AmoswapD,
    AmoaddD,
    AmoxorD,
    AmoandD,
    AmoorD,
    AmominD,
    AmomaxD,
    AmominuD,
    AmomaxuD,
    Fence,
    FenceI,
    Ecall,
    Ebreak,
    Mret,
    Sret,
    Wfi,
    SfenceVma,
    /// CSR ops: `imm` is the CSR address; for the `*i` forms `rs1` is the 5-bit immediate.
    Csrrw,
    Csrrs,
    Csrrc,
    Csrrwi,
    Csrrsi,
    Csrrci,
    /// F/D instruction, dispatched to `cpu::fp`. `imm` holds the 32-bit encoding.
    Fp,
}

impl Op {
    #[inline]
    pub fn is_csr(self) -> bool {
        matches!(self, Op::Csrrw | Op::Csrrs | Op::Csrrc | Op::Csrrwi | Op::Csrrsi | Op::Csrrci)
    }
}

/// A decoded instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Insn {
    pub op: Op,
    pub rd: u8,
    pub rs1: u8,
    pub rs2: u8,
    pub imm: i32,
}

impl Insn {
    pub const ILLEGAL: Insn = Insn { op: Op::Illegal, rd: 0, rs1: 0, rs2: 0, imm: 0 };

    #[inline]
    const fn new(op: Op, rd: u32, rs1: u32, rs2: u32, imm: i32) -> Insn {
        Insn { op, rd: rd as u8, rs1: rs1 as u8, rs2: rs2 as u8, imm }
    }
}

/// Decode an instruction. `raw` holds 32 bits for standard encodings or a compressed
/// encoding in the low 16 bits (upper bits ignored).
#[inline]
pub fn decode(raw: u32) -> Insn {
    if raw & 3 == 3 {
        decode32(raw)
    } else {
        decode16(raw as u16)
    }
}

#[inline]
fn bits(x: u32, hi: u32, lo: u32) -> u32 {
    (x >> lo) & ((1 << (hi - lo + 1)) - 1)
}

/// Sign-extend the low `n` bits of `x`.
#[inline]
fn sext(x: u32, n: u32) -> i32 {
    ((x << (32 - n)) as i32) >> (32 - n)
}

fn decode32(i: u32) -> Insn {
    use Op::*;
    let rd = bits(i, 11, 7);
    let rs1 = bits(i, 19, 15);
    let rs2 = bits(i, 24, 20);
    let f3 = bits(i, 14, 12);
    let f7 = bits(i, 31, 25);
    let imm_i = (i as i32) >> 20;
    let imm_s = ((i as i32) >> 25 << 5) | bits(i, 11, 7) as i32;
    let imm_b =
        sext((bits(i, 31, 31) << 12) | (bits(i, 7, 7) << 11) | (bits(i, 30, 25) << 5) | (bits(i, 11, 8) << 1), 13);
    let imm_u = (i & 0xffff_f000) as i32;
    let imm_j =
        sext((bits(i, 31, 31) << 20) | (bits(i, 19, 12) << 12) | (bits(i, 20, 20) << 11) | (bits(i, 30, 21) << 1), 21);
    let r = |op| Insn::new(op, rd, rs1, rs2, 0);
    let ii = |op| Insn::new(op, rd, rs1, 0, imm_i);
    match i & 0x7f {
        0x37 => Insn::new(Lui, rd, 0, 0, imm_u),
        0x17 => Insn::new(Auipc, rd, 0, 0, imm_u),
        0x6f => Insn::new(Jal, rd, 0, 0, imm_j),
        0x67 if f3 == 0 => ii(Jalr),
        0x63 => {
            let op = match f3 {
                0 => Beq,
                1 => Bne,
                4 => Blt,
                5 => Bge,
                6 => Bltu,
                7 => Bgeu,
                _ => return Insn::ILLEGAL,
            };
            Insn::new(op, 0, rs1, rs2, imm_b)
        }
        0x03 => {
            let op = match f3 {
                0 => Lb,
                1 => Lh,
                2 => Lw,
                3 => Ld,
                4 => Lbu,
                5 => Lhu,
                6 => Lwu,
                _ => return Insn::ILLEGAL,
            };
            ii(op)
        }
        0x23 => {
            let op = match f3 {
                0 => Sb,
                1 => Sh,
                2 => Sw,
                3 => Sd,
                _ => return Insn::ILLEGAL,
            };
            Insn::new(op, 0, rs1, rs2, imm_s)
        }
        0x13 => {
            let shamt = bits(i, 25, 20) as i32;
            match f3 {
                0 => ii(Addi),
                1 if f7 >> 1 == 0 => Insn::new(Slli, rd, rs1, 0, shamt),
                2 => ii(Slti),
                3 => ii(Sltiu),
                4 => ii(Xori),
                5 if f7 >> 1 == 0 => Insn::new(Srli, rd, rs1, 0, shamt),
                5 if f7 >> 1 == 0x10 => Insn::new(Srai, rd, rs1, 0, shamt),
                6 => ii(Ori),
                7 => ii(Andi),
                _ => Insn::ILLEGAL,
            }
        }
        0x1b => {
            let shamt = rs2 as i32;
            match (f3, f7) {
                (0, _) => ii(Addiw),
                (1, 0) => Insn::new(Slliw, rd, rs1, 0, shamt),
                (5, 0) => Insn::new(Srliw, rd, rs1, 0, shamt),
                (5, 0x20) => Insn::new(Sraiw, rd, rs1, 0, shamt),
                _ => Insn::ILLEGAL,
            }
        }
        0x33 => {
            let op = match (f7, f3) {
                (0, 0) => Add,
                (0x20, 0) => Sub,
                (0, 1) => Sll,
                (0, 2) => Slt,
                (0, 3) => Sltu,
                (0, 4) => Xor,
                (0, 5) => Srl,
                (0x20, 5) => Sra,
                (0, 6) => Or,
                (0, 7) => And,
                (1, 0) => Mul,
                (1, 1) => Mulh,
                (1, 2) => Mulhsu,
                (1, 3) => Mulhu,
                (1, 4) => Div,
                (1, 5) => Divu,
                (1, 6) => Rem,
                (1, 7) => Remu,
                _ => return Insn::ILLEGAL,
            };
            r(op)
        }
        0x3b => {
            let op = match (f7, f3) {
                (0, 0) => Addw,
                (0x20, 0) => Subw,
                (0, 1) => Sllw,
                (0, 5) => Srlw,
                (0x20, 5) => Sraw,
                (1, 0) => Mulw,
                (1, 4) => Divw,
                (1, 5) => Divuw,
                (1, 6) => Remw,
                (1, 7) => Remuw,
                _ => return Insn::ILLEGAL,
            };
            r(op)
        }
        0x2f => {
            let f5 = bits(i, 31, 27);
            let op = match (f3, f5) {
                (2, 0x02) if rs2 == 0 => LrW,
                (2, 0x03) => ScW,
                (2, 0x01) => AmoswapW,
                (2, 0x00) => AmoaddW,
                (2, 0x04) => AmoxorW,
                (2, 0x0c) => AmoandW,
                (2, 0x08) => AmoorW,
                (2, 0x10) => AmominW,
                (2, 0x14) => AmomaxW,
                (2, 0x18) => AmominuW,
                (2, 0x1c) => AmomaxuW,
                (3, 0x02) if rs2 == 0 => LrD,
                (3, 0x03) => ScD,
                (3, 0x01) => AmoswapD,
                (3, 0x00) => AmoaddD,
                (3, 0x04) => AmoxorD,
                (3, 0x0c) => AmoandD,
                (3, 0x08) => AmoorD,
                (3, 0x10) => AmominD,
                (3, 0x14) => AmomaxD,
                (3, 0x18) => AmominuD,
                (3, 0x1c) => AmomaxuD,
                _ => return Insn::ILLEGAL,
            };
            r(op)
        }
        0x0f => match f3 {
            0 => Insn::new(Fence, 0, 0, 0, 0),
            1 => Insn::new(FenceI, 0, 0, 0, 0),
            _ => Insn::ILLEGAL,
        },
        0x73 => {
            let csr = bits(i, 31, 20) as i32;
            match f3 {
                0 => match i {
                    0x0000_0073 => Insn::new(Ecall, 0, 0, 0, 0),
                    0x0010_0073 => Insn::new(Ebreak, 0, 0, 0, 0),
                    0x3020_0073 => Insn::new(Mret, 0, 0, 0, 0),
                    0x1020_0073 => Insn::new(Sret, 0, 0, 0, 0),
                    0x1050_0073 => Insn::new(Wfi, 0, 0, 0, 0),
                    _ if i & 0xfe00_7fff == 0x1200_0073 => Insn::new(SfenceVma, 0, rs1, rs2, 0),
                    _ => Insn::ILLEGAL,
                },
                1 => Insn::new(Csrrw, rd, rs1, 0, csr),
                2 => Insn::new(Csrrs, rd, rs1, 0, csr),
                3 => Insn::new(Csrrc, rd, rs1, 0, csr),
                5 => Insn::new(Csrrwi, rd, rs1, 0, csr),
                6 => Insn::new(Csrrsi, rd, rs1, 0, csr),
                7 => Insn::new(Csrrci, rd, rs1, 0, csr),
                _ => Insn::ILLEGAL,
            }
        }
        0x07 | 0x27 | 0x53 | 0x43 | 0x47 | 0x4b | 0x4f => Insn::new(Fp, rd, rs1, rs2, i as i32),
        _ => Insn::ILLEGAL,
    }
}

/// 32-bit FLD encoding.
fn enc_fld(rd: u32, rs1: u32, imm: u32) -> u32 {
    (imm << 20) | (rs1 << 15) | (3 << 12) | (rd << 7) | 0x07
}

/// 32-bit FSD encoding.
fn enc_fsd(rs2: u32, rs1: u32, imm: u32) -> u32 {
    ((imm >> 5) << 25) | (rs2 << 20) | (rs1 << 15) | (3 << 12) | ((imm & 0x1f) << 7) | 0x27
}

fn decode16(c: u16) -> Insn {
    use Op::*;
    let c = c as u32;
    let f3 = bits(c, 15, 13);
    // Full-width register fields.
    let rd = bits(c, 11, 7);
    let rs2 = bits(c, 6, 2);
    // Compressed register fields (x8..x15).
    let rdp = bits(c, 4, 2) + 8;
    let rs1p = bits(c, 9, 7) + 8;
    // CI-format 6-bit signed immediate: imm[5] = c[12], imm[4:0] = c[6:2].
    let imm6 = sext((bits(c, 12, 12) << 5) | bits(c, 6, 2), 6);
    let shamt = ((bits(c, 12, 12) << 5) | bits(c, 6, 2)) as i32;
    let n = |op, rd, rs1, rs2, imm| Insn::new(op, rd, rs1, rs2, imm);
    match (c & 3, f3) {
        // ---- quadrant 0 ----
        (0, 0) => {
            let imm = (bits(c, 12, 11) << 4) | (bits(c, 10, 7) << 6) | (bits(c, 6, 6) << 2) | (bits(c, 5, 5) << 3);
            if imm == 0 {
                return Insn::ILLEGAL;
            }
            n(Addi, rdp, 2, 0, imm as i32)
        }
        (0, 1) => {
            let imm = (bits(c, 12, 10) << 3) | (bits(c, 6, 5) << 6);
            n(Fp, rdp, rs1p, 0, enc_fld(rdp, rs1p, imm) as i32)
        }
        (0, 2) => {
            let imm = (bits(c, 12, 10) << 3) | (bits(c, 6, 6) << 2) | (bits(c, 5, 5) << 6);
            n(Lw, rdp, rs1p, 0, imm as i32)
        }
        (0, 3) => {
            let imm = (bits(c, 12, 10) << 3) | (bits(c, 6, 5) << 6);
            n(Ld, rdp, rs1p, 0, imm as i32)
        }
        (0, 5) => {
            let imm = (bits(c, 12, 10) << 3) | (bits(c, 6, 5) << 6);
            n(Fp, 0, rs1p, rdp, enc_fsd(rdp, rs1p, imm) as i32)
        }
        (0, 6) => {
            let imm = (bits(c, 12, 10) << 3) | (bits(c, 6, 6) << 2) | (bits(c, 5, 5) << 6);
            n(Sw, 0, rs1p, rdp, imm as i32)
        }
        (0, 7) => {
            let imm = (bits(c, 12, 10) << 3) | (bits(c, 6, 5) << 6);
            n(Sd, 0, rs1p, rdp, imm as i32)
        }
        // ---- quadrant 1 ----
        (1, 0) => n(Addi, rd, rd, 0, imm6),
        (1, 1) => {
            if rd == 0 {
                return Insn::ILLEGAL;
            }
            n(Addiw, rd, rd, 0, imm6)
        }
        (1, 2) => n(Addi, rd, 0, 0, imm6),
        (1, 3) => {
            if rd == 2 {
                let imm = sext(
                    (bits(c, 12, 12) << 9)
                        | (bits(c, 6, 6) << 4)
                        | (bits(c, 5, 5) << 6)
                        | (bits(c, 4, 3) << 7)
                        | (bits(c, 2, 2) << 5),
                    10,
                );
                if imm == 0 {
                    return Insn::ILLEGAL;
                }
                n(Addi, 2, 2, 0, imm)
            } else {
                if imm6 == 0 {
                    return Insn::ILLEGAL;
                }
                n(Lui, rd, 0, 0, imm6 << 12)
            }
        }
        (1, 4) => match bits(c, 11, 10) {
            0 => n(Srli, rs1p, rs1p, 0, shamt),
            1 => n(Srai, rs1p, rs1p, 0, shamt),
            2 => n(Andi, rs1p, rs1p, 0, imm6),
            _ => {
                let op = match (bits(c, 12, 12), bits(c, 6, 5)) {
                    (0, 0) => Sub,
                    (0, 1) => Xor,
                    (0, 2) => Or,
                    (0, 3) => And,
                    (1, 0) => Subw,
                    (1, 1) => Addw,
                    _ => return Insn::ILLEGAL,
                };
                n(op, rs1p, rs1p, rdp, 0)
            }
        },
        (1, 5) => {
            let imm = sext(
                (bits(c, 12, 12) << 11)
                    | (bits(c, 11, 11) << 4)
                    | (bits(c, 10, 9) << 8)
                    | (bits(c, 8, 8) << 10)
                    | (bits(c, 7, 7) << 6)
                    | (bits(c, 6, 6) << 7)
                    | (bits(c, 5, 3) << 1)
                    | (bits(c, 2, 2) << 5),
                12,
            );
            n(Jal, 0, 0, 0, imm)
        }
        (1, 6) | (1, 7) => {
            let imm = sext(
                (bits(c, 12, 12) << 8)
                    | (bits(c, 11, 10) << 3)
                    | (bits(c, 6, 5) << 6)
                    | (bits(c, 4, 3) << 1)
                    | (bits(c, 2, 2) << 5),
                9,
            );
            n(if f3 == 6 { Beq } else { Bne }, 0, rs1p, 0, imm)
        }
        // ---- quadrant 2 ----
        (2, 0) => n(Slli, rd, rd, 0, shamt),
        (2, 1) => {
            let imm = (bits(c, 12, 12) << 5) | (bits(c, 6, 5) << 3) | (bits(c, 4, 2) << 6);
            n(Fp, rd, 2, 0, enc_fld(rd, 2, imm) as i32)
        }
        (2, 2) => {
            if rd == 0 {
                return Insn::ILLEGAL;
            }
            let imm = (bits(c, 12, 12) << 5) | (bits(c, 6, 4) << 2) | (bits(c, 3, 2) << 6);
            n(Lw, rd, 2, 0, imm as i32)
        }
        (2, 3) => {
            if rd == 0 {
                return Insn::ILLEGAL;
            }
            let imm = (bits(c, 12, 12) << 5) | (bits(c, 6, 5) << 3) | (bits(c, 4, 2) << 6);
            n(Ld, rd, 2, 0, imm as i32)
        }
        (2, 4) => {
            if bits(c, 12, 12) == 0 {
                if rs2 == 0 {
                    if rd == 0 {
                        return Insn::ILLEGAL;
                    }
                    n(Jalr, 0, rd, 0, 0)
                } else {
                    n(Add, rd, 0, rs2, 0)
                }
            } else if rs2 == 0 {
                if rd == 0 {
                    n(Ebreak, 0, 0, 0, 0)
                } else {
                    n(Jalr, 1, rd, 0, 0)
                }
            } else {
                n(Add, rd, rd, rs2, 0)
            }
        }
        (2, 5) => {
            let imm = (bits(c, 12, 10) << 3) | (bits(c, 9, 7) << 6);
            n(Fp, 0, 2, rs2, enc_fsd(rs2, 2, imm) as i32)
        }
        (2, 6) => {
            let imm = (bits(c, 12, 9) << 2) | (bits(c, 8, 7) << 6);
            n(Sw, 0, 2, rs2, imm as i32)
        }
        (2, 7) => {
            let imm = (bits(c, 12, 10) << 3) | (bits(c, 9, 7) << 6);
            n(Sd, 0, 2, rs2, imm as i32)
        }
        _ => Insn::ILLEGAL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(raw: u32) -> Insn {
        decode(raw)
    }

    #[test]
    fn base_encodings() {
        // addi a0, a0, -1
        assert_eq!(d(0xfff5_0513), Insn::new(Op::Addi, 10, 10, 0, -1));
        // lui t0, 0x80000
        assert_eq!(d(0x8000_02b7), Insn::new(Op::Lui, 5, 0, 0, 0x8000_0000u32 as i32));
        // jal ra, -4  (0xffdff0ef)
        assert_eq!(d(0xffdf_f0ef), Insn::new(Op::Jal, 1, 0, 0, -4));
        // bne a0, a1, -8 (0xfeb51ce3)
        assert_eq!(d(0xfeb5_1ce3), Insn::new(Op::Bne, 0, 10, 11, -8));
        // sd ra, 8(sp) (0x00113423)
        assert_eq!(d(0x0011_3423), Insn::new(Op::Sd, 0, 2, 1, 8));
        // srai a0, a0, 63 (0x43f55513)
        assert_eq!(d(0x43f5_5513), Insn::new(Op::Srai, 10, 10, 0, 63));
        // csrrw zero, mtvec, t0 (0x30529073)
        assert_eq!(d(0x3052_9073), Insn::new(Op::Csrrw, 0, 5, 0, 0x305));
        // sfence.vma zero, zero
        assert_eq!(d(0x1200_0073).op, Op::SfenceVma);
        // amoswap.d.aqrl a0, a1, (a2)
        assert_eq!(d(0x0eb6_352f), Insn::new(Op::AmoswapD, 10, 12, 11, 0));
        assert_eq!(d(0x0000_0000).op, Op::Illegal);
        assert_eq!(d(0xffff_ffff).op, Op::Illegal);
    }

    #[test]
    fn compressed_encodings() {
        // c.addi4spn s0, sp, 16 (0x0800)
        assert_eq!(d(0x0800), Insn::new(Op::Addi, 8, 2, 0, 16));
        assert_eq!(d(0x0000).op, Op::Illegal);
        // c.li a0, -1 (0x557d)
        assert_eq!(d(0x557d), Insn::new(Op::Addi, 10, 0, 0, -1));
        // c.addi16sp sp, -64 (0x7139)
        assert_eq!(d(0x7139), Insn::new(Op::Addi, 2, 2, 0, -64));
        // c.lui a0, 0xfffff (0x757d) -> lui a0, -4096
        assert_eq!(d(0x757d), Insn::new(Op::Lui, 10, 0, 0, -4096));
        // c.j -2 (0xbffd)
        assert_eq!(d(0xbffd), Insn::new(Op::Jal, 0, 0, 0, -2));
        // c.beqz a0, +8 (0xc501)
        assert_eq!(d(0xc501), Insn::new(Op::Beq, 0, 10, 0, 8));
        // c.ldsp ra, 8(sp) (0x60a2)
        assert_eq!(d(0x60a2), Insn::new(Op::Ld, 1, 2, 0, 8));
        // c.sdsp ra, 8(sp) (0xe406)
        assert_eq!(d(0xe406), Insn::new(Op::Sd, 0, 2, 1, 8));
        // c.jr ra (0x8082)
        assert_eq!(d(0x8082), Insn::new(Op::Jalr, 0, 1, 0, 0));
        // c.mv a0, a1 (0x852e)
        assert_eq!(d(0x852e), Insn::new(Op::Add, 10, 0, 11, 0));
        // c.ebreak (0x9002)
        assert_eq!(d(0x9002).op, Op::Ebreak);
        // c.subw a0, a1 (0x9d0d)
        assert_eq!(d(0x9d0d), Insn::new(Op::Subw, 10, 10, 11, 0));
        // c.fld fa0, 8(a1) (0x2588) -> fld fa0, 8(a1) = 0x0085b507
        assert_eq!(d(0x2588).imm as u32, 0x0085_b507);
        // c.fsdsp fa0, 16(sp) (0xa82a) -> fsd fa0, 16(sp) = 0x00a13827
        assert_eq!(d(0xa82a).imm as u32, 0x00a1_3827);
    }
}
