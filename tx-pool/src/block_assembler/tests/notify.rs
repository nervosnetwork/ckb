use super::NotifyScriptRunner;

#[test]
fn one_configured_script_owns_at_most_one_live_process_slot() {
    let runner = NotifyScriptRunner::new(&["script-a".to_owned(), "script-b".to_owned()]);
    let [first, second] = runner.scripts.as_ref() else {
        panic!("fixture must contain exactly two script slots");
    };

    let first_permit = first.try_claim().expect("first slot is initially free");
    assert!(first.try_claim().is_none());
    assert!(second.try_claim().is_some());

    drop(first_permit);
    assert!(first.try_claim().is_some());
}
