// RED PHASE: this file intentionally fails to compile until
// `autumn-web` with `features = ["test-support"]` is added to
// [dev-dependencies] in examples/todo-app/Cargo.toml.
//
// TestDb is only exported when both the `db` and `test-support` Cargo
// features are active (see autumn/src/test.rs). Without the feature,
// the import below produces:
//
//   error[E0432]: unresolved import `autumn_web::test::TestDb`

use autumn_web::test::TestDb;

#[allow(unused)]
fn _assert_test_db_importable(_: &TestDb) {}
