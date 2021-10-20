use crate::convert_compatible_crate_name;

#[test]
fn test_convert_compatible_crate_name() {
    let spec = "info,a-b=trace,c-d_e-f=warn,g-h-i=debug,jkl=trace/*[0-9]";
    let expected = "info,a-b=trace,a_b=trace,c-d_e-f=warn,c_d_e_f=warn,g-h-i=debug,g_h_i=debug,jkl=trace/*[0-9]";
    let result = convert_compatible_crate_name(&spec);
    assert_eq!(&result, &expected);
    let spec = "info,a-b=trace,c-d_e-f=warn,g-h-i=debug,jkl=trace";
    let expected =
        "info,a-b=trace,a_b=trace,c-d_e-f=warn,c_d_e_f=warn,g-h-i=debug,g_h_i=debug,jkl=trace";
    let result = convert_compatible_crate_name(&spec);
    assert_eq!(&result, &expected);
    let spec = "info/*[0-9]";
    let expected = "info/*[0-9]";
    let result = convert_compatible_crate_name(&spec);
    assert_eq!(&result, &expected);
    let spec = "info";
    let expected = "info";
    let result = convert_compatible_crate_name(&spec);
    assert_eq!(&result, &expected);
}
