use super::*;
use crate::cpu::ExceptionCause;
use crate::trace::RegWrite;

const RAM: u64 = 0x8000_0000;

const RNE: u32 = 0;
const RTZ: u32 = 1;
const DYN: u32 = 7;

fn machine() -> (Cpu, Bus) {
    let mut cpu = Cpu::new(RAM);
    cpu.set_mstatus_fs(FS_INITIAL);
    (cpu, Bus::new(RAM, 0x1_0000))
}

fn op_fp(funct5: u32, fmt: u32, rs2: u32, rs1: u32, rm: u32, rd: u32) -> u32 {
    (funct5 << 27) | (fmt << 25) | (rs2 << 20) | (rs1 << 15) | (rm << 12) | (rd << 7) | 0x53
}

fn fma(op: u32, fmt: u32, rs3: u32, rs2: u32, rs1: u32, rm: u32, rd: u32) -> u32 {
    (rs3 << 27) | (fmt << 25) | (rs2 << 20) | (rs1 << 15) | (rm << 12) | (rd << 7) | op
}

fn load(f3: u32, rd: u32, rs1: u32, imm: i32) -> u32 {
    ((imm as u32) << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | 0x07
}

fn store(f3: u32, rs2: u32, rs1: u32, imm: i32) -> u32 {
    let imm = imm as u32;
    ((imm >> 5) << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | ((imm & 31) << 7) | 0x27
}

fn fadd_d(rd: u32, rs1: u32, rs2: u32, rm: u32) -> u32 {
    op_fp(0x00, 1, rs2, rs1, rm, rd)
}

fn d(x: f64) -> u64 {
    x.to_bits()
}

fn boxed(x: f32) -> u64 {
    sf::nan_box_f32(x.to_bits())
}

fn is_illegal(r: Result<(), Exception>, insn: u32) -> bool {
    r == Err(Exception::new(ExceptionCause::IllegalInstruction, insn as u64))
}

#[test]
fn everything_is_illegal_while_fs_is_off() {
    let (mut cpu, mut bus) = machine();
    cpu.set_mstatus_fs(FS_OFF);
    cpu.x[1] = RAM;
    for insn in [
        load(3, 1, 1, 0),
        store(3, 1, 1, 0),
        fadd_d(1, 2, 3, RNE),
        fma(0x43, 1, 4, 3, 2, RNE, 1),
        op_fp(0x1c, 1, 0, 1, 0, 5), // fmv.x.d
        op_fp(0x1e, 1, 0, 1, 0, 5), // fmv.d.x
    ] {
        assert!(is_illegal(execute(&mut cpu, &mut bus, insn), insn), "{insn:08x}");
    }
    assert_eq!(cpu.x[5], 0);
    assert_eq!(cpu.fp.f, [0; 32]);
    assert_eq!(read_csr(&cpu, 0x003), Err(()));
    assert_eq!(write_csr(&mut cpu, 0x001, 1), Err(()));
    assert_eq!(cpu.mstatus_fs(), FS_OFF);
}

#[test]
fn register_writes_dirty_fs_and_set_sd() {
    let (mut cpu, mut bus) = machine();
    cpu.fp.f[2] = d(1.5);
    cpu.fp.f[3] = d(2.25);
    assert_eq!(cpu.csr.read_mstatus() >> 63, 0);
    execute(&mut cpu, &mut bus, fadd_d(1, 2, 3, RNE)).unwrap();
    assert_eq!(cpu.f(1), d(3.75));
    assert_eq!(cpu.fp.fflags, 0);
    assert_eq!(cpu.mstatus_fs(), FS_DIRTY);
    assert_eq!(cpu.csr.read_mstatus() >> 63, 1);
}

#[test]
fn integer_results_without_flags_leave_fs_clean() {
    let (mut cpu, mut bus) = machine();
    cpu.set_mstatus_fs(FS_CLEAN);
    cpu.fp.f[1] = d(-2.0);
    execute(&mut cpu, &mut bus, op_fp(0x1c, 1, 0, 1, 0, 5)).unwrap(); // fmv.x.d
    assert_eq!(cpu.x[5], d(-2.0));
    execute(&mut cpu, &mut bus, op_fp(0x1c, 1, 0, 1, 1, 6)).unwrap(); // fclass.d
    assert_eq!(cpu.x[6], 1 << 1);
    execute(&mut cpu, &mut bus, op_fp(0x18, 1, 0, 1, RNE, 7)).unwrap(); // fcvt.w.d
    assert_eq!(cpu.x[7] as i64, -2);
    cpu.x[8] = RAM;
    execute(&mut cpu, &mut bus, store(3, 1, 8, 16)).unwrap(); // fsd
    assert_eq!(cpu.mstatus_fs(), FS_CLEAN);
}

#[test]
fn raised_flags_accrue_and_dirty_fs() {
    let (mut cpu, mut bus) = machine();
    cpu.set_mstatus_fs(FS_CLEAN);
    cpu.fp.f[1] = sf::F64_CANONICAL_NAN;
    // flt with a quiet NaN raises NV, writes only an integer register, and dirties FS.
    execute(&mut cpu, &mut bus, op_fp(0x14, 1, 1, 1, 1, 5)).unwrap();
    assert_eq!(cpu.x[5], 0);
    assert_eq!(cpu.fp.fflags, Flags::NV.bits());
    assert_eq!(cpu.mstatus_fs(), FS_DIRTY);
    // feq with a quiet NaN raises nothing, and flags accumulate rather than replace.
    cpu.fp.f[2] = d(0.1);
    cpu.fp.f[3] = d(0.2);
    execute(&mut cpu, &mut bus, op_fp(0x14, 1, 1, 1, 2, 6)).unwrap();
    execute(&mut cpu, &mut bus, fadd_d(4, 2, 3, RNE)).unwrap();
    assert_eq!(cpu.fp.fflags, (Flags::NV | Flags::NX).bits());
}

#[test]
fn static_and_dynamic_rounding_modes() {
    let (mut cpu, mut bus) = machine();
    cpu.fp.f[2] = d(0.1);
    cpu.fp.f[3] = d(0.2);
    cpu.fp.frm = RTZ as u8;
    execute(&mut cpu, &mut bus, fadd_d(1, 2, 3, DYN)).unwrap();
    assert_eq!(cpu.f(1), 0x3fd3_3333_3333_3333);
    execute(&mut cpu, &mut bus, fadd_d(1, 2, 3, RNE)).unwrap();
    assert_eq!(cpu.f(1), 0x3fd3_3333_3333_3334);
    // fcvt.w.d 2.5 with RMM (static) and RNE (dynamic).
    cpu.fp.f[4] = d(2.5);
    cpu.fp.frm = RNE as u8;
    execute(&mut cpu, &mut bus, op_fp(0x18, 1, 0, 4, 4, 5)).unwrap();
    execute(&mut cpu, &mut bus, op_fp(0x18, 1, 0, 4, DYN, 6)).unwrap();
    assert_eq!((cpu.x[5], cpu.x[6]), (3, 2));
}

#[test]
fn reserved_rounding_modes_are_illegal() {
    let (mut cpu, mut bus) = machine();
    cpu.fp.f[2] = d(0.1);
    cpu.fp.f[3] = d(0.2);
    for rm in [5, 6] {
        let insn = fadd_d(1, 2, 3, rm);
        assert!(is_illegal(execute(&mut cpu, &mut bus, insn), insn));
    }
    for frm in [5, 6, 7] {
        cpu.fp.frm = frm;
        for insn in [
            fadd_d(1, 2, 3, DYN),
            op_fp(0x08, 1, 0, 2, DYN, 1),  // fcvt.d.s is exact but still checks rm
            op_fp(0x1a, 1, 0, 2, DYN, 1),  // fcvt.d.w
            fma(0x4f, 0, 3, 2, 2, DYN, 1), // fnmadd.s
            op_fp(0x0b, 1, 0, 2, DYN, 1),  // fsqrt.d
        ] {
            assert!(is_illegal(execute(&mut cpu, &mut bus, insn), insn), "{insn:08x}");
        }
    }
    // Instructions without an rm field ignore frm.
    cpu.fp.frm = 7;
    execute(&mut cpu, &mut bus, op_fp(0x04, 1, 3, 2, 0, 5)).unwrap(); // fsgnj.d
    assert_eq!(cpu.f(5), d(0.1));
    assert_eq!(cpu.f(1), 0);
    assert_eq!(cpu.fp.fflags, 0);
}

#[test]
fn invalid_encodings_are_illegal() {
    let (mut cpu, mut bus) = machine();
    for insn in [
        op_fp(0x00, 2, 3, 2, RNE, 1), // fadd.h
        op_fp(0x00, 3, 3, 2, RNE, 1), // fadd.q
        op_fp(0x0b, 1, 1, 2, RNE, 1), // fsqrt.d with rs2 != 0
        op_fp(0x04, 1, 3, 2, 3, 1),   // fsgnj funct3 = 3
        op_fp(0x05, 1, 3, 2, 2, 1),   // fmin/fmax funct3 = 2
        op_fp(0x14, 1, 3, 2, 3, 1),   // feq/flt/fle funct3 = 3
        op_fp(0x08, 0, 0, 2, RNE, 1), // fcvt.s.s
        op_fp(0x08, 1, 1, 2, RNE, 1), // fcvt.d.d
        op_fp(0x18, 1, 4, 2, RNE, 1), // fcvt with rs2 = 4
        op_fp(0x1c, 1, 0, 2, 2, 1),   // fmv.x.d funct3 = 2
        op_fp(0x1e, 1, 0, 2, 1, 1),   // fmv.d.x funct3 = 1
        op_fp(0x06, 1, 0, 2, 0, 1),   // unassigned funct5
        fma(0x43, 2, 4, 3, 2, RNE, 1),
        load(1, 1, 2, 0),  // flh
        store(4, 1, 2, 0), // fsq
    ] {
        assert!(is_illegal(execute(&mut cpu, &mut bus, insn), insn), "{insn:08x}");
    }
    assert_eq!(cpu.mstatus_fs(), FS_INITIAL);
}

#[test]
fn single_precision_values_are_nan_boxed() {
    let (mut cpu, mut bus) = machine();
    cpu.x[1] = RAM;
    bus.write(RAM + 8, 4, 0x4049_0fdb).unwrap();
    execute(&mut cpu, &mut bus, load(2, 3, 1, 8)).unwrap(); // flw f3, 8(x1)
    assert_eq!(cpu.f(3), 0xffff_ffff_4049_0fdb);
    // fadd.s on a properly boxed value.
    cpu.fp.f[4] = boxed(1.0);
    execute(&mut cpu, &mut bus, op_fp(0x00, 0, 4, 4, RNE, 5)).unwrap();
    assert_eq!(cpu.f(5), boxed(2.0));
    // An improperly boxed operand reads as the canonical NaN (a quiet NaN: no flags).
    cpu.fp.f[6] = d(1.0);
    execute(&mut cpu, &mut bus, op_fp(0x00, 0, 4, 6, RNE, 7)).unwrap();
    assert_eq!(cpu.f(7), sf::nan_box_f32(sf::F32_CANONICAL_NAN));
    assert_eq!(cpu.fp.fflags, 0);
    // fmv.x.w moves the raw low half, sign-extended, without unboxing.
    execute(&mut cpu, &mut bus, op_fp(0x1c, 0, 0, 6, 0, 8)).unwrap();
    assert_eq!(cpu.x[8], 0);
    cpu.fp.f[9] = 0x1234_5678_bf80_0000;
    execute(&mut cpu, &mut bus, op_fp(0x1c, 0, 0, 9, 0, 8)).unwrap();
    assert_eq!(cpu.x[8], 0xffff_ffff_bf80_0000);
    // fclass.s does unbox.
    execute(&mut cpu, &mut bus, op_fp(0x1c, 0, 0, 9, 1, 10)).unwrap();
    assert_eq!(cpu.x[10], 1 << 9);
    // fsw stores the raw low half.
    execute(&mut cpu, &mut bus, store(2, 9, 1, 16)).unwrap();
    assert_eq!(bus.read(RAM + 16, 8).unwrap(), 0xbf80_0000);
    // fmv.w.x boxes.
    cpu.x[11] = 0xdead_beef_3f80_0000;
    execute(&mut cpu, &mut bus, op_fp(0x1e, 0, 0, 11, 0, 12)).unwrap();
    assert_eq!(cpu.f(12), boxed(1.0));
}

#[test]
fn double_loads_and_stores() {
    let (mut cpu, mut bus) = machine();
    cpu.x[2] = RAM + 0x100;
    cpu.fp.f[7] = d(-1234.5);
    execute(&mut cpu, &mut bus, store(3, 7, 2, -8)).unwrap(); // fsd f7, -8(x2)
    assert_eq!(bus.read(RAM + 0xf8, 8).unwrap(), d(-1234.5));
    execute(&mut cpu, &mut bus, load(3, 8, 2, -8)).unwrap(); // fld f8, -8(x2)
    assert_eq!(cpu.f(8), d(-1234.5));
    // Misaligned addresses are supported.
    execute(&mut cpu, &mut bus, store(3, 7, 2, 3)).unwrap();
    execute(&mut cpu, &mut bus, load(3, 9, 2, 3)).unwrap();
    assert_eq!(cpu.f(9), d(-1234.5));
    // A faulting load writes nothing.
    cpu.x[2] = 0x1000;
    let r = execute(&mut cpu, &mut bus, load(3, 10, 2, 0));
    assert_eq!(r.map_err(|e| e.cause), Err(ExceptionCause::LoadAccessFault));
    assert_eq!(cpu.f(10), 0);
}

#[test]
fn conversions_sign_extend_32_bit_results() {
    let (mut cpu, mut bus) = machine();
    cpu.fp.f[1] = d(3e9);
    execute(&mut cpu, &mut bus, op_fp(0x18, 1, 1, 1, RTZ, 5)).unwrap(); // fcvt.wu.d
    assert_eq!(cpu.x[5], 3_000_000_000u32 as i32 as i64 as u64);
    execute(&mut cpu, &mut bus, op_fp(0x18, 1, 0, 1, RTZ, 6)).unwrap(); // fcvt.w.d saturates
    assert_eq!(cpu.x[6], i32::MAX as u64);
    assert_eq!(cpu.fp.fflags, Flags::NV.bits());
    cpu.x[7] = 0xffff_ffff_ffff_fffe;
    execute(&mut cpu, &mut bus, op_fp(0x1a, 1, 1, 7, RNE, 2)).unwrap(); // fcvt.d.wu
    assert_eq!(cpu.f(2), d(4294967294.0));
    execute(&mut cpu, &mut bus, op_fp(0x1a, 1, 0, 7, RNE, 3)).unwrap(); // fcvt.d.w
    assert_eq!(cpu.f(3), d(-2.0));
    execute(&mut cpu, &mut bus, op_fp(0x1a, 0, 3, 7, RNE, 4)).unwrap(); // fcvt.s.lu
    assert_eq!(cpu.f(4), boxed(1.8446744e19));
}

#[test]
fn fused_multiply_add_opcodes() {
    let (mut cpu, mut bus) = machine();
    cpu.fp.f[1] = d(2.0);
    cpu.fp.f[2] = d(3.0);
    cpu.fp.f[3] = d(1.0);
    let cases = [(0x43, 7.0), (0x47, 5.0), (0x4b, -5.0), (0x4f, -7.0)];
    for (op, want) in cases {
        execute(&mut cpu, &mut bus, fma(op, 1, 3, 2, 1, RNE, 4)).unwrap();
        assert_eq!(cpu.f(4), d(want), "opcode {op:#x}");
    }
    cpu.fp.f[5] = boxed(2.0);
    cpu.fp.f[6] = boxed(3.0);
    cpu.fp.f[7] = boxed(1.0);
    execute(&mut cpu, &mut bus, fma(0x47, 0, 7, 6, 5, RNE, 8)).unwrap();
    assert_eq!(cpu.f(8), boxed(5.0));
}

#[test]
fn fcsr_fields() {
    let (mut cpu, _) = machine();
    write_csr(&mut cpu, 0x003, 0xff).unwrap();
    assert_eq!((cpu.fp.frm, cpu.fp.fflags), (7, 0x1f));
    assert_eq!(read_csr(&cpu, 0x003), Ok(0xff));
    write_csr(&mut cpu, 0x002, 0x2).unwrap();
    assert_eq!(read_csr(&cpu, 0x003), Ok(0x5f));
    write_csr(&mut cpu, 0x001, 0x21).unwrap();
    assert_eq!(read_csr(&cpu, 0x001), Ok(0x01));
    assert_eq!(cpu.mstatus_fs(), FS_DIRTY);
}

struct NullSink;

impl crate::trace::TraceSink for NullSink {
    fn commit(&mut self, _commit: &crate::trace::Commit) {}
}

/// A machine with tracing enabled, so register and CSR writes are recorded in
/// \`cpu.commit\`.
fn traced_machine() -> (Cpu, Bus) {
    let (mut cpu, bus) = machine();
    cpu.set_tracer(Some(Box::new(NullSink)));
    (cpu, bus)
}

fn logged(cpu: &mut Cpu, f: impl FnOnce(&mut Cpu)) -> Vec<RegWrite> {
    cpu.commit.clear();
    f(cpu);
    let mut regs = cpu.commit.regs.clone();
    regs.sort_by_key(|r| format!("{r:?}"));
    regs
}

fn sorted(mut v: Vec<RegWrite>) -> Vec<RegWrite> {
    v.sort_by_key(|r| format!("{r:?}"));
    v
}

const DIRTY_MSTATUS: u64 = (3 << 13) | (1 << 63);

#[test]
fn commit_log_matches_spike_rules() {
    let (mut cpu, mut bus) = traced_machine();
    let mstatus = |cpu: &Cpu| cpu.csr.read_mstatus();
    cpu.fp.f[2] = d(0.1);
    cpu.fp.f[3] = d(0.2);
    // First write while FS is not Dirty: the f register, fflags (a flag was raised), and
    // mstatus with FS = 3 and SD = 1.
    let regs = logged(&mut cpu, |cpu| execute(cpu, &mut bus, fadd_d(1, 2, 3, RNE)).unwrap());
    assert_eq!(mstatus(&cpu) & DIRTY_MSTATUS, DIRTY_MSTATUS);
    assert_eq!(
        regs,
        sorted(vec![
            RegWrite::F(1, 0x3fd3_3333_3333_3334),
            RegWrite::Csr(csr::FFLAGS, 1),
            RegWrite::Csr(csr::MSTATUS, mstatus(&cpu)),
        ])
    );
    // Already Dirty and no flags raised: only the f register, NaN-boxed and 64 bits wide.
    cpu.fp.f[4] = boxed(1.0);
    let regs = logged(&mut cpu, |cpu| execute(cpu, &mut bus, op_fp(0x00, 0, 4, 4, RNE, 5)).unwrap());
    assert_eq!(regs, vec![RegWrite::F(5, 0xffff_ffff_4000_0000)]);
    // A raised flag that is already set still logs fflags with the accrued value.
    let regs = logged(&mut cpu, |cpu| execute(cpu, &mut bus, fadd_d(1, 2, 3, RNE)).unwrap());
    assert_eq!(regs, sorted(vec![RegWrite::F(1, 0x3fd3_3333_3333_3334), RegWrite::Csr(csr::FFLAGS, 1)]));
    cpu.fp.f[6] = sf::F64_CANONICAL_NAN;
    let regs = logged(&mut cpu, |cpu| execute(cpu, &mut bus, op_fp(0x14, 1, 6, 6, 1, 7)).unwrap());
    assert_eq!(regs, sorted(vec![RegWrite::X(7, 0), RegWrite::Csr(csr::FFLAGS, 0x11)]));
}

#[test]
fn fp_csr_writes_log_fflags_and_frm_not_fcsr() {
    let (mut cpu, mut bus) = traced_machine();
    let _ = &mut bus;
    let regs = logged(&mut cpu, |cpu| write_csr(cpu, csr::FCSR, 0x1ff).unwrap());
    let ms = cpu.csr.read_mstatus();
    assert_eq!(
        regs,
        sorted(vec![RegWrite::Csr(csr::FFLAGS, 0x1f), RegWrite::Csr(csr::FRM, 0x7), RegWrite::Csr(csr::MSTATUS, ms)])
    );
    let regs = logged(&mut cpu, |cpu| write_csr(cpu, csr::FRM, 0xfa).unwrap());
    assert_eq!(regs, vec![RegWrite::Csr(csr::FRM, 2)]);
    let regs = logged(&mut cpu, |cpu| write_csr(cpu, csr::FFLAGS, 0x23).unwrap());
    assert_eq!(regs, vec![RegWrite::Csr(csr::FFLAGS, 3)]);
}
