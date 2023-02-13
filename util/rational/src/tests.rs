use crate::*;
use numext_fixed_uint::U256;
use proptest::arbitrary::Arbitrary;
use proptest::prelude::*;
use proptest::strategy::{NewTree, Strategy, ValueTree};
use proptest::test_runner::{TestRng, TestRunner};

#[derive(Clone, PartialEq, PartialOrd, Eq, Ord)]
pub struct U256LeBytes {
    pub inner: [u8; 32],
}

impl U256LeBytes {
    pub fn nonzero(rng: &mut TestRng) -> Self {
        let mut ret = U256LeBytes { inner: [0u8; 32] };
        rng.fill_bytes(&mut ret.inner[..15]);
        'outer: loop {
            for unit in &ret.inner[..] {
                if *unit != 0 {
                    break 'outer;
                }
            }
            rng.fill_bytes(&mut ret.inner[..15]);
        }
        ret
    }

    fn highest_nonzero_bytes(&self) -> Option<usize> {
        let mut ret: Option<usize> = None;
        for i in 0..32 {
            if self.inner[31 - i] != 0 {
                ret = Some(31 - i);
                break;
            }
        }
        ret
    }

    fn shrink(&mut self) -> bool {
        if let Some(hi) = self.highest_nonzero_bytes() {
            self.inner[hi] >>= 1;
            true
        } else {
            false
        }
    }
}

impl ::std::fmt::Debug for U256LeBytes {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        write!(f, "0x")?;
        for i in self.inner.iter().rev() {
            write!(f, "{i:02x}")?;
        }
        write!(f, "")
    }
}

pub struct U256LeBytesValueTree {
    orig: U256LeBytes,
    curr: U256LeBytes,
    shrink_times: usize,
}

impl U256LeBytesValueTree {
    pub fn new(runner: &mut TestRunner) -> Self {
        let rng = runner.rng();
        let orig = U256LeBytes::nonzero(rng);
        let curr = orig.clone();
        let shrink_times = 0;
        Self {
            orig,
            curr,
            shrink_times,
        }
    }
}

impl ValueTree for U256LeBytesValueTree {
    type Value = U256LeBytes;

    fn current(&self) -> Self::Value {
        self.curr.clone()
    }

    fn simplify(&mut self) -> bool {
        if self.curr.shrink() {
            self.shrink_times += 1;
            true
        } else {
            false
        }
    }

    fn complicate(&mut self) -> bool {
        if self.shrink_times > 0 {
            self.shrink_times -= 1;
            let mut prev = self.orig.clone();
            let mut times = self.shrink_times;
            while times > 0 {
                prev.shrink();
                times -= 1;
            }
            self.curr = prev;
            true
        } else {
            false
        }
    }
}

#[derive(Debug)]
pub struct U256LeBytesStrategy;

impl Strategy for U256LeBytesStrategy {
    type Tree = U256LeBytesValueTree;
    type Value = U256LeBytes;

    fn new_tree(&self, runner: &mut TestRunner) -> NewTree<Self> {
        let tree = Self::Tree::new(runner);
        Ok(tree)
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct U256LeBytesParameters;

impl Arbitrary for U256LeBytes {
    type Parameters = U256LeBytesParameters;
    type Strategy = U256LeBytesStrategy;
    fn arbitrary() -> Self::Strategy {
        U256LeBytesStrategy
    }
    fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
        U256LeBytesStrategy
    }
}

impl<'a> ::std::convert::From<&'a U256LeBytes> for U256 {
    fn from(bytes: &U256LeBytes) -> Self {
        U256::from_little_endian(&bytes.inner).expect("U256LeBytes convert")
    }
}

impl ::std::convert::From<U256LeBytes> for U256 {
    fn from(bytes: U256LeBytes) -> Self {
        U256::from_little_endian(&bytes.inner).expect("U256LeBytes convert")
    }
}

fn _test_add(a: RationalU256, b: RationalU256, c: U256) {
    assert_eq!((&a + &b).into_u256(), c);
    assert_eq!((a.clone() + b.clone()).into_u256(), c);
    assert_eq!((a.clone() + &b).into_u256(), c);
    assert_eq!((&a + b).into_u256(), c);
}

fn _test_add_u256(a: RationalU256, b: U256, c: U256) {
    assert_eq!((&a + &b).into_u256(), c);
    assert_eq!((a.clone() + b.clone()).into_u256(), c);
    assert_eq!((a.clone() + &b).into_u256(), c);
    assert_eq!((&a + b).into_u256(), c);
}

fn _test_mul(a: RationalU256, b: RationalU256, c: U256) {
    assert_eq!((&a * &b).into_u256(), c);
    assert_eq!((a.clone() * b.clone()).into_u256(), c);
    assert_eq!((a.clone() * &b).into_u256(), c);
    assert_eq!((&a * b).into_u256(), c);
}

fn _test_mul_u256(a: RationalU256, b: U256, c: U256) {
    assert_eq!((&a * &b).into_u256(), c);
    assert_eq!((a.clone() * b.clone()).into_u256(), c);
    assert_eq!((a.clone() * &b).into_u256(), c);
    assert_eq!((&a * b).into_u256(), c);
}

fn _test_div(a: RationalU256, b: RationalU256, c: U256) {
    assert_eq!((&a / &b).into_u256(), c);
    assert_eq!((a.clone() / b.clone()).into_u256(), c);
    assert_eq!((a.clone() / &b).into_u256(), c);
    assert_eq!((&a / b).into_u256(), c);
}

fn _test_div_u256(a: RationalU256, b: U256, c: U256) {
    assert_eq!((&a / &b).into_u256(), c);
    assert_eq!((a.clone() / b.clone()).into_u256(), c);
    assert_eq!((a.clone() / &b).into_u256(), c);
    assert_eq!((&a / b).into_u256(), c);
}

fn _test_sub(a: RationalU256, b: RationalU256, c: U256) {
    assert_eq!((&a - &b).into_u256(), c);
    assert_eq!((a.clone() - b.clone()).into_u256(), c);
    assert_eq!((a.clone() - &b).into_u256(), c);
    assert_eq!((&a - b).into_u256(), c);
}

fn _test_sub_u256(a: RationalU256, b: U256, c: U256) {
    assert_eq!((&a - &b).into_u256(), c);
    assert_eq!((a.clone() - b.clone()).into_u256(), c);
    assert_eq!((a.clone() - &b).into_u256(), c);
    assert_eq!((&a - b).into_u256(), c);
}

fn _test_saturating_sub(a: RationalU256, b: RationalU256, c: U256) {
    assert_eq!(a.saturating_sub(b).into_u256(), c);
}

fn _test_saturating_sub_u256(a: RationalU256, b: U256, c: U256) {
    assert_eq!(a.saturating_sub_u256(b).into_u256(), c);
}

proptest! {
    #[test]
    fn test_add(a in any::<U256LeBytes>(), b in any::<U256LeBytes>(), c in any::<U256LeBytes>(), d in any::<U256LeBytes>()) {
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
        _test_add_u256(RationalU256::new(a, b), c, e);
    }
}

proptest! {
    #[test]
    fn test_mul(a in any::<U256LeBytes>(), b in any::<U256LeBytes>(), c in any::<U256LeBytes>(), d in any::<U256LeBytes>()) {
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
        _test_mul_u256(RationalU256::new(a, b), c, e);
    }
}

proptest! {
    #[test]
    fn test_div(a in any::<U256LeBytes>(), b in any::<U256LeBytes>(), c in any::<U256LeBytes>(), d in any::<U256LeBytes>()) {
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
        _test_div_u256(RationalU256::new(a, b), c, e);
    }
}

proptest! {
    #[test]
    fn test_sub(a in any::<U256LeBytes>(), b in any::<U256LeBytes>(), c in any::<U256LeBytes>(), d in any::<U256LeBytes>()) {
        let a = U256::from(a);
        let b = U256::from(b);
        let c = U256::from(c);
        let d = U256::from(d);
        // a/b - c/d = (a*d - b*c) / (b*d)
        let (_, overflowing1) = (&a * &d).overflowing_sub(&(&b * &c));
        let (_, overflowing2) = a.overflowing_sub(&(&b * &c));
        if !(overflowing1 || overflowing2) {
            let r = (&a * &d - &b * &c) / (&b * &d);
            let e = (&a - &b * &c) / &b;
            _test_sub(
                RationalU256::new(a.clone(), b.clone()),
                RationalU256::new(c.clone(), d),
                r,
            );
            _test_sub_u256(RationalU256::new(a, b), c, e);
        }
    }
}

proptest! {
    #[test]
    fn test_saturating_sub(a in any::<U256LeBytes>(), b in any::<U256LeBytes>(), c in any::<U256LeBytes>(), d in any::<U256LeBytes>()) {
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
        _test_saturating_sub_u256(RationalU256::new(a, b), c, e);
    }
}
