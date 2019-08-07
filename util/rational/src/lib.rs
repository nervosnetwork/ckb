#![allow(clippy::suspicious_arithmetic_impl)]

use numext_fixed_uint::U256;
use std::ops::{Add, Div, Mul, Sub};

#[derive(Clone, Debug)]
pub struct RationalU256 {
    /// Numerator.
    numer: U256,
    /// Denominator.
    denom: U256,
}

impl RationalU256 {
    #[inline]
    pub fn new(numer: U256, denom: U256) -> RationalU256 {
        if denom.is_zero() {
            panic!("denominator == 0");
        }
        let mut ret = RationalU256::new_raw(numer, denom);
        ret.reduce();
        ret
    }

    #[inline]
    pub const fn new_raw(numer: U256, denom: U256) -> RationalU256 {
        RationalU256 { numer, denom }
    }

    #[inline]
    pub const fn from_u256(t: U256) -> RationalU256 {
        RationalU256::new_raw(t, U256::one())
    }

    #[inline]
    pub fn is_zero(&self) -> bool {
        self.numer.is_zero()
    }

    #[inline]
    pub const fn zero() -> RationalU256 {
        RationalU256::new_raw(U256::zero(), U256::one())
    }

    #[inline]
    pub const fn one() -> RationalU256 {
        RationalU256::new_raw(U256::one(), U256::one())
    }

    #[inline]
    pub fn into_u256(self) -> U256 {
        self.numer / self.denom
    }

    #[inline]
    pub fn saturating_sub(self, rhs: RationalU256) -> Self {
        let (numer, overflowing) =
            (&self.numer * &rhs.denom).overflowing_sub(&(&self.denom * &rhs.numer));
        if overflowing {
            RationalU256::zero()
        } else {
            RationalU256::new(numer, &self.denom * &rhs.denom)
        }
    }

    #[inline]
    pub fn saturating_sub_u256(self, rhs: U256) -> Self {
        let (numer, overflowing) = self.numer.overflowing_sub(&(&self.denom * rhs));
        if overflowing {
            RationalU256::zero()
        } else {
            RationalU256::new(numer, self.denom.clone())
        }
    }

    /// Puts self into lowest terms, with denom > 0.
    fn reduce(&mut self) {
        let g = self.numer.gcd(&self.denom);
        self.numer = &self.numer / &g;
        self.denom = &self.denom / &g;
    }
}

// a/b * c/d = (a*c)/(b*d)
impl Mul<&RationalU256> for &RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn mul(self, rhs: &RationalU256) -> RationalU256 {
        RationalU256::new(&self.numer * &rhs.numer, &self.denom * &rhs.denom)
    }
}

impl Mul<RationalU256> for &RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn mul(self, rhs: RationalU256) -> RationalU256 {
        RationalU256::new(&self.numer * rhs.numer, &self.denom * rhs.denom)
    }
}

impl Mul<&RationalU256> for RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn mul(self, rhs: &RationalU256) -> RationalU256 {
        RationalU256::new(&self.numer * &rhs.numer, &self.denom * &rhs.denom)
    }
}

impl Mul<RationalU256> for RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn mul(self, rhs: RationalU256) -> RationalU256 {
        RationalU256::new(self.numer * rhs.numer, self.denom * rhs.denom)
    }
}

// a/b * c/1 = (a*c) / (b*1) = (a*c) / b
impl Mul<&U256> for &RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn mul(self, rhs: &U256) -> RationalU256 {
        RationalU256::new(&self.numer * rhs, self.denom.clone())
    }
}

impl Mul<U256> for &RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn mul(self, rhs: U256) -> RationalU256 {
        RationalU256::new(&self.numer * rhs, self.denom.clone())
    }
}

impl Mul<U256> for RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn mul(self, rhs: U256) -> RationalU256 {
        RationalU256::new(&self.numer * rhs, self.denom)
    }
}

impl Mul<&U256> for RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn mul(self, rhs: &U256) -> RationalU256 {
        RationalU256::new(&self.numer * rhs, self.denom)
    }
}

// (a/b) / (c/d) = (a*d) / (b*c)
impl Div<&RationalU256> for &RationalU256 {
    type Output = RationalU256;

    #[inline]
    fn div(self, rhs: &RationalU256) -> RationalU256 {
        RationalU256::new(&self.numer * &rhs.denom, &self.denom * &rhs.numer)
    }
}

impl Div<RationalU256> for RationalU256 {
    type Output = RationalU256;

    #[inline]
    fn div(self, rhs: RationalU256) -> RationalU256 {
        RationalU256::new(self.numer * rhs.denom, self.denom * rhs.numer)
    }
}

impl Div<RationalU256> for &RationalU256 {
    type Output = RationalU256;

    #[inline]
    fn div(self, rhs: RationalU256) -> RationalU256 {
        RationalU256::new(&self.numer * &rhs.denom, &self.denom * &rhs.numer)
    }
}

impl Div<&RationalU256> for RationalU256 {
    type Output = RationalU256;

    #[inline]
    fn div(self, rhs: &RationalU256) -> RationalU256 {
        RationalU256::new(&self.numer * &rhs.denom, &self.denom * &rhs.numer)
    }
}

// (a/b) / (c/1) = (a*1) / (b*c) = a / (b*c)
impl Div<&U256> for &RationalU256 {
    type Output = RationalU256;

    #[inline]
    fn div(self, rhs: &U256) -> RationalU256 {
        RationalU256::new(self.numer.clone(), &self.denom * rhs)
    }
}

impl Div<U256> for RationalU256 {
    type Output = RationalU256;

    #[inline]
    fn div(self, rhs: U256) -> RationalU256 {
        RationalU256::new(self.numer, &self.denom * rhs)
    }
}

impl Div<&U256> for RationalU256 {
    type Output = RationalU256;

    #[inline]
    fn div(self, rhs: &U256) -> RationalU256 {
        RationalU256::new(self.numer, &self.denom * rhs)
    }
}

impl Div<U256> for &RationalU256 {
    type Output = RationalU256;

    #[inline]
    fn div(self, rhs: U256) -> RationalU256 {
        RationalU256::new(self.numer.clone(), &self.denom * rhs)
    }
}

impl Add<&RationalU256> for &RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn add(self, rhs: &RationalU256) -> RationalU256 {
        RationalU256::new(
            (&self.numer * &rhs.denom) + (&self.denom * &rhs.numer),
            &self.denom * &rhs.denom,
        )
    }
}

impl Add<RationalU256> for RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn add(self, rhs: RationalU256) -> RationalU256 {
        RationalU256::new(
            (&self.numer * &rhs.denom) + (&self.denom * &rhs.numer),
            &self.denom * &rhs.denom,
        )
    }
}

impl Add<&RationalU256> for RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn add(self, rhs: &RationalU256) -> RationalU256 {
        RationalU256::new(
            (&self.numer * &rhs.denom) + (&self.denom * &rhs.numer),
            &self.denom * &rhs.denom,
        )
    }
}

impl Add<RationalU256> for &RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn add(self, rhs: RationalU256) -> RationalU256 {
        RationalU256::new(
            (&self.numer * &rhs.denom) + (&self.denom * &rhs.numer),
            &self.denom * &rhs.denom,
        )
    }
}

impl Add<U256> for RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn add(self, rhs: U256) -> RationalU256 {
        RationalU256::new(&self.numer + (&self.denom * rhs), self.denom)
    }
}

impl Add<&U256> for RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn add(self, rhs: &U256) -> RationalU256 {
        RationalU256::new(&self.numer + (&self.denom * rhs), self.denom)
    }
}

impl Add<U256> for &RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn add(self, rhs: U256) -> RationalU256 {
        RationalU256::new(&self.numer + (&self.denom * &rhs), self.denom.clone())
    }
}

impl Add<&U256> for &RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn add(self, rhs: &U256) -> RationalU256 {
        RationalU256::new(&self.numer + (&self.denom * rhs), self.denom.clone())
    }
}

impl Sub<RationalU256> for RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn sub(self, rhs: RationalU256) -> RationalU256 {
        RationalU256::new(
            (&self.numer * &rhs.denom) - (&self.denom * &rhs.numer),
            &self.denom * &rhs.denom,
        )
    }
}

impl Sub<&RationalU256> for &RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn sub(self, rhs: &RationalU256) -> RationalU256 {
        RationalU256::new(
            (&self.numer * &rhs.denom) - (&self.denom * &rhs.numer),
            &self.denom * &rhs.denom,
        )
    }
}

impl Sub<&RationalU256> for RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn sub(self, rhs: &RationalU256) -> RationalU256 {
        RationalU256::new(
            (&self.numer * &rhs.denom) - (&self.denom * &rhs.numer),
            &self.denom * &rhs.denom,
        )
    }
}

impl Sub<RationalU256> for &RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn sub(self, rhs: RationalU256) -> RationalU256 {
        RationalU256::new(
            (&self.numer * &rhs.denom) - (&self.denom * &rhs.numer),
            &self.denom * &rhs.denom,
        )
    }
}

impl Sub<U256> for RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn sub(self, rhs: U256) -> RationalU256 {
        RationalU256::new(&self.numer - (&self.denom * rhs), self.denom)
    }
}

impl Sub<&U256> for &RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn sub(self, rhs: &U256) -> RationalU256 {
        RationalU256::new(&self.numer - (&self.denom * rhs), self.denom.clone())
    }
}

impl Sub<&U256> for RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn sub(self, rhs: &U256) -> RationalU256 {
        RationalU256::new(&self.numer - (&self.denom * rhs), self.denom)
    }
}

impl Sub<U256> for &RationalU256 {
    type Output = RationalU256;
    #[inline]
    fn sub(self, rhs: U256) -> RationalU256 {
        RationalU256::new(&self.numer - (&self.denom * rhs), self.denom.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn _test_add(a: RationalU256, b: RationalU256, c: U256) {
        assert_eq!((&a + &b).into_u256(), c);
        assert_eq!((a.clone() + b.clone()).into_u256(), c);
        assert_eq!((a.clone() + &b).into_u256(), c);
        assert_eq!((&a + b.clone()).into_u256(), c);
    }

    fn _test_add_u256(a: RationalU256, b: U256, c: U256) {
        assert_eq!((&a + &b).into_u256(), c);
        assert_eq!((a.clone() + b.clone()).into_u256(), c);
        assert_eq!((a.clone() + &b).into_u256(), c);
        assert_eq!((&a + b.clone()).into_u256(), c);
    }

    fn _test_mul(a: RationalU256, b: RationalU256, c: U256) {
        assert_eq!((&a * &b).into_u256(), c);
        assert_eq!((a.clone() * b.clone()).into_u256(), c);
        assert_eq!((a.clone() * &b).into_u256(), c);
        assert_eq!((&a * b.clone()).into_u256(), c);
    }

    fn _test_mul_u256(a: RationalU256, b: U256, c: U256) {
        assert_eq!((&a * &b).into_u256(), c);
        assert_eq!((a.clone() * b.clone()).into_u256(), c);
        assert_eq!((a.clone() * &b).into_u256(), c);
        assert_eq!((&a * b.clone()).into_u256(), c);
    }

    fn _test_div(a: RationalU256, b: RationalU256, c: U256) {
        assert_eq!((&a / &b).into_u256(), c);
        assert_eq!((a.clone() / b.clone()).into_u256(), c);
        assert_eq!((a.clone() / &b).into_u256(), c);
        assert_eq!((&a / b.clone()).into_u256(), c);
    }

    fn _test_div_u256(a: RationalU256, b: U256, c: U256) {
        assert_eq!((&a / &b).into_u256(), c);
        assert_eq!((a.clone() / b.clone()).into_u256(), c);
        assert_eq!((a.clone() / &b).into_u256(), c);
        assert_eq!((&a / b.clone()).into_u256(), c);
    }

    fn _test_sub(a: RationalU256, b: RationalU256, c: U256) {
        assert_eq!((&a - &b).into_u256(), c);
        assert_eq!((a.clone() - b.clone()).into_u256(), c);
        assert_eq!((a.clone() - &b).into_u256(), c);
        assert_eq!((&a - b.clone()).into_u256(), c);
    }

    fn _test_sub_u256(a: RationalU256, b: U256, c: U256) {
        assert_eq!((&a - &b).into_u256(), c);
        assert_eq!((a.clone() - b.clone()).into_u256(), c);
        assert_eq!((a.clone() - &b).into_u256(), c);
        assert_eq!((&a - b.clone()).into_u256(), c);
    }

    fn _test_saturating_sub(a: RationalU256, b: RationalU256, c: U256) {
        assert_eq!(a.saturating_sub(b).into_u256(), c);
    }

    fn _test_saturating_sub_u256(a: RationalU256, b: U256, c: U256) {
        assert_eq!(a.saturating_sub_u256(b).into_u256(), c);
    }

    proptest! {
        #[test]
        fn test_add(a in 0u32..10000, b in 1u32..10000, c in 0u32..10000, d in 1u32..10000) {
            // a/b + c/d = (a*d + b*c) / (b*d)
            let a = U256::from(a);
            let b = U256::from(b);
            let c = U256::from(c);
            let d = U256::from(d);
            let r = (&a * &d + &b * &c) / (&b * &d);
            let e = (&a + &b * &c) / &b;

            _test_add(
                RationalU256::new(a.clone(), b.clone()),
                RationalU256::new(c.clone(), d),
                r,
            );
            _test_add_u256(RationalU256::new(a.clone(), b.clone()), c, e);
        }
    }

    proptest! {
        #[test]
        fn test_mul(a in 0u32..10000, b in 1u32..10000, c in 0u32..10000, d in 1u32..10000) {
            // a/b * c/d = (a*c)/(b*d)
            let a = U256::from(a);
            let b = U256::from(b);
            let c = U256::from(c);
            let d = U256::from(d);
            let r = (&a * &c) / (&b * &d);
            let e = (&a * &c) / &b;

            _test_mul(
                RationalU256::new(a.clone(), b.clone()),
                RationalU256::new(c.clone(), d),
                r,
            );
            _test_mul_u256(RationalU256::new(a.clone(), b.clone()), c, e);
        }
    }

    proptest! {
        #[test]
        fn test_div(a in 0u32..10000, b in 1u32..10000, c in 0u32..10000, d in 1u32..10000) {
            // (a/b) / (c/d) = (a*d) / (b*c)
            let a = U256::from(a);
            let b = U256::from(b);
            let c = U256::from(c);
            let d = U256::from(d);
            let r = (&a * &d) / (&b * &c);
            let e = &a / (&b * &c);

            _test_div(
                RationalU256::new(a.clone(), b.clone()),
                RationalU256::new(c.clone(), d),
                r,
            );
            _test_div_u256(RationalU256::new(a.clone(), b.clone()), c, e);
        }
    }

    proptest! {
        #[test]
        fn test_sub(a in 0u32..10000, b in 1u32..10000, c in 0u32..10000, d in 1u32..10000) {
            // a/b - c/d = (a*d - b*c) / (b*d)
            let (_, overflowing1) = (a * d).overflowing_sub(b * c);
            let (_, overflowing2) = a.overflowing_sub(b * c);
            if !(overflowing1 || overflowing2) {
                let a = U256::from(a);
                let b = U256::from(b);
                let c = U256::from(c);
                let d = U256::from(d);
                let r = (&a * &d - &b * &c) / (&b * &d);
                let e = (&a - &b * &c) / &b;

                _test_sub(
                    RationalU256::new(a.clone(), b.clone()),
                    RationalU256::new(c.clone(), d),
                    r,
                );
                _test_sub_u256(RationalU256::new(a.clone(), b.clone()), c, e);
            }
        }
    }

    proptest! {
        #[test]
        fn test_saturating_sub(a in 0u32..10000, b in 1u32..10000, c in 0u32..10000, d in 1u32..10000) {
            // (a/b) / (c/d) = (a*d) / (b*c)
            let a = U256::from(a);
            let b = U256::from(b);
            let c = U256::from(c);
            let d = U256::from(d);
            let r = (&a * &d).saturating_sub(&(&b * &c)) / (&b * &d);
            let e = a.saturating_sub(&(&b * &c)) / &b;

            _test_saturating_sub(
                RationalU256::new(a.clone(), b.clone()),
                RationalU256::new(c.clone(), d),
                r,
            );
            _test_saturating_sub_u256(RationalU256::new(a.clone(), b.clone()), c, e);
        }
    }
}
