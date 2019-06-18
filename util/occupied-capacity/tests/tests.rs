use occupied_capacity::{capacity_bytes, Capacity, HasOccupiedCapacity, OccupiedCapacity};

#[derive(HasOccupiedCapacity)]
struct StructA {
    f1: u32,
    f2: Vec<u8>,
}

#[derive(HasOccupiedCapacity)]
struct StructB(u32, Vec<u8>);

#[derive(HasOccupiedCapacity)]
struct StructA1 {
    f1: u32,
    f2: Vec<u8>,
    #[free_capacity]
    _f3: Vec<u8>,
}

#[derive(HasOccupiedCapacity)]
struct StructB1(u32, Vec<u8>, #[free_capacity] Vec<u8>);

#[test]
pub fn capacity() {
    let v = vec![0u8; 7];
    assert_eq!(v.occupied_capacity().unwrap(), capacity_bytes!(11));
    let u = 0u32;
    assert_eq!(u.occupied_capacity().unwrap(), capacity_bytes!(4));
    let a = StructA {
        f1: 1,
        f2: v.clone(),
    };
    assert_eq!(a.occupied_capacity().unwrap(), capacity_bytes!(15));
    let a1 = StructA1 {
        f1: 2,
        f2: v.clone(),
        _f3: v.clone(),
    };
    assert_eq!(a1.occupied_capacity().unwrap(), capacity_bytes!(15));
    let b = StructB(3, v.clone());
    assert_eq!(b.occupied_capacity().unwrap(), capacity_bytes!(15));
    let b1 = StructB1(4, v.clone(), v.clone());
    assert_eq!(b1.occupied_capacity().unwrap(), capacity_bytes!(15));
}
