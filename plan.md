We have found some good refactoring opportunities to improve idiomatic Rust and flatten the pyramid of doom without changing logic.

1. **Smell:** In `autumn-cli/src/main.rs`, multiple tests use `match cli.command` with a panic arm to assert the correct variant, instead of using `let ... else` guard clauses. E.g., `let Commands::New { name } = cli.command else { panic!("expected New command"); };`.
2. **Solution:** Refactor the tests in `autumn-cli/src/main.rs` to use `let ... else` statements to extract variants instead of `match`.
3. **Benefit:** Reduces nesting in the test functions and improves readability by making the assertions un-nested.

1. **Smell:** In `autumn/src/config.rs`, the `deep_merge_with_depth` function has deeply nested `if` and `if let` blocks.
2. **Solution:** Flatten `deep_merge_with_depth` by using early returns and `let ... else` and `if let Some` properly.
3. **Benefit:** Much more readable and reduces cognitive load by eliminating "pyramid of doom".

1. **Smell:** In `autumn-cli/src/monitor.rs`, `let client = match client { Ok(c) => c, Err(e) => { ... } };` could be flattened with `let Ok(client) = client else { ... return; };`
2. **Solution:** Refactor `monitor.rs` to use `let Ok(client) = client else` for the `client` check and any similar ones.
3. **Benefit:** Uses idiomatic Rust early return guard clauses.

I will focus on refactoring `autumn/src/config.rs`'s `deep_merge_with_depth` as it's a perfect example of Forge's philosophy ("Nesting is the mind-killer").

I will also refactor the tests in `autumn-cli/src/main.rs` to use guard clauses.

Let's check `autumn-cli/src/main.rs` for `parse_build_with_package` and others.
Let's check `autumn/src/config.rs`.
