//! Conversions between formats and to and from integers.

use super::pack::{round_pack, round_up, shr_jam128, Fmt};
use super::{Flags, RoundingMode};

/// Converts to an integer `width` bits wide (32 or 64), returning its two's
/// complement bit pattern sign-extended to 64 bits for signed results.
///
/// Out-of-range values and infinities saturate and raise NV; NaN converts to
/// the largest positive value. NX is raised only for in-range inexact results.
pub(crate) fn to_int(fmt: Fmt, a: u64, rm: RoundingMode, signed: bool, width: u32, fl: &mut Flags) -> u64 {
    let sign = fmt.sign(a);
    let max_pos: u64 = if signed { (1u64 << (width - 1)) - 1 } else { u64::MAX >> (64 - width) };
    let saturate = |neg: bool, fl: &mut Flags| {
        *fl |= Flags::NV;
        match (neg, signed) {
            (false, _) => max_pos,
            (true, true) => (-1i64 << (width - 1)) as u64,
            (true, false) => 0,
        }
    };
    if fmt.is_nan(a) {
        return saturate(false, fl);
    }
    if fmt.is_inf(a) {
        return saturate(sign, fl);
    }
    if fmt.is_zero(a) {
        return 0;
    }
    let (e, m) = fmt.unpack(a);
    let (mag, inexact) = if e >= 0 {
        if e > 64 {
            return saturate(sign, fl);
        }
        ((m as u128) << e, false)
    } else {
        // 64.64 fixed point: the high half is the integer part.
        let fixed = shr_jam128((m as u128) << 64, (-e) as u32);
        let int = fixed >> 64;
        let frac = fixed as u64;
        let up = round_up(rm, sign, frac, 1 << 63, int & 1 != 0);
        (int + up as u128, frac != 0)
    };
    let result = if sign {
        let limit: u128 = if signed { 1 << (width - 1) } else { 0 };
        if mag > limit {
            return saturate(true, fl);
        }
        (mag as u64).wrapping_neg()
    } else {
        if mag > max_pos as u128 {
            return saturate(false, fl);
        }
        mag as u64
    };
    if inexact {
        *fl |= Flags::NX;
    }
    result
}

/// Converts the integer `(-1)^sign * mag` to `fmt`.
pub(crate) fn from_int(fmt: Fmt, sign: bool, mag: u64, rm: RoundingMode, fl: &mut Flags) -> u64 {
    if mag == 0 {
        return 0;
    }
    round_pack(fmt, sign, 0, mag, rm, fl)
}

/// Converts a value from format `src` to format `dst`.
pub(crate) fn convert(src: Fmt, dst: Fmt, a: u64, rm: RoundingMode, fl: &mut Flags) -> u64 {
    let sign = src.sign(a);
    if src.is_nan(a) {
        if src.is_snan(a) {
            *fl |= Flags::NV;
        }
        return dst.canonical_nan();
    }
    if src.is_inf(a) {
        return dst.signed_inf(sign);
    }
    if src.is_zero(a) {
        return dst.signed_zero(sign);
    }
    let (e, m) = src.unpack(a);
    round_pack(dst, sign, e, m, rm, fl)
}
