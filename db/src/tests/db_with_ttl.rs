use crate::DBWithTTL;

#[test]
fn test_open_db_with_ttl() {
    let tmp_dir = tempfile::Builder::new()
        .prefix("test_open_db_with_ttl")
        .tempdir()
        .unwrap();

    let db = DBWithTTL::open_cf(&tmp_dir, vec!["1"], 100);
    assert!(db.is_ok(), "{db:?}");
    let mut db = db.unwrap();
    assert!(db.has_cf("1"));

    for i in 0..1000u64 {
        db.put("1", i.to_le_bytes(), [2]).unwrap();
        assert_eq!(
            db.get_pinned("1", &i.to_le_bytes())
                .unwrap()
                .unwrap()
                .as_ref(),
            &[2]
        );
    }

    let estimate_num_keys = db.estimate_num_keys_cf("1").unwrap();
    assert!(estimate_num_keys.is_some());

    db.drop_cf("1").unwrap();
    assert!(!db.has_cf("1"));
    let error = db.get_pinned("1", &[1]).unwrap_err();
    assert!(error.to_string().contains("column 1 not found"), "{error}");

    db.create_cf_with_ttl("1", 50).unwrap();
    assert!(db.has_cf("1"));
    assert!(db.get_pinned("1", &[1]).unwrap().is_none());
    db.put("1", [1], [3]).unwrap();
    assert_eq!(db.get_pinned("1", &[1]).unwrap().unwrap().as_ref(), &[3]);
}
