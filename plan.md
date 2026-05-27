1. **Refactor `scrub_sql` in `autumn/src/db.rs`**
   - 🚮 **Smell:** The `scrub_sql` function in `autumn/src/db.rs` is a "God Function" with a complex `while let Some(c) = chars.next()` loop that spans hundreds of lines (lines ~336 to ~480+). It contains deep nesting and handles multiple distinct parsing concerns (E-strings, single-quoted strings, dollar-quoted strings, numeric literals, identifiers) all inline.
   - ✨ **Solution:** Extract the inline parsing logic for each token type into distinct, private helper functions: `consume_single_quoted_string`, `consume_dollar_quoted_string_or_param`, `consume_numeric_literal`, etc. The main loop will then delegate to these helpers. Guard clauses / early continues will be used to flatten the control flow where possible.
   - 🧼 **Benefit:** Dramastically improves clarity in a complex parsing routine by isolating the logic for each token type, making the main loop a clear state machine dispatcher rather than a monolith.
   - 🛡️ **Verification:** Tests passed. No logic changed.
2. **Complete pre-commit steps to ensure proper testing, verification, review, and reflection are done.**
3. **Submit the PR.**
