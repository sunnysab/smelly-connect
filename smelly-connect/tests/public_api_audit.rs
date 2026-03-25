#[test]
fn legacy_internal_api_is_not_public() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/legacy_public_api.rs");
}
