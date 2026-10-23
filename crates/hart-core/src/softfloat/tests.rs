use super::*;

const RNE: RoundingMode = RoundingMode::NearestEven;
const RTZ: RoundingMode = RoundingMode::TowardZero;
const RDN: RoundingMode = RoundingMode::Down;
const RUP: RoundingMode = RoundingMode::Up;
const RMM: RoundingMode = RoundingMode::NearestMaxMag;
const ALL_RM: [RoundingMode; 5] = [RNE, RTZ, RDN, RUP, RMM];

const F64_SNAN: u64 = 0x7ff0_0000_0000_0001;
const F64_QNAN_PAYLOAD: u64 = 0xfff8_dead_beef_0001;
const F32_SNAN: u32 = 0x7f80_0001;
const F64_MAX: u64 = 0x7fef_ffff_ffff_ffff;
const F64_INF: u64 = 0x7ff0_0000_0000_0000;
const F64_MIN_NORMAL: u64 = 0x0010_0000_0000_0000;

fn d(x: f64) -> u64 {
    x.to_bits()
}

fn s(x: f32) -> u32 {
    x.to_bits()
}

/// Runs `f` with fresh flags and returns its result together with the flags.
fn run<T>(f: impl FnOnce(&mut Flags) -> T) -> (T, Flags) {
    let mut fl = Flags::empty();
    let r = f(&mut fl);
    (r, fl)
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// A random binary64 pattern biased toward special and boundary values.
    fn f64(&mut self) -> u64 {
        let r = self.next();
        let sign = r & (1 << 63);
        match r % 8 {
            0 => sign | (r >> 8) & 0x000f_ffff_ffff_ffff,
            1 => sign | ((r >> 3) % 3 + 0x7fd) << 52 | (self.next() & 0x000f_ffff_ffff_ffff),
            2 => sign | ((r >> 3) % 64 + 1) << 52 | (self.next() & 0x000f_ffff_ffff_ffff),
            3 => sign | 0x3ff0_0000_0000_0000 | (self.next() & 0xff),
            _ => self.next() & !(0x7ffu64 << 52) | (((r >> 4) % 2047) << 52) | sign,
        }
    }

    fn f32(&mut self) -> u32 {
        let r = self.next();
        let sign = (r as u32) & 0x8000_0000;
        let frac = (self.next() as u32) & 0x007f_ffff;
        match r % 8 {
            0 => sign | frac,
            1 => sign | (((r >> 3) % 3 + 0xfd) as u32) << 23 | frac,
            2 => sign | (((r >> 3) % 32 + 1) as u32) << 23 | frac,
            _ => sign | (((r >> 4) % 255) as u32) << 23 | frac,
        }
    }
}

/// The host computes binary64 arithmetic in round-to-nearest-even with
/// gradual underflow, so it is an independent oracle for RNE results.
#[test]
fn f64_matches_host_in_rne() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    for _ in 0..200_000 {
        let (a, b, c) = (rng.f64(), rng.f64(), rng.f64());
        let (fa, fb, fc) = (f64::from_bits(a), f64::from_bits(b), f64::from_bits(c));
        let check = |name: &str, got: u64, want: f64| {
            if want.is_nan() {
                assert_eq!(got, F64_CANONICAL_NAN, "{name} {a:016x} {b:016x} {c:016x}");
            } else {
                assert_eq!(got, want.to_bits(), "{name} {a:016x} {b:016x} {c:016x}");
            }
        };
        let mut fl = Flags::empty();
        check("add", f64_add(a, b, RNE, &mut fl), fa + fb);
        check("sub", f64_sub(a, b, RNE, &mut fl), fa - fb);
        check("mul", f64_mul(a, b, RNE, &mut fl), fa * fb);
        check("div", f64_div(a, b, RNE, &mut fl), fa / fb);
        check("sqrt", f64_sqrt(a, RNE, &mut fl), fa.sqrt());
        check("fma", f64_mul_add(a, b, c, RNE, &mut fl), fa.mul_add(fb, fc));
        let narrow = f64_to_f32(a, RNE, &mut fl);
        if fa.is_nan() {
            assert_eq!(narrow, F32_CANONICAL_NAN);
        } else {
            assert_eq!(narrow, (fa as f32).to_bits(), "f64_to_f32 {a:016x}");
        }
    }
}

#[test]
fn f32_matches_host_in_rne() {
    let mut rng = Rng(0x0fed_cba9_8765_4321);
    for _ in 0..200_000 {
        let (a, b, c) = (rng.f32(), rng.f32(), rng.f32());
        let (fa, fb, fc) = (f32::from_bits(a), f32::from_bits(b), f32::from_bits(c));
        let check = |name: &str, got: u32, want: f32| {
            if want.is_nan() {
                assert_eq!(got, F32_CANONICAL_NAN, "{name} {a:08x} {b:08x} {c:08x}");
            } else {
                assert_eq!(got, want.to_bits(), "{name} {a:08x} {b:08x} {c:08x}");
            }
        };
        let mut fl = Flags::empty();
        check("add", f32_add(a, b, RNE, &mut fl), fa + fb);
        check("sub", f32_sub(a, b, RNE, &mut fl), fa - fb);
        check("mul", f32_mul(a, b, RNE, &mut fl), fa * fb);
        check("div", f32_div(a, b, RNE, &mut fl), fa / fb);
        check("sqrt", f32_sqrt(a, RNE, &mut fl), fa.sqrt());
        check("fma", f32_mul_add(a, b, c, RNE, &mut fl), fa.mul_add(fb, fc));
        let wide = f32_to_f64(a, &mut fl);
        if fa.is_nan() {
            assert_eq!(wide, F64_CANONICAL_NAN);
        } else {
            assert_eq!(wide, (fa as f64).to_bits());
        }
    }
}

#[test]
fn inexact_flag() {
    assert_eq!(run(|fl| f64_add(d(0.1), d(0.2), RNE, fl)), (0x3fd3_3333_3333_3334, Flags::NX));
    assert_eq!(run(|fl| f64_add(d(1.0), d(2.0), RNE, fl)), (d(3.0), Flags::empty()));
    assert_eq!(run(|fl| f64_add(d(0.1), d(0.2), RTZ, fl)), (0x3fd3_3333_3333_3333, Flags::NX));
}

#[test]
fn overflow_depends_on_rounding_direction() {
    let neg = |x: u64| x | 1 << 63;
    let of = Flags::OF | Flags::NX;
    let cases = [
        (RNE, F64_INF, neg(F64_INF)),
        (RTZ, F64_MAX, neg(F64_MAX)),
        (RDN, F64_MAX, neg(F64_INF)),
        (RUP, F64_INF, neg(F64_MAX)),
        (RMM, F64_INF, neg(F64_INF)),
    ];
    for (rm, pos, negative) in cases {
        assert_eq!(run(|fl| f64_mul(F64_MAX, d(2.0), rm, fl)), (pos, of), "{rm:?}");
        assert_eq!(run(|fl| f64_mul(neg(F64_MAX), d(2.0), rm, fl)), (negative, of), "{rm:?}");
    }
    // Overflow caused only by rounding up a value just below the threshold.
    let (r, fl) = run(|fl| f64_add(F64_MAX, d(2f64.powi(970)), RNE, fl));
    assert_eq!((r, fl), (F64_INF, of));
    let (r, fl) = run(|fl| f64_add(F64_MAX, d(2f64.powi(969)), RNE, fl));
    assert_eq!((r, fl), (F64_MAX, Flags::NX));
}

#[test]
fn tininess_is_detected_after_rounding() {
    let c = F64_MIN_NORMAL;
    // 2^-1022 - 2^-1077 rounds to 2^-1022 even with an unbounded exponent, so
    // it is not tiny: only NX is raised.
    let (a, b) = (d(2f64.powi(-540)), d(-(2f64.powi(-537))));
    assert_eq!(run(|fl| f64_mul_add(a, b, c, RNE, fl)), (F64_MIN_NORMAL, Flags::NX));
    // 2^-1022 - 2^-1075 is exactly representable with an unbounded exponent,
    // so it is tiny, and the subnormal result is inexact: UF and NX.
    let (a, b) = (d(2f64.powi(-538)), d(-(2f64.powi(-537))));
    assert_eq!(run(|fl| f64_mul_add(a, b, c, RNE, fl)), (F64_MIN_NORMAL, Flags::UF | Flags::NX));
    // An exact subnormal result raises no flags.
    assert_eq!(
        run(|fl| f64_mul(d(2f64.powi(-1000)), d(2f64.powi(-60)), RNE, fl)),
        (d(2f64.powi(-1060)), Flags::empty())
    );
    // Rounding to zero still raises UF and NX.
    let (r, fl) = run(|fl| f64_mul(d(2f64.powi(-1000)), d(2f64.powi(-100)), RTZ, fl));
    assert_eq!((r, fl), (0, Flags::UF | Flags::NX));
    let (r, fl) = run(|fl| f64_mul(d(2f64.powi(-1000)), d(2f64.powi(-100)), RUP, fl));
    assert_eq!((r, fl), (1, Flags::UF | Flags::NX));
}

#[test]
fn nan_results_are_canonical() {
    let nv = Flags::NV;
    assert_eq!(run(|fl| f64_sub(F64_INF, F64_INF, RNE, fl)), (F64_CANONICAL_NAN, nv));
    assert_eq!(run(|fl| f64_add(F64_QNAN_PAYLOAD, d(1.0), RNE, fl)), (F64_CANONICAL_NAN, Flags::empty()));
    assert_eq!(run(|fl| f64_add(F64_SNAN, d(1.0), RNE, fl)), (F64_CANONICAL_NAN, nv));
    assert_eq!(run(|fl| f64_mul(0, F64_INF, RNE, fl)), (F64_CANONICAL_NAN, nv));
    assert_eq!(run(|fl| f64_div(0, 0, RNE, fl)), (F64_CANONICAL_NAN, nv));
    assert_eq!(run(|fl| f64_div(F64_INF, F64_INF, RNE, fl)), (F64_CANONICAL_NAN, nv));
    assert_eq!(run(|fl| f64_sqrt(d(-1.0), RNE, fl)), (F64_CANONICAL_NAN, nv));
    assert_eq!(run(|fl| f32_mul(F32_SNAN, s(2.0), RNE, fl)), (F32_CANONICAL_NAN, nv));
    assert_eq!(run(|fl| f32_to_f64(F32_SNAN, fl)), (F64_CANONICAL_NAN, nv));
    assert_eq!(run(|fl| f64_to_f32(F64_QNAN_PAYLOAD, RNE, fl)), (F32_CANONICAL_NAN, Flags::empty()));
}

#[test]
fn divide_by_zero_and_signed_zeros() {
    assert_eq!(run(|fl| f64_div(d(1.0), d(-0.0), RNE, fl)), (d(f64::NEG_INFINITY), Flags::DZ));
    assert_eq!(run(|fl| f64_sqrt(d(-0.0), RNE, fl)), (d(-0.0), Flags::empty()));
    assert_eq!(run(|fl| f64_add(d(1.0), d(-1.0), RNE, fl)), (d(0.0), Flags::empty()));
    assert_eq!(run(|fl| f64_add(d(1.0), d(-1.0), RDN, fl)), (d(-0.0), Flags::empty()));
    assert_eq!(run(|fl| f64_add(d(-0.0), d(0.0), RDN, fl)), (d(-0.0), Flags::empty()));
    assert_eq!(run(|fl| f64_add(d(-0.0), d(-0.0), RNE, fl)), (d(-0.0), Flags::empty()));
}

#[test]
fn fused_multiply_add_variants() {
    let (two, three, one) = (d(2.0), d(3.0), d(1.0));
    assert_eq!(run(|fl| f64_fmadd(two, three, one, RNE, fl)).0, d(7.0));
    assert_eq!(run(|fl| f64_fmsub(two, three, one, RNE, fl)).0, d(5.0));
    assert_eq!(run(|fl| f64_fnmsub(two, three, one, RNE, fl)).0, d(-5.0));
    assert_eq!(run(|fl| f64_fnmadd(two, three, one, RNE, fl)).0, d(-7.0));
    assert_eq!(run(|fl| f32_fnmadd(s(2.0), s(3.0), s(1.0), RNE, fl)).0, s(-7.0));
    // -(0 * 1) - 0 is -0.
    assert_eq!(run(|fl| f64_fnmadd(0, one, 0, RNE, fl)), (d(-0.0), Flags::empty()));
    // inf * 0 is invalid even with a quiet NaN addend.
    assert_eq!(run(|fl| f64_fmadd(F64_INF, 0, F64_CANONICAL_NAN, RNE, fl)), (F64_CANONICAL_NAN, Flags::NV));
    assert_eq!(run(|fl| f64_fmadd(F64_CANONICAL_NAN, 0, one, RNE, fl)), (F64_CANONICAL_NAN, Flags::empty()));
    // The product is not rounded before the addition.
    let a = d(1.0 + f64::EPSILON);
    let want = (1.0 + f64::EPSILON).mul_add(1.0 - f64::EPSILON, -1.0);
    assert_eq!(run(|fl| f64_fmadd(a, d(1.0 - f64::EPSILON), d(-1.0), RNE, fl)).0, d(want));
    assert_ne!(want, 0.0);
}

#[test]
fn comparisons_raise_nv_per_riscv_rules() {
    let q = F64_CANONICAL_NAN;
    assert_eq!(run(|fl| f64_eq(q, d(1.0), fl)), (false, Flags::empty()));
    assert_eq!(run(|fl| f64_eq(F64_SNAN, d(1.0), fl)), (false, Flags::NV));
    assert_eq!(run(|fl| f64_lt(q, d(1.0), fl)), (false, Flags::NV));
    assert_eq!(run(|fl| f64_le(d(1.0), q, fl)), (false, Flags::NV));
    assert_eq!(run(|fl| f64_eq(d(0.0), d(-0.0), fl)), (true, Flags::empty()));
    assert_eq!(run(|fl| f64_lt(d(-0.0), d(0.0), fl)), (false, Flags::empty()));
    assert_eq!(run(|fl| f64_le(d(0.0), d(-0.0), fl)), (true, Flags::empty()));
    assert!(run(|fl| f64_lt(d(-2.0), d(-1.0), fl)).0);
    assert!(!run(|fl| f64_lt(d(-1.0), d(-2.0), fl)).0);
    assert!(run(|fl| f32_le(s(-3.0), s(2.0), fl)).0);
    assert!(!run(|fl| f32_lt(s(2.0), s(2.0), fl)).0);
}

#[test]
fn min_max_follow_riscv_semantics() {
    let q = F64_CANONICAL_NAN;
    assert_eq!(run(|fl| f64_min(d(-0.0), d(0.0), fl)), (d(-0.0), Flags::empty()));
    assert_eq!(run(|fl| f64_min(d(0.0), d(-0.0), fl)), (d(-0.0), Flags::empty()));
    assert_eq!(run(|fl| f64_max(d(-0.0), d(0.0), fl)), (d(0.0), Flags::empty()));
    assert_eq!(run(|fl| f64_max(q, d(1.0), fl)), (d(1.0), Flags::empty()));
    assert_eq!(run(|fl| f64_min(d(1.0), F64_SNAN, fl)), (d(1.0), Flags::NV));
    assert_eq!(run(|fl| f64_min(F64_QNAN_PAYLOAD, F64_SNAN, fl)), (q, Flags::NV));
    assert_eq!(run(|fl| f64_max(F64_QNAN_PAYLOAD, F64_QNAN_PAYLOAD, fl)), (q, Flags::empty()));
    assert_eq!(run(|fl| f64_min(d(-5.0), d(-7.0), fl)).0, d(-7.0));
    assert_eq!(run(|fl| f64_max(d(-5.0), d(-7.0), fl)).0, d(-5.0));
    assert_eq!(run(|fl| f32_max(s(3.0), s(-7.0), fl)).0, s(3.0));
    assert_eq!(run(|fl| f32_min(s(3.0), s(-7.0), fl)).0, s(-7.0));
}

#[test]
fn classify_covers_every_class() {
    let cases: [(u64, u64); 10] = [
        (d(f64::NEG_INFINITY), 1 << 0),
        (d(-1.5), 1 << 1),
        (0x800f_ffff_ffff_ffff, 1 << 2),
        (d(-0.0), 1 << 3),
        (d(0.0), 1 << 4),
        (0x0000_0000_0000_0001, 1 << 5),
        (d(1.5), 1 << 6),
        (F64_INF, 1 << 7),
        (F64_SNAN, 1 << 8),
        (F64_QNAN_PAYLOAD, 1 << 9),
    ];
    for (x, want) in cases {
        assert_eq!(f64_classify(x), want, "{x:016x}");
    }
    assert_eq!(f32_classify(F32_SNAN), 1 << 8);
    assert_eq!(f32_classify(F32_CANONICAL_NAN), 1 << 9);
    assert_eq!(f32_classify(0x0040_0000), 1 << 5);
}

#[test]
fn sign_injection() {
    assert_eq!(f64_sgnj(d(1.0), d(-2.0)), d(-1.0));
    assert_eq!(f64_sgnjn(d(1.0), d(-2.0)), d(1.0));
    assert_eq!(f64_sgnjx(d(-1.0), d(-2.0)), d(1.0));
    assert_eq!(f32_sgnj(s(-1.0), s(2.0)), s(1.0));
    assert_eq!(f32_sgnjn(s(1.0), s(2.0)), s(-1.0));
    assert_eq!(f32_sgnjx(s(1.0), s(-2.0)), s(-1.0));
}

#[test]
fn float_to_int_saturation_and_flags() {
    let nv = Flags::NV;
    assert_eq!(run(|fl| f64_to_i32(F64_CANONICAL_NAN, RNE, fl)), (i32::MAX, nv));
    assert_eq!(run(|fl| f64_to_u32(F64_CANONICAL_NAN, RNE, fl)), (u32::MAX, nv));
    assert_eq!(run(|fl| f64_to_i64(d(f64::NEG_INFINITY), RNE, fl)), (i64::MIN, nv));
    assert_eq!(run(|fl| f64_to_u64(d(f64::NEG_INFINITY), RNE, fl)), (0, nv));
    assert_eq!(run(|fl| f64_to_u32(d(-1.0), RNE, fl)), (0, nv));
    assert_eq!(run(|fl| f64_to_u32(d(-0.5), RTZ, fl)), (0, Flags::NX));
    assert_eq!(run(|fl| f64_to_u32(d(-0.5), RDN, fl)), (0, nv));
    assert_eq!(run(|fl| f64_to_i32(d(2.5), RNE, fl)), (2, Flags::NX));
    assert_eq!(run(|fl| f64_to_i32(d(2.5), RMM, fl)), (3, Flags::NX));
    assert_eq!(run(|fl| f64_to_i32(d(-2.5), RDN, fl)), (-3, Flags::NX));
    assert_eq!(run(|fl| f64_to_i32(d(-2.5), RUP, fl)), (-2, Flags::NX));
    assert_eq!(run(|fl| f64_to_i32(d(2147483647.4), RNE, fl)), (i32::MAX, Flags::NX));
    assert_eq!(run(|fl| f64_to_i32(d(2147483647.5), RNE, fl)), (i32::MAX, nv));
    assert_eq!(run(|fl| f64_to_i32(d(-2147483648.9), RTZ, fl)), (i32::MIN, Flags::NX));
    assert_eq!(run(|fl| f64_to_i64(d(2f64.powi(63)), RNE, fl)), (i64::MAX, nv));
    assert_eq!(run(|fl| f64_to_i64(d(-(2f64.powi(63))), RNE, fl)), (i64::MIN, Flags::empty()));
    assert_eq!(run(|fl| f64_to_u64(0x43ef_ffff_ffff_ffff, RNE, fl)), (18446744073709549568, Flags::empty()));
    assert_eq!(run(|fl| f32_to_u64(s(1.8446744e19), RNE, fl)), (u64::MAX, nv));
    assert_eq!(run(|fl| f32_to_i32(s(-0.0), RNE, fl)), (0, Flags::empty()));
    assert_eq!(run(|fl| f32_to_i64(0x0000_0001, RUP, fl)), (1, Flags::NX));
}

#[test]
fn int_to_float_rounding() {
    assert_eq!(run(|fl| i64_to_f32(i64::MIN, RNE, fl)), (s(-9.223372e18), Flags::empty()));
    assert_eq!(run(|fl| u64_to_f64(u64::MAX, RNE, fl)), (d(1.8446744073709552e19), Flags::NX));
    assert_eq!(run(|fl| u64_to_f64(u64::MAX, RTZ, fl)), (0x43ef_ffff_ffff_ffff, Flags::NX));
    assert_eq!(run(|fl| i32_to_f64(i32::MIN, RNE, fl)), (d(-2147483648.0), Flags::empty()));
    assert_eq!(run(|fl| u32_to_f32(16_777_217, RNE, fl)), (s(16_777_216.0), Flags::NX));
    assert_eq!(run(|fl| u32_to_f32(16_777_217, RUP, fl)), (s(16_777_218.0), Flags::NX));
    assert_eq!(run(|fl| i32_to_f32(0, RNE, fl)), (0, Flags::empty()));
    for rm in ALL_RM {
        assert_eq!(run(|fl| i64_to_f64(-3, rm, fl)), (d(-3.0), Flags::empty()));
    }
}

#[test]
fn narrowing_conversion() {
    assert_eq!(run(|fl| f64_to_f32(d(1e300), RNE, fl)), (s(f32::INFINITY), Flags::OF | Flags::NX));
    assert_eq!(run(|fl| f64_to_f32(d(1e300), RTZ, fl)), (s(f32::MAX), Flags::OF | Flags::NX));
    assert_eq!(run(|fl| f64_to_f32(d(1e-300), RNE, fl)), (0, Flags::UF | Flags::NX));
    assert_eq!(run(|fl| f64_to_f32(d(1e-300), RUP, fl)), (1, Flags::UF | Flags::NX));
    assert_eq!(run(|fl| f64_to_f32(d(0.1), RNE, fl)), (s(0.1), Flags::NX));
    assert_eq!(run(|fl| f64_to_f32(d(-0.0), RNE, fl)), (s(-0.0), Flags::empty()));
    assert_eq!(run(|fl| f32_to_f64(0x0000_0001, fl)), (d(f32::from_bits(1) as f64), Flags::empty()));
}

#[test]
fn nan_boxing() {
    assert_eq!(nan_box_f32(s(1.0)), 0xffff_ffff_3f80_0000);
    assert_eq!(unbox_f32(0xffff_ffff_3f80_0000), s(1.0));
    assert_eq!(unbox_f32(0xffff_fffe_3f80_0000), F32_CANONICAL_NAN);
    assert_eq!(unbox_f32(0x0000_0000_3f80_0000), F32_CANONICAL_NAN);
}

#[test]
fn rounding_mode_decoding() {
    for (bits, rm) in ALL_RM.iter().enumerate() {
        assert_eq!(RoundingMode::from_bits(bits as u32), Some(*rm));
        assert_eq!(rm.bits(), bits as u32);
    }
    for bits in 5..8 {
        assert_eq!(RoundingMode::from_bits(bits), None);
    }
}
