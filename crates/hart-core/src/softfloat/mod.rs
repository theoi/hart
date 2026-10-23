//! IEEE 754 binary32 and binary64 arithmetic built from integer operations.
//!
//! The behavior follows the RISC-V F and D extensions:
//!
//! - every NaN result is the canonical NaN (no payload propagation),
//! - tininess is detected after rounding,
//! - float-to-integer conversions saturate, and NaN converts to the largest
//!   positive integer,
//! - `min`/`max` follow the RISC-V 2.2 semantics (minimumNumber/maximumNumber,
//!   with -0 ordered below +0).
//!
//! Values are passed as raw bit patterns: `u32` for binary32 and `u64` for
//! binary64. Exception flags accumulate into a [`Flags`] value whose bit layout
//! matches the RISC-V `fflags` CSR.

mod arith;
mod compare;
mod convert;
mod pack;
#[cfg(test)]
mod tests;

use pack::{F32, F64};

/// A RISC-V rounding mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RoundingMode {
    /// Round to nearest, ties to even (`RNE`, rm = 0).
    NearestEven,
    /// Round toward zero (`RTZ`, rm = 1).
    TowardZero,
    /// Round down, toward negative infinity (`RDN`, rm = 2).
    Down,
    /// Round up, toward positive infinity (`RUP`, rm = 3).
    Up,
    /// Round to nearest, ties to max magnitude (`RMM`, rm = 4).
    NearestMaxMag,
}

impl RoundingMode {
    /// Decodes a 3-bit RISC-V rounding-mode value. Returns `None` for the
    /// reserved encodings 5 and 6 and for 7 (`DYN`), which the caller must
    /// resolve through `frm` first.
    pub fn from_bits(rm: u32) -> Option<Self> {
        match rm {
            0 => Some(Self::NearestEven),
            1 => Some(Self::TowardZero),
            2 => Some(Self::Down),
            3 => Some(Self::Up),
            4 => Some(Self::NearestMaxMag),
            _ => None,
        }
    }

    /// The 3-bit RISC-V encoding of this rounding mode.
    pub fn bits(self) -> u32 {
        self as u32
    }
}

/// Accrued IEEE exception flags, laid out like the RISC-V `fflags` CSR.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Flags(pub u8);

impl Flags {
    /// Inexact.
    pub const NX: Flags = Flags(1 << 0);
    /// Underflow.
    pub const UF: Flags = Flags(1 << 1);
    /// Overflow.
    pub const OF: Flags = Flags(1 << 2);
    /// Divide by zero.
    pub const DZ: Flags = Flags(1 << 3);
    /// Invalid operation.
    pub const NV: Flags = Flags(1 << 4);

    pub const fn empty() -> Self {
        Flags(0)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn contains(self, other: Flags) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl core::ops::BitOr for Flags {
    type Output = Flags;
    fn bitor(self, rhs: Flags) -> Flags {
        Flags(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign for Flags {
    fn bitor_assign(&mut self, rhs: Flags) {
        self.0 |= rhs.0;
    }
}

/// The RISC-V canonical binary32 NaN.
pub const F32_CANONICAL_NAN: u32 = 0x7fc0_0000;
/// The RISC-V canonical binary64 NaN.
pub const F64_CANONICAL_NAN: u64 = 0x7ff8_0000_0000_0000;

/// NaN-boxes a binary32 value into a 64-bit FP register image.
#[inline]
pub fn nan_box_f32(a: u32) -> u64 {
    0xffff_ffff_0000_0000 | a as u64
}

/// Extracts a binary32 operand from a 64-bit FP register image. An improperly
/// NaN-boxed value reads as the canonical NaN.
#[inline]
pub fn unbox_f32(reg: u64) -> u32 {
    if reg >> 32 == 0xffff_ffff {
        reg as u32
    } else {
        F32_CANONICAL_NAN
    }
}

macro_rules! format_api {
    (
        $fmt:ident, $t:ty,
        add: $add:ident, sub: $sub:ident, mul: $mul:ident, div: $div:ident, sqrt: $sqrt:ident,
        mul_add: $mul_add:ident, fmadd: $fmadd:ident, fmsub: $fmsub:ident,
        fnmsub: $fnmsub:ident, fnmadd: $fnmadd:ident,
        eq: $eq:ident, lt: $lt:ident, le: $le:ident, min: $min:ident, max: $max:ident,
        classify: $classify:ident, sgnj: $sgnj:ident, sgnjn: $sgnjn:ident, sgnjx: $sgnjx:ident,
        to_i32: $to_i32:ident, to_u32: $to_u32:ident, to_i64: $to_i64:ident, to_u64: $to_u64:ident,
        from_i32: $from_i32:ident, from_u32: $from_u32:ident,
        from_i64: $from_i64:ident, from_u64: $from_u64:ident,
    ) => {
        /// `a + b`.
        #[inline]
        pub fn $add(a: $t, b: $t, rm: RoundingMode, fl: &mut Flags) -> $t {
            arith::add($fmt, a as u64, b as u64, rm, fl) as $t
        }

        /// `a - b`.
        #[inline]
        pub fn $sub(a: $t, b: $t, rm: RoundingMode, fl: &mut Flags) -> $t {
            arith::add($fmt, a as u64, (b as u64) ^ $fmt.sign_bit(), rm, fl) as $t
        }

        /// `a * b`.
        #[inline]
        pub fn $mul(a: $t, b: $t, rm: RoundingMode, fl: &mut Flags) -> $t {
            arith::mul($fmt, a as u64, b as u64, rm, fl) as $t
        }

        /// `a / b`.
        #[inline]
        pub fn $div(a: $t, b: $t, rm: RoundingMode, fl: &mut Flags) -> $t {
            arith::div($fmt, a as u64, b as u64, rm, fl) as $t
        }

        /// Square root of `a`.
        #[inline]
        pub fn $sqrt(a: $t, rm: RoundingMode, fl: &mut Flags) -> $t {
            arith::sqrt($fmt, a as u64, rm, fl) as $t
        }

        /// Fused `a * b + c` with a single rounding.
        #[inline]
        pub fn $mul_add(a: $t, b: $t, c: $t, rm: RoundingMode, fl: &mut Flags) -> $t {
            arith::mul_add($fmt, a as u64, b as u64, c as u64, rm, fl) as $t
        }

        /// RISC-V `fmadd`: `a * b + c`.
        #[inline]
        pub fn $fmadd(a: $t, b: $t, c: $t, rm: RoundingMode, fl: &mut Flags) -> $t {
            $mul_add(a, b, c, rm, fl)
        }

        /// RISC-V `fmsub`: `a * b - c`.
        #[inline]
        pub fn $fmsub(a: $t, b: $t, c: $t, rm: RoundingMode, fl: &mut Flags) -> $t {
            let s = $fmt.sign_bit() as $t;
            $mul_add(a, b, c ^ s, rm, fl)
        }

        /// RISC-V `fnmsub`: `-(a * b) + c`.
        #[inline]
        pub fn $fnmsub(a: $t, b: $t, c: $t, rm: RoundingMode, fl: &mut Flags) -> $t {
            let s = $fmt.sign_bit() as $t;
            $mul_add(a ^ s, b, c, rm, fl)
        }

        /// RISC-V `fnmadd`: `-(a * b) - c`.
        #[inline]
        pub fn $fnmadd(a: $t, b: $t, c: $t, rm: RoundingMode, fl: &mut Flags) -> $t {
            let s = $fmt.sign_bit() as $t;
            $mul_add(a ^ s, b, c ^ s, rm, fl)
        }

        /// RISC-V `feq`: quiet equality; only signaling NaNs raise NV.
        #[inline]
        pub fn $eq(a: $t, b: $t, fl: &mut Flags) -> bool {
            compare::eq($fmt, a as u64, b as u64, fl)
        }

        /// RISC-V `flt`: signaling less-than; any NaN raises NV.
        #[inline]
        pub fn $lt(a: $t, b: $t, fl: &mut Flags) -> bool {
            compare::lt($fmt, a as u64, b as u64, fl)
        }

        /// RISC-V `fle`: signaling less-or-equal; any NaN raises NV.
        #[inline]
        pub fn $le(a: $t, b: $t, fl: &mut Flags) -> bool {
            compare::le($fmt, a as u64, b as u64, fl)
        }

        /// RISC-V `fmin`.
        #[inline]
        pub fn $min(a: $t, b: $t, fl: &mut Flags) -> $t {
            compare::min_max($fmt, a as u64, b as u64, false, fl) as $t
        }

        /// RISC-V `fmax`.
        #[inline]
        pub fn $max(a: $t, b: $t, fl: &mut Flags) -> $t {
            compare::min_max($fmt, a as u64, b as u64, true, fl) as $t
        }

        /// RISC-V `fclass`: a one-hot 10-bit classification mask.
        #[inline]
        pub fn $classify(a: $t) -> u64 {
            compare::classify($fmt, a as u64)
        }

        /// RISC-V `fsgnj`: magnitude of `a`, sign of `b`.
        #[inline]
        pub fn $sgnj(a: $t, b: $t) -> $t {
            let s = $fmt.sign_bit() as $t;
            (a & !s) | (b & s)
        }

        /// RISC-V `fsgnjn`: magnitude of `a`, opposite sign of `b`.
        #[inline]
        pub fn $sgnjn(a: $t, b: $t) -> $t {
            let s = $fmt.sign_bit() as $t;
            (a & !s) | (!b & s)
        }

        /// RISC-V `fsgnjx`: `a` with its sign XORed with the sign of `b`.
        #[inline]
        pub fn $sgnjx(a: $t, b: $t) -> $t {
            let s = $fmt.sign_bit() as $t;
            a ^ (b & s)
        }

        /// Conversion to a signed 32-bit integer (RISC-V `fcvt.w`).
        #[inline]
        pub fn $to_i32(a: $t, rm: RoundingMode, fl: &mut Flags) -> i32 {
            convert::to_int($fmt, a as u64, rm, true, 32, fl) as i32
        }

        /// Conversion to an unsigned 32-bit integer (RISC-V `fcvt.wu`).
        #[inline]
        pub fn $to_u32(a: $t, rm: RoundingMode, fl: &mut Flags) -> u32 {
            convert::to_int($fmt, a as u64, rm, false, 32, fl) as u32
        }

        /// Conversion to a signed 64-bit integer (RISC-V `fcvt.l`).
        #[inline]
        pub fn $to_i64(a: $t, rm: RoundingMode, fl: &mut Flags) -> i64 {
            convert::to_int($fmt, a as u64, rm, true, 64, fl) as i64
        }

        /// Conversion to an unsigned 64-bit integer (RISC-V `fcvt.lu`).
        #[inline]
        pub fn $to_u64(a: $t, rm: RoundingMode, fl: &mut Flags) -> u64 {
            convert::to_int($fmt, a as u64, rm, false, 64, fl)
        }

        /// Conversion from a signed 32-bit integer.
        #[inline]
        pub fn $from_i32(a: i32, rm: RoundingMode, fl: &mut Flags) -> $t {
            convert::from_int($fmt, a < 0, a.unsigned_abs() as u64, rm, fl) as $t
        }

        /// Conversion from an unsigned 32-bit integer.
        #[inline]
        pub fn $from_u32(a: u32, rm: RoundingMode, fl: &mut Flags) -> $t {
            convert::from_int($fmt, false, a as u64, rm, fl) as $t
        }

        /// Conversion from a signed 64-bit integer.
        #[inline]
        pub fn $from_i64(a: i64, rm: RoundingMode, fl: &mut Flags) -> $t {
            convert::from_int($fmt, a < 0, a.unsigned_abs(), rm, fl) as $t
        }

        /// Conversion from an unsigned 64-bit integer.
        #[inline]
        pub fn $from_u64(a: u64, rm: RoundingMode, fl: &mut Flags) -> $t {
            convert::from_int($fmt, false, a, rm, fl) as $t
        }
    };
}

format_api! {
    F32, u32,
    add: f32_add, sub: f32_sub, mul: f32_mul, div: f32_div, sqrt: f32_sqrt,
    mul_add: f32_mul_add, fmadd: f32_fmadd, fmsub: f32_fmsub,
    fnmsub: f32_fnmsub, fnmadd: f32_fnmadd,
    eq: f32_eq, lt: f32_lt, le: f32_le, min: f32_min, max: f32_max,
    classify: f32_classify, sgnj: f32_sgnj, sgnjn: f32_sgnjn, sgnjx: f32_sgnjx,
    to_i32: f32_to_i32, to_u32: f32_to_u32, to_i64: f32_to_i64, to_u64: f32_to_u64,
    from_i32: i32_to_f32, from_u32: u32_to_f32, from_i64: i64_to_f32, from_u64: u64_to_f32,
}

format_api! {
    F64, u64,
    add: f64_add, sub: f64_sub, mul: f64_mul, div: f64_div, sqrt: f64_sqrt,
    mul_add: f64_mul_add, fmadd: f64_fmadd, fmsub: f64_fmsub,
    fnmsub: f64_fnmsub, fnmadd: f64_fnmadd,
    eq: f64_eq, lt: f64_lt, le: f64_le, min: f64_min, max: f64_max,
    classify: f64_classify, sgnj: f64_sgnj, sgnjn: f64_sgnjn, sgnjx: f64_sgnjx,
    to_i32: f64_to_i32, to_u32: f64_to_u32, to_i64: f64_to_i64, to_u64: f64_to_u64,
    from_i32: i32_to_f64, from_u32: u32_to_f64, from_i64: i64_to_f64, from_u64: u64_to_f64,
}

/// Widens a binary32 value to binary64 (always exact).
#[inline]
pub fn f32_to_f64(a: u32, fl: &mut Flags) -> u64 {
    convert::convert(F32, F64, a as u64, RoundingMode::NearestEven, fl)
}

/// Narrows a binary64 value to binary32.
#[inline]
pub fn f64_to_f32(a: u64, rm: RoundingMode, fl: &mut Flags) -> u32 {
    convert::convert(F64, F32, a, rm, fl) as u32
}
