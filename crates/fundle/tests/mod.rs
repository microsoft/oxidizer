use trybuild::TestCases;

#[test]
fn proc() {
    let t = TestCases::new();

    t.pass("tests/proc/bundle_empty.rs");
    t.compile_fail("tests/proc/bundle_enum.rs");
    t.pass("tests/proc/bundle_forward.rs");
    t.pass("tests/proc/bundle_simple.rs");
    t.compile_fail("tests/proc/bundle_tupled.rs");
    t.pass("tests/proc/deps_simple.rs");
    t.pass("tests/proc/newtype_simple.rs");

}
