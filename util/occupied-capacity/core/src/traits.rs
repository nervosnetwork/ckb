use numext_fixed_hash::H256;
use std::mem;

use crate::{Capacity, Result};

pub trait OccupiedCapacity {
    /// Measure the occupied capacity of structures
    fn occupied_capacity(&self) -> Result<Capacity>;
}

impl<T: OccupiedCapacity> OccupiedCapacity for [T] {
    fn occupied_capacity(&self) -> Result<Capacity> {
        self.iter()
            .map(OccupiedCapacity::occupied_capacity)
            .try_fold(Capacity::zero(), |acc, rhs| {
                rhs.and_then(|x| acc.safe_add(x))
            })
    }
}

impl<T: OccupiedCapacity> OccupiedCapacity for Option<T> {
    fn occupied_capacity(&self) -> Result<Capacity> {
        self.as_ref()
            .map_or(Ok(Capacity::zero()), OccupiedCapacity::occupied_capacity)
    }
}

impl<T: OccupiedCapacity> OccupiedCapacity for Vec<T> {
    fn occupied_capacity(&self) -> Result<Capacity> {
        self.iter()
            .map(OccupiedCapacity::occupied_capacity)
            .try_fold(Capacity::zero(), |acc, rhs| {
                rhs.and_then(|x| acc.safe_add(x))
            })
    }
}

impl OccupiedCapacity for H256 {
    fn occupied_capacity(&self) -> Result<Capacity> {
        Capacity::bytes(H256::size_of())
    }
}

macro_rules! impl_mem_size_of {
    ($type:ty) => {
        impl OccupiedCapacity for $type {
            fn occupied_capacity(&self) -> Result<Capacity> {
                Capacity::bytes(mem::size_of::<$type>())
            }
        }
    };
}

// https://github.com/rust-lang/rust/issues/31844#
// Currently, specialization haven't implemented
impl OccupiedCapacity for Vec<u8> {
    fn occupied_capacity(&self) -> Result<Capacity> {
        Capacity::bytes(self.len())
    }
}

impl OccupiedCapacity for [u8] {
    fn occupied_capacity(&self) -> Result<Capacity> {
        Capacity::bytes(self.len())
    }
}

impl OccupiedCapacity for bytes::Bytes {
    fn occupied_capacity(&self) -> Result<Capacity> {
        Capacity::bytes(self.len())
    }
}

// conflicting implementation for `std::vec::Vec<u8>`
// impl_mem_size_of!(u8);
impl_mem_size_of!(u32);
impl_mem_size_of!(u64);
impl_mem_size_of!(bool);
impl_mem_size_of!(());
