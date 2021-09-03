use crate::pool::OutputsValidator;

#[test]
fn test_outputs_validator_json_display() {
    assert_eq!(
        "well_known_scripts_only",
        OutputsValidator::WellKnownScriptsOnly.json_display()
    );
    assert_eq!("passthrough", OutputsValidator::Passthrough.json_display());
}
