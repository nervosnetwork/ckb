use crate::{
    core::{DepType, ScriptHashType},
    packed,
};

#[test]
fn test_script_hash_type() {
    let default = ScriptHashType::default();
    assert!(default == ScriptHashType::Data);
    let default_value: u8 = default.into();
    assert_eq!(default_value, 0);

    let max_value = 3u8;
    for v in 0..32 {
        let res = ScriptHashType::try_from(v);
        if v <= max_value {
            let value: u8 = res.unwrap().into();
            assert_eq!(value, v);
        } else {
            assert!(res.is_err());
        }
    }
}

#[test]
fn test_dep_type() {
    let default = DepType::default();
    assert!(default == DepType::Code);
    let default_value: u8 = default.into();
    assert_eq!(default_value, 0);

    let max_value = 1u8;
    for v in 0..32 {
        let b: packed::Byte = v.into();
        let res = DepType::try_from(b);
        if v <= max_value {
            let value: u8 = res.unwrap().into();
            assert_eq!(value, v);
        } else {
            assert!(res.is_err());
        }
    }
}
