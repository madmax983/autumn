🛡️ Sentry: [test coverage improvement]

🎯 **Target**: Tested `fallback_404_handler` function in `autumn/src/middleware/error_page_filter.rs`.
💣 **Risk**: This function lacked any tests. If someone unknowingly modifies it, it could break our error handling capabilities and routing failovers.
🧪 **Strategy**: Added the `fallback_404_handler_creates_correct_error` test that invokes the handler with an unmatched URI and asserts the correct properties in the return value (correct status and message).
🔬 **Verification**: Ran `cargo test -p autumn-web --lib middleware::error_page_filter` directly testing the modified suite to ensure success.er stays stuck at 1 instead of decrementing.
