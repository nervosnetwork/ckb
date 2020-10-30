//! Rational numbers.
#[cfg(test)]
mod tests;

use numext_fixed_uint::U256;
use std::cmp::Ordering;
use std::fmt;
use std::ops::{Add, Div, Mul, Sub};

/// Represents the ratio `numerator / denominator`, where `numerator` and `denominator` are both
/// unsigned 256-bit integers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RationalU256 {
    /// Numerator.
    numer: U256,
    /// Denominator.
    denom: U256,
}

impl fmt::Display for RationalU256 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}/{}", self.numer, self.denom)
    }
}

impl RationalU256 {
    /// Creates a new ratio `numer / denom`.
    ///
    /// ## Panics
    ///
    /// Panics when `denom` is zero.
    #[inline]
    pub fn new(numer: U256, denom: U256) -> RationalU256 {
        if denom.is_zero() {
            panic!("denominator == 0");
        }
        let mut ret = RationalU256::new_raw(numer, denom);
        ret.reduce();
        ret
    }

    /// Creates a new ratio `numer / denom` without checking whether `denom` is zero.
    #[inline]
    pub const fn new_raw(numer: U256, denom: U256) -> RationalU256 {
        RationalU256 { numer, denom }
    }

    /// Creates a new ratio `t / 1`.
    #[inline]
    pub const fn from_u256(t: U256) -> RationalU256 {
        RationalU256::new_raw(t, U256::one())
    }

    /// Tells whether the numerator is zero.
    #[inline]
    pub fn is_zero(&self) -> bool {
        self.numer.is_zero()
    }

    /// Creates a new ratio `0 / 1`.
    #[inline]
    pub const fn zero() -> RationalU256 {
        RationalU256::new_raw(U256::zero(), U256::one())
    }

    /// Creates a new ratio `1 / 1`.
    #[inline]
    pub const fn one() -> RationalU256 {
        RationalU256::new_raw(U256::one(), U256::one())
    }

    /// Rounds down the ratio into an unsigned 256-bit integer.
    #[inline]
    pub fn into_u256(self) -> U256 {
        self.numer / self.denom
    }

    /// Computes `self - rhs` and saturates the result to zero when `self` is less than `rhs`.
    ///
    /// Returns `self - rhs` when `self > rhs`, returns zero otherwise.
    #[inline]
    pub fn saturating_sub(self, rhs: RationalU256) -> Self {
        if self.denom == rhs.denom {
            let (numer, overflowing) = self.numer.overflowing_sub(&rhs.numer);
            return if overflowing {
                RationalU256::zero()
            } else {
                RationalU256::new(numer, self.denom)
            };
        }

        let gcd = self.denom.gcd(&rhs.denom);
        let lcm = &self.denom * (&rhs.denom / gcd);
        let lhs_numer = &self.numer * (&lcm / self.denom);
        let rhs_numer = &rhs.numer * (&lcm / &rhs.denom);

        let (numer, overflowing) = lhs_numer.overflowing_sub(&rhs_numer);
        if overflowing {
            RationalU256::zero()
        } else {
            RationalU256::new(numer, lcm)
        }
    }

    /// Computes `self - rhs` and saturates the result to zero when `self` is less than `rhs`.
    ///
    /// Returns `self - rhs` when `self > rhs`, returns zero otherwise.
    #[inline]
    pub fn saturating_sub_u256(self, rhs: U256) -> Self {
        let (numer, overflowing) = self.numer.overflowing_sub(&(&self.denom * rhs));
        if overflowing {
            RationalU256::zero()
        } else {
            RationalU256::new_raw(numer, self.denom)
        }
    }

    /// Puts self into lowest terms, with denom > 0.
    fn reduce(&mut self) {
        let g = self.numer.gcd(&self.denom);
        self.numer = &self.numer / &g;
        self.denom = &self.denom / &g;
    }
}

// a/b * c/d = (a/gcd_ad)*(c/gcd_bc) / ((d/gcd_ad)*(b/gcd_bc))
impl Mul<&RationalU256> for &RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn mul(self, rhs: &RationalU256) -> RationalU256 {
        let gcd_ad = self.numer.gcd(&rhs.denom);
        let gcd_bc = self.denom.gcd(&rhs.numer);

        RationalU256::new_raw(
            (&self.numer / &gcd_ad) * (&rhs.numer / &gcd_bc),
            (&self.denom / gcd_bc) * (&rhs.denom / gcd_ad),
        )
    }
}

impl Mul<RationalU256> for &RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn mul(self, rhs: RationalU256) -> RationalU256 {
        self.mul(&rhs)
    }
}

impl Mul<&RationalU256> for RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn mul(self, rhs: &RationalU256) -> RationalU256 {
        (&self).mul(rhs)
    }
}

impl Mul<RationalU256> for RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn mul(self, rhs: RationalU256) -> RationalU256 {
        (&self).mul(&rhs)
    }
}

// a/b * c/1 = (a*c) / (b*1) = (a*c) / b
impl Mul<&U256> for &RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn mul(self, rhs: &U256) -> RationalU256 {
        let gcd = self.denom.gcd(&rhs);
        RationalU256::new_raw(&self.numer * (rhs.div(&gcd)), (&self.denom).div(gcd))
    }
}

impl Mul<U256> for &RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn mul(self, rhs: U256) -> RationalU256 {
        self.mul(&rhs)
    }
}

impl Mul<U256> for RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn mul(self, rhs: U256) -> RationalU256 {
        (&self).mul(&rhs)
    }
}

impl Mul<&U256> for RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn mul(self, rhs: &U256) -> RationalU256 {
        (&self).mul(rhs)
    }
}

// (a/b) / (c/d) = (a/gcd_ac)*(d/gcd_bd) / ((c/gcd_ac)*(b/gcd_bd))
impl Div<&RationalU256> for &RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn div(self, rhs: &RationalU256) -> RationalU256 {
        let gcd_ac = self.numer.gcd(&rhs.numer);
        let gcd_bd = self.denom.gcd(&rhs.denom);
        RationalU256::new_raw(
            (&self.numer / &gcd_ac) * (&rhs.denom / &gcd_bd),
            (&self.denom / gcd_bd) * (&rhs.numer / gcd_ac),
        )
    }
}

impl Div<RationalU256> for RationalU256 {
    type Output = RationalU256;

    #[inline]
    fn div(self, rhs: RationalU256) -> RationalU256 {
        (&self).div(&rhs)
    }
}

impl Div<RationalU256> for &RationalU256 {
    type Output = RationalU256;

    #[inline]
    fn div(self, rhs: RationalU256) -> RationalU256 {
        (&self).div(&rhs)
    }
}

impl Div<&RationalU256> for RationalU256 {
    type Output = RationalU256;

    #[inline]
    fn div(self, rhs: &RationalU256) -> RationalU256 {
        (&self).div(rhs)
    }
}

// (a/b) / (c/1) = (a*1) / (b*c) = a / (b*c)
impl Div<&U256> for &RationalU256 {
    type Output = RationalU256;

    #[inline]
    fn div(self, rhs: &U256) -> RationalU256 {
        let gcd = self.numer.gcd(&rhs);
        RationalU256::new_raw(&self.numer / &gcd, &self.denom * (rhs / gcd))
    }
}

impl Div<U256> for RationalU256 {
    type Output = RationalU256;

    #[inline]
    fn div(self, rhs: U256) -> RationalU256 {
        (&self).div(&rhs)
    }
}

impl Div<&U256> for RationalU256 {
    type Output = RationalU256;

    #[inline]
    fn div(self, rhs: &U256) -> RationalU256 {
        (&self).div(rhs)
    }
}

impl Div<U256> for &RationalU256 {
    type Output = RationalU256;

    #[inline]
    fn div(self, rhs: U256) -> RationalU256 {
        (self).div(&rhs)
    }
}

impl Add<&RationalU256> for &RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn add(self, rhs: &RationalU256) -> RationalU256 {
        if self.denom == rhs.denom {
            RationalU256::new(&self.numer + &rhs.numer, self.denom.clone())
        } else {
            let gcd = self.denom.gcd(&rhs.denom);
            let lcm = &self.denom * (&rhs.denom / gcd);
            let lhs_numer = &self.numer * (&lcm / &self.denom);
            let rhs_numer = &rhs.numer * (&lcm / &rhs.denom);

            RationalU256::new(lhs_numer + rhs_numer, lcm)
        }
    }
}

impl Add<RationalU256> for RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn add(self, rhs: RationalU256) -> RationalU256 {
        (&self).add(&rhs)
    }
}

impl Add<&RationalU256> for RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn add(self, rhs: &RationalU256) -> RationalU256 {
        (&self).add(rhs)
    }
}

impl Add<RationalU256> for &RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn add(self, rhs: RationalU256) -> RationalU256 {
        (self).add(&rhs)
    }
}

impl Add<&U256> for &RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn add(self, rhs: &U256) -> RationalU256 {
        RationalU256::new_raw(&self.numer + (&self.denom * rhs), self.denom.clone())
    }
}

impl Add<U256> for RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn add(self, rhs: U256) -> RationalU256 {
        (&self).add(&rhs)
    }
}

impl Add<&U256> for RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn add(self, rhs: &U256) -> RationalU256 {
        (&self).add(rhs)
    }
}

impl Add<U256> for &RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn add(self, rhs: U256) -> RationalU256 {
        self.add(&rhs)
    }
}

impl Sub<&RationalU256> for &RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn sub(self, rhs: &RationalU256) -> RationalU256 {
        if self.denom == rhs.denom {
            RationalU256::new(&self.numer - &rhs.numer, self.denom.clone())
        } else {
            let gcd = self.denom.gcd(&rhs.denom);
            let lcm = &self.denom * (&rhs.denom / gcd);
            let lhs_numer = &self.numer * (&lcm / &self.denom);
            let rhs_numer = &rhs.numer * (&lcm / &rhs.denom);

            RationalU256::new(lhs_numer - rhs_numer, lcm)
        }
    }
}

impl Sub<RationalU256> for RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn sub(self, rhs: RationalU256) -> RationalU256 {
        (&self).sub(&rhs)
    }
}

impl Sub<&RationalU256> for RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn sub(self, rhs: &RationalU256) -> RationalU256 {
        (&self).sub(rhs)
    }
}

impl Sub<RationalU256> for &RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn sub(self, rhs: RationalU256) -> RationalU256 {
        (&self).sub(&rhs)
    }
}

impl Sub<&U256> for &RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn sub(self, rhs: &U256) -> RationalU256 {
        RationalU256::new_raw(&self.numer - (&self.denom * rhs), self.denom.clone())
    }
}

impl Sub<U256> for RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn sub(self, rhs: U256) -> RationalU256 {
        (&self).sub(&rhs)
    }
}

impl Sub<&U256> for RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn sub(self, rhs: &U256) -> RationalU256 {
        (&self).sub(rhs)
    }
}

impl Sub<U256> for &RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn sub(self, rhs: U256) -> RationalU256 {
        self.sub(&rhs)
    }
}

impl PartialOrd for RationalU256 {
    fn partial_cmp(&self, other: &RationalU256) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RationalU256 {
    fn cmp(&self, other: &RationalU256) -> Ordering {
        let gcd = self.denom.gcd(&other.denom);
        let lhs = &self.numer * (&other.denom / &gcd);
        let rhs = &other.numer * (&self.denom / &gcd);
        lhs.cmp(&rhs)
    }
}
