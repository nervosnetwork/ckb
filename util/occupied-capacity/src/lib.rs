//! Data structure measurement.

use numext_fixed_hash::H256;
pub use occupied_capacity_derive::*;
use std::mem;

pub trait OccupiedCapacity {
    /// Measure the occupied capacity of structures
    fn occupied_capacity(&self) -> usize;
}

impl<T: OccupiedCapacity> OccupiedCapacity for [T] {
    fn occupied_capacity(&self) -> usize {
        self.iter().map(OccupiedCapacity::occupied_capacity).sum()
    }
}

impl<T: OccupiedCapacity> OccupiedCapacity for Option<T> {
    fn occupied_capacity(&self) -> usize {
        self.as_ref().map_or(0, OccupiedCapacity::occupied_capacity)
    }
}

impl<T: OccupiedCapacity> OccupiedCapacity for Vec<T> {
    fn occupied_capacity(&self) -> usize {
        self.iter().map(OccupiedCapacity::occupied_capacity).sum()
    }
}

impl OccupiedCapacity for H256 {
    fn occupied_capacity(&self) -> usize {
        H256::size_of()
    }
}

macro_rules! impl_mem_size_of {
    ($type:ty) => {
        impl OccupiedCapacity for $type {
            fn occupied_capacity(&self) -> usize {
                mem::size_of::<$type>()
            }
        }
    };
}

// https://github.com/rust-lang/rust/issues/31844#
// Currently, specialization haven't implemented
impl OccupiedCapacity for Vec<u8> {
    fn occupied_capacity(&self) -> usize {
        self.len()
    }
}

impl OccupiedCapacity for [u8] {
    fn occupied_capacity(&self) -> usize {
        self.len()
    }
}

// conflicting implementation for `std::vec::Vec<u8>`
// impl_mem_size_of!(u8);
impl_mem_size_of!(u32);
impl_mem_size_of!(u64);
impl_mem_size_of!(bool);
impl_mem_size_of!(());
