use autumn_web::security::RateLimitConfig;
use autumn_web::security::RateLimitLayer;

#[test]
fn havoc_rate_limit_fuzz_bounds() {
    let cases = vec![
        (0.0, 0),
        (f64::NAN, 100),
        (f64::INFINITY, 10),
        (f64::NEG_INFINITY, 5),
        (-1.0, 5),
        (1e300, 100),
    ];

    for (rps, burst) in cases {
        let config = RateLimitConfig {
            enabled: true,
            requests_per_second: rps,
            burst,
            trust_forwarded_headers: true,
        };

        // If math is unsafe this would panic during from_config or subsequent limit checks.
        let _layer = RateLimitLayer::from_config(&config);
    }
}
