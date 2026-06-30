#[tokio::test]
async fn eris_route_traversal_compilation_fails() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/security/compile_fail/route_traversal.rs");
}
