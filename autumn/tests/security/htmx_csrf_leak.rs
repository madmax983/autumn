#[tokio::test]
async fn eris_htmx_csrf_leak_is_blocked() {
    #[cfg(feature = "htmx")]
    {
        // HTMX CSRF JS must ensure it only sends the CSRF token to same-origin URLs
        // to prevent token leakage to attacker-controlled domains (e.g. via hx-post="https://attacker.com").
        let js = std::str::from_utf8(autumn_web::HTMX_CSRF_JS.as_bytes()).unwrap();

        assert!(
            js.contains("window.location.origin"),
            "VULNERABILITY: HTMX CSRF JavaScript lacks a same-origin check, leaking CSRF tokens to cross-origin targets!"
        );

        assert!(
            js.contains("isSameOrigin"),
            "VULNERABILITY: HTMX CSRF JavaScript lacks a same-origin check, leaking CSRF tokens to cross-origin targets!"
        );
    }
}
