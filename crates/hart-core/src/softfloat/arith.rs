//! Addition, multiplication, division, square root, and fused multiply-add.

use super::pack::{round_pack, round_pack128, shr_jam128, shr_jam64, Fmt};
use super::{Flags, RoundingMode};

/// The canonical NaN, raising NV if any listed operand is a signaling NaN.
#[inline]
fn nan_result(fmt: Fmt, operands: &[u64], fl: &mut Flags) -> u64 {
    if operands.iter().any(|&x| fmt.is_snan(x)) {
        *fl |= Flags::NV;
    }
    fmt.canonical_nan()
}

/// The canonical NaN for an invalid operation.
#[inline]
fn invalid(fmt: Fmt, fl: &mut Flags) -> u64 {
    *fl |= Flags::NV;
    fmt.canonical_nan()
}

/// The sign of an exact zero sum of operands with opposite signs.
#[inline]
fn exact_zero_sum(fmt: Fmt, rm: RoundingMode) -> u64 {
    fmt.signed_zero(rm == RoundingMode::Down)
}

pub(crate) fn add(fmt: Fmt, a: u64, b: u64, rm: RoundingMode, fl: &mut Flags) -> u64 {
    if fmt.is_nan(a) || fmt.is_nan(b) {
        return nan_result(fmt, &[a, b], fl);
    }
    let (sa, sb) = (fmt.sign(a), fmt.sign(b));
    let (a_inf, b_inf) = (fmt.is_inf(a), fmt.is_inf(b));
    if a_inf || b_inf {
        if a_inf && b_inf && sa != sb {
            return invalid(fmt, fl);
        }
        return if a_inf { a } else { b };
    }
    let (a_zero, b_zero) = (fmt.is_zero(a), fmt.is_zero(b));
    if a_zero || b_zero {
        if a_zero && b_zero {
            return if sa == sb { a } else { exact_zero_sum(fmt, rm) };
        }
        return if a_zero { b } else { a };
    }

    // Order the operands by magnitude.
    let (big, small) = if a & fmt.abs_mask() >= b & fmt.abs_mask() { (a, b) } else { (b, a) };
    let sign = fmt.sign(big);
    let (e_big, m_big) = fmt.unpack(big);
    let (e_small, m_small) = fmt.unpack(small);
    // Leave two bits of headroom above a normal significand for the carry and
    // the normalization in `round_pack`.
    let g = 61 - fmt.frac_bits;
    let x = m_big << g;
    let y = shr_jam64(m_small << g, (e_big - e_small) as u32);
    let sum = if fmt.sign(small) == sign { x + y } else { x - y };
    if sum == 0 {
        return exact_zero_sum(fmt, rm);
    }
    round_pack(fmt, sign, e_big - g as i32, sum, rm, fl)
}

pub(crate) fn mul(fmt: Fmt, a: u64, b: u64, rm: RoundingMode, fl: &mut Flags) -> u64 {
    if fmt.is_nan(a) || fmt.is_nan(b) {
        return nan_result(fmt, &[a, b], fl);
    }
    let sign = fmt.sign(a) != fmt.sign(b);
    let (a_zero, b_zero) = (fmt.is_zero(a), fmt.is_zero(b));
    if fmt.is_inf(a) || fmt.is_inf(b) {
        if a_zero || b_zero {
            return invalid(fmt, fl);
        }
        return fmt.signed_inf(sign);
    }
    if a_zero || b_zero {
        return fmt.signed_zero(sign);
    }
    let (ea, ma) = fmt.unpack(a);
    let (eb, mb) = fmt.unpack(b);
    round_pack128(fmt, sign, ea + eb, ma as u128 * mb as u128, rm, fl)
}

pub(crate) fn div(fmt: Fmt, a: u64, b: u64, rm: RoundingMode, fl: &mut Flags) -> u64 {
    if fmt.is_nan(a) || fmt.is_nan(b) {
        return nan_result(fmt, &[a, b], fl);
    }
    let sign = fmt.sign(a) != fmt.sign(b);
    let (a_inf, b_inf) = (fmt.is_inf(a), fmt.is_inf(b));
    if a_inf {
        if b_inf {
            return invalid(fmt, fl);
        }
        return fmt.signed_inf(sign);
    }
    if b_inf {
        return fmt.signed_zero(sign);
    }
    let (a_zero, b_zero) = (fmt.is_zero(a), fmt.is_zero(b));
    if b_zero {
        if a_zero {
            return invalid(fmt, fl);
        }
        *fl |= Flags::DZ;
        return fmt.signed_inf(sign);
    }
    if a_zero {
        return fmt.signed_zero(sign);
    }
    // Both significands have their leading one at bit `frac_bits`, so the
    // quotient of (ma << 62) / mb lies in [2^61, 2^63).
    let (ea, ma) = fmt.unpack_norm(a);
    let (eb, mb) = fmt.unpack_norm(b);
    let q = if fmt.frac_bits <= 24 {
        let n = ma << 38;
        let q = n / mb;
        (q << 24) | (n % mb != 0) as u64
    } else {
        let n = (ma as u128) << 62;
        let mb = mb as u128;
        let q = (n / mb) as u64;
        q | (n % mb != 0) as u64
    };
    round_pack(fmt, sign, ea - eb - 62, q, rm, fl)
}

/// Floor of the square root of `n`, and whether the remainder is nonzero.
fn isqrt128(n: u128) -> (u64, bool) {
    let mut rem = n;
    let mut root: u128 = 0;
    let mut bit: u128 = 1 << (126 - (n.leading_zeros() & !1));
    while bit != 0 {
        if rem >= root + bit {
            rem -= root + bit;
            root = (root >> 1) + bit;
        } else {
            root >>= 1;
        }
        bit >>= 2;
    }
    (root as u64, rem != 0)
}

pub(crate) fn sqrt(fmt: Fmt, a: u64, rm: RoundingMode, fl: &mut Flags) -> u64 {
    if fmt.is_nan(a) {
        return nan_result(fmt, &[a], fl);
    }
    if fmt.is_zero(a) {
        return a;
    }
    if fmt.sign(a) {
        return invalid(fmt, fl);
    }
    if fmt.is_inf(a) {
        return a;
    }
    let (e, m) = fmt.unpack_norm(a);
    // Scale the radicand so its leading one is at bit 124 or 125 with an even
    // exponent; the root then lies in [2^62, 2^63).
    let mut k = 124 - fmt.frac_bits as i32;
    if (e - k) & 1 != 0 {
        k += 1;
    }
    let (root, inexact) = isqrt128((m as u128) << k);
    round_pack(fmt, false, (e - k) / 2, root | inexact as u64, rm, fl)
}

pub(crate) fn mul_add(fmt: Fmt, a: u64, b: u64, c: u64, rm: RoundingMode, fl: &mut Flags) -> u64 {
    let (a_inf, b_inf, c_inf) = (fmt.is_inf(a), fmt.is_inf(b), fmt.is_inf(c));
    let (a_zero, b_zero, c_zero) = (fmt.is_zero(a), fmt.is_zero(b), fmt.is_zero(c));
    // inf * 0 is invalid even when the addend is a quiet NaN.
    if (a_inf && b_zero) || (a_zero && b_inf) {
        *fl |= Flags::NV;
        return fmt.canonical_nan();
    }
    if fmt.is_nan(a) || fmt.is_nan(b) || fmt.is_nan(c) {
        return nan_result(fmt, &[a, b, c], fl);
    }
    let sp = fmt.sign(a) != fmt.sign(b);
    let sc = fmt.sign(c);
    if a_inf || b_inf {
        if c_inf && sc != sp {
            return invalid(fmt, fl);
        }
        return fmt.signed_inf(sp);
    }
    if c_inf {
        return c;
    }
    if a_zero || b_zero {
        if c_zero {
            return if sp == sc { fmt.signed_zero(sp) } else { exact_zero_sum(fmt, rm) };
        }
        return c;
    }
    let (ea, ma) = fmt.unpack(a);
    let (eb, mb) = fmt.unpack(b);
    let prod = ma as u128 * mb as u128;
    if c_zero {
        return round_pack128(fmt, sp, ea + eb, prod, rm, fl);
    }
    // Align the exact product and the addend with their leading ones at bit
    // 124, leaving headroom for a carry out of the sum.
    let lp = prod.leading_zeros() as i32 - 3;
    let (ep, p) = (ea + eb - lp, prod << lp);
    let (ec, mc) = fmt.unpack(c);
    let lc = (mc as u128).leading_zeros() as i32 - 3;
    let (ec, cc) = (ec - lc, (mc as u128) << lc);

    let ((s_big, e_big, big), (s_small, e_small, small)) =
        if (ep, p) >= (ec, cc) { ((sp, ep, p), (sc, ec, cc)) } else { ((sc, ec, cc), (sp, ep, p)) };
    let y = shr_jam128(small, (e_big - e_small) as u32);
    let sum = if s_big == s_small { big + y } else { big - y };
    if sum == 0 {
        return exact_zero_sum(fmt, rm);
    }
    round_pack128(fmt, s_big, e_big, sum, rm, fl)
}
