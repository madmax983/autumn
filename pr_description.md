🛡️ Sentry: [test coverage improvement]

🎯 **Target:** Added explicit test coverage for the `inject_snippet` function in `autumn/src/middleware/dev.rs` to address missing edge cases.

💣 **Risk:** The `inject_snippet` function manipulates raw string bytes to insert a live reload script into HTML responses. It was previously only tested for typical well-formed HTML shells and multiple closing body tags. Without comprehensive tests, future refactoring could inadvertently break functionality on malformed tags, empty bodies, or specific casing rules, leading to missed live-reloads or corrupted HTML output.

🧪 **Strategy:** Added a new test assertion block in `inject_snippet_edge_cases` to explicitly cover:
- Empty response bodies (ensuring no panic and returning empty bytes).
- Responses matching exactly `</body>` with no other content.
- Case sensitivity behaviors (verifying that uppercase `<HTML><BODY>` tags are ignored as intended by the current exact-match logic).
- Malformed/spaced tags like `<html><body >` (verifying they fallback appropriately to other tags or ignore).

🔭 **Verification:** Ran `cargo test -p autumn-web -- middleware::dev::tests::` and `cargo check --all-targets --all-features` to ensure tests passed and no regressions were introduced.
