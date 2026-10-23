//! Format descriptions, unpacking, and the shared round-and-pack step.

use super::{Flags, RoundingMode};

/// An IEEE 754 binary interchange format, described by its field widths.
/// Bit patterns of every format are carried in the low bits of a `u64`.
#[derive(Clone, Copy)]
pub(crate) struct Fmt {
    pub exp_bits: u32,
    pub frac_bits: u32,
}

pub(crate) const F32: Fmt = Fmt { exp_bits: 8, frac_bits: 23 };
pub(crate) const F64: Fmt = Fmt { exp_bits: 11, frac_bits: 52 };

impl Fmt {
    #[inline(always)]
    pub const fn bias(self) -> i32 {
        (1 << (self.exp_bits - 1)) - 1
    }

    /// The all-ones exponent field value used by infinities and NaNs.
    #[inline(always)]
    pub const fn exp_max(self) -> u64 {
        (1 << self.exp_bits) - 1
    }

    #[inline(always)]
    pub const fn sign_bit(self) -> u64 {
        1 << (self.exp_bits + self.frac_bits)
    }

    #[inline(always)]
    pub const fn frac_mask(self) -> u64 {
        (1 << self.frac_bits) - 1
    }

    #[inline(always)]
    pub const fn quiet_bit(self) -> u64 {
        1 << (self.frac_bits - 1)
    }

    #[inline(always)]
    pub const fn abs_mask(self) -> u64 {
        self.sign_bit() - 1
    }

    #[inline(always)]
    pub const fn sign(self, x: u64) -> bool {
        x & self.sign_bit() != 0
    }

    #[inline(always)]
    pub const fn exp_field(self, x: u64) -> u64 {
        (x >> self.frac_bits) & self.exp_max()
    }

    #[inline(always)]
    pub const fn inf_bits(self) -> u64 {
        self.exp_max() << self.frac_bits
    }

    #[inline(always)]
    pub const fn canonical_nan(self) -> u64 {
        self.inf_bits() | self.quiet_bit()
    }

    #[inline(always)]
    pub const fn is_nan(self, x: u64) -> bool {
        x & self.abs_mask() > self.inf_bits()
    }

    #[inline(always)]
    pub const fn is_snan(self, x: u64) -> bool {
        self.is_nan(x) && x & self.quiet_bit() == 0
    }

    #[inline(always)]
    pub const fn is_inf(self, x: u64) -> bool {
        x & self.abs_mask() == self.inf_bits()
    }

    #[inline(always)]
    pub const fn is_zero(self, x: u64) -> bool {
        x & self.abs_mask() == 0
    }

    #[inline(always)]
    pub const fn signed_zero(self, sign: bool) -> u64 {
        if sign {
            self.sign_bit()
        } else {
            0
        }
    }

    #[inline(always)]
    pub const fn signed_inf(self, sign: bool) -> u64 {
        self.signed_zero(sign) | self.inf_bits()
    }

    /// Splits a finite nonzero value into `(exp, sig)` with
    /// `|x| = sig * 2^exp`. Subnormal significands are left unnormalized.
    #[inline(always)]
    pub const fn unpack(self, x: u64) -> (i32, u64) {
        let e = self.exp_field(x);
        let frac = x & self.frac_mask();
        let base = self.bias() + self.frac_bits as i32;
        if e == 0 {
            (1 - base, frac)
        } else {
            (e as i32 - base, frac | (1 << self.frac_bits))
        }
    }

    /// Like [`Fmt::unpack`], but shifts the significand so its leading one
    /// sits at bit `frac_bits`.
    #[inline(always)]
    pub const fn unpack_norm(self, x: u64) -> (i32, u64) {
        let (e, m) = self.unpack(x);
        let shift = m.leading_zeros() as i32 - (63 - self.frac_bits as i32);
        (e - shift, m << shift)
    }
}

/// Shifts `x` right by `n` bits, ORing any discarded one bits into bit 0.
#[inline(always)]
pub(crate) const fn shr_jam64(x: u64, n: u32) -> u64 {
    if n == 0 {
        x
    } else if n < 64 {
        (x >> n) | ((x << (64 - n)) != 0) as u64
    } else {
        (x != 0) as u64
    }
}

/// 128-bit version of [`shr_jam64`].
#[inline(always)]
pub(crate) const fn shr_jam128(x: u128, n: u32) -> u128 {
    if n == 0 {
        x
    } else if n < 128 {
        (x >> n) | ((x << (128 - n)) != 0) as u128
    } else {
        (x != 0) as u128
    }
}

/// Decides whether a value whose truncated significand has least significant
/// bit `lsb` and discarded bits `rest` (compared against `half`, the weight of
/// one half unit in the last place) must be rounded up in magnitude.
#[inline(always)]
pub(crate) const fn round_up(rm: RoundingMode, sign: bool, rest: u64, half: u64, lsb: bool) -> bool {
    match rm {
        RoundingMode::NearestEven => rest > half || (rest == half && lsb),
        RoundingMode::NearestMaxMag => rest >= half,
        RoundingMode::TowardZero => false,
        RoundingMode::Down => sign && rest != 0,
        RoundingMode::Up => !sign && rest != 0,
    }
}

/// The result of an overflow: infinity, or the largest finite value when the
/// rounding direction points toward zero.
#[inline]
pub(crate) fn overflow(fmt: Fmt, sign: bool, rm: RoundingMode, fl: &mut Flags) -> u64 {
    *fl |= Flags::OF | Flags::NX;
    let to_inf = match rm {
        RoundingMode::NearestEven | RoundingMode::NearestMaxMag => true,
        RoundingMode::TowardZero => false,
        RoundingMode::Down => sign,
        RoundingMode::Up => !sign,
    };
    if to_inf {
        fmt.signed_inf(sign)
    } else {
        fmt.signed_zero(sign) | (fmt.inf_bits() - 1)
    }
}

/// Rounds `(-1)^sign * sig * 2^exp` to `fmt` and returns its bit pattern.
///
/// `sig` must be nonzero. If nonzero bits were discarded below bit 0, bit 0
/// must be set (a sticky bit); callers keep at least `frac_bits + 3`
/// significant bits above any sticky bit so it never reaches the rounding
/// position.
pub(crate) fn round_pack(fmt: Fmt, sign: bool, exp: i32, sig: u64, rm: RoundingMode, fl: &mut Flags) -> u64 {
    debug_assert!(sig != 0);
    // Normalize so that the leading one is at bit 62.
    let lz = sig.leading_zeros() as i32;
    let (mut sig, e) = if lz == 0 { (shr_jam64(sig, 1), exp + 1) } else { (sig << (lz - 1), exp - (lz - 1)) };
    // Biased exponent of the leading bit.
    let mut be = e + 62 + fmt.bias();
    // Bits below the result's least significant bit.
    let shift = 62 - fmt.frac_bits;
    let mask = (1u64 << shift) - 1;
    let half = 1u64 << (shift - 1);

    if be >= fmt.exp_max() as i32 {
        return overflow(fmt, sign, rm, fl);
    }

    let mut tiny = false;
    if be <= 0 {
        // Tininess after rounding: the result is tiny unless rounding to full
        // precision with an unbounded exponent carries up to 2^emin.
        tiny = if be < 0 {
            true
        } else {
            let m = sig >> shift;
            let carries = m == (1u64 << (fmt.frac_bits + 1)) - 1 && round_up(rm, sign, sig & mask, half, m & 1 != 0);
            !carries
        };
        sig = shr_jam64(sig, (1 - be) as u32);
        be = 1;
    }

    let rest = sig & mask;
    let mut m = sig >> shift;
    if rest != 0 {
        *fl |= Flags::NX;
        if tiny {
            *fl |= Flags::UF;
        }
        if round_up(rm, sign, rest, half, m & 1 != 0) {
            m += 1;
        }
    }
    // The hidden bit of `m` (or a carry out of it) adds into the exponent field.
    let mag = (((be - 1) as u64) << fmt.frac_bits) + m;
    if mag >= fmt.inf_bits() {
        return overflow(fmt, sign, rm, fl);
    }
    fmt.signed_zero(sign) | mag
}

/// [`round_pack`] for a 128-bit significand.
#[inline]
pub(crate) fn round_pack128(fmt: Fmt, sign: bool, exp: i32, sig: u128, rm: RoundingMode, fl: &mut Flags) -> u64 {
    let lz = sig.leading_zeros();
    if lz >= 64 {
        round_pack(fmt, sign, exp, sig as u64, rm, fl)
    } else {
        let shift = 64 - lz;
        let s = shr_jam128(sig, shift) as u64;
        round_pack(fmt, sign, exp + shift as i32, s, rm, fl)
    }
}
