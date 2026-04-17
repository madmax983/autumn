🧪 Sentry: [test coverage improvement]

🎯 **Target**:
Added test coverage for the JSON API fallback scenario inside the error exception filter logic. Specifically, verified that when an unmatched route hits the `fallback_404_handler` while expecting `application/json`, it appropriately bypasses HTML generation and returns the correctly formatted JSON error object.

💣 **Risk**:
Without this test, any unintentional behavioral change to the `ExceptionFilterLayer` or its reliance on `accepts_html` could have caused JSON API endpoints or 404 fallbacks for REST clients to start responding with an unexpected styled HTML payload.

🧪 **Strategy**:
Appended a test `json_api_fallback_gets_json_errors` that fires a request at an unmapped endpoint (`/nonexistent`) configured to solicit a JSON response. It validates the output payload, HTTP status code (`404`), and standard error-object structure (`{"error": {"status": 404, "message": "No route matches /nonexistent"}}`).

🔭 **Verification**:
Executed `cargo test` verifying the new suite successfully parses and operates as designed without causing flakiness or regressions. Verified clean output from `cargo clippy`.
