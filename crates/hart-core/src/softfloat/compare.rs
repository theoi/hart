//! Comparisons, min/max, and classification.

use super::pack::Fmt;
use super::Flags;

pub(crate) fn eq(fmt: Fmt, a: u64, b: u64, fl: &mut Flags) -> bool {
    if fmt.is_nan(a) || fmt.is_nan(b) {
        if fmt.is_snan(a) || fmt.is_snan(b) {
            *fl |= Flags::NV;
        }
        return false;
    }
    a == b || (a | b) & fmt.abs_mask() == 0
}

pub(crate) fn lt(fmt: Fmt, a: u64, b: u64, fl: &mut Flags) -> bool {
    if fmt.is_nan(a) || fmt.is_nan(b) {
        *fl |= Flags::NV;
        return false;
    }
    let (sa, sb) = (fmt.sign(a), fmt.sign(b));
    if sa != sb {
        return sa && (a | b) & fmt.abs_mask() != 0;
    }
    a != b && (sa != (a < b))
}

pub(crate) fn le(fmt: Fmt, a: u64, b: u64, fl: &mut Flags) -> bool {
    if fmt.is_nan(a) || fmt.is_nan(b) {
        *fl |= Flags::NV;
        return false;
    }
    let (sa, sb) = (fmt.sign(a), fmt.sign(b));
    if sa != sb {
        return sa || (a | b) & fmt.abs_mask() == 0;
    }
    a == b || (sa != (a < b))
}

/// RISC-V `fmin`/`fmax`: a NaN operand is ignored unless both are NaN, any
/// signaling NaN raises NV, and -0 is treated as less than +0.
pub(crate) fn min_max(fmt: Fmt, a: u64, b: u64, want_max: bool, fl: &mut Flags) -> u64 {
    let (a_nan, b_nan) = (fmt.is_nan(a), fmt.is_nan(b));
    if a_nan || b_nan {
        if fmt.is_snan(a) || fmt.is_snan(b) {
            *fl |= Flags::NV;
        }
        return match (a_nan, b_nan) {
            (true, true) => fmt.canonical_nan(),
            (true, false) => b,
            _ => a,
        };
    }
    let (sa, sb) = (fmt.sign(a), fmt.sign(b));
    let a_less = if sa != sb { sa } else { sa != (a < b) };
    if a_less != want_max {
        a
    } else {
        b
    }
}

/// RISC-V `fclass` mask bits.
pub(crate) const CLASS_NEG_INF: u64 = 1 << 0;
pub(crate) const CLASS_NEG_NORMAL: u64 = 1 << 1;
pub(crate) const CLASS_NEG_SUBNORMAL: u64 = 1 << 2;
pub(crate) const CLASS_NEG_ZERO: u64 = 1 << 3;
pub(crate) const CLASS_POS_ZERO: u64 = 1 << 4;
pub(crate) const CLASS_POS_SUBNORMAL: u64 = 1 << 5;
pub(crate) const CLASS_POS_NORMAL: u64 = 1 << 6;
pub(crate) const CLASS_POS_INF: u64 = 1 << 7;
pub(crate) const CLASS_SNAN: u64 = 1 << 8;
pub(crate) const CLASS_QNAN: u64 = 1 << 9;

pub(crate) fn classify(fmt: Fmt, a: u64) -> u64 {
    let sign = fmt.sign(a);
    let e = fmt.exp_field(a);
    let frac = a & fmt.frac_mask();
    let pick = |neg: u64, pos: u64| if sign { neg } else { pos };
    if e == fmt.exp_max() {
        if frac == 0 {
            pick(CLASS_NEG_INF, CLASS_POS_INF)
        } else if frac & fmt.quiet_bit() != 0 {
            CLASS_QNAN
        } else {
            CLASS_SNAN
        }
    } else if e == 0 {
        if frac == 0 {
            pick(CLASS_NEG_ZERO, CLASS_POS_ZERO)
        } else {
            pick(CLASS_NEG_SUBNORMAL, CLASS_POS_SUBNORMAL)
        }
    } else {
        pick(CLASS_NEG_NORMAL, CLASS_POS_NORMAL)
    }
}
