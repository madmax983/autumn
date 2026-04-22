use proptest::prelude::*;
use std::time::{Instant, Duration};

proptest! {
    #[test]
    fn test_rate_limit_havoc(
        burst in 1u32..10000,
        requests_per_second in 0.1f64..10000.0,
        elapsed_ms in 0u64..1000000
    ) {
        use super::config::RateLimitConfig;
        use super::rate_limit::Limiter;
        let config = RateLimitConfig {
            enabled: true,
            requests_per_second,
            burst,
            trust_forwarded_headers: true,
        };
        let limiter = Limiter::from_config(&config);

        let t0 = Instant::now();
        limiter.decide("test", t0);

        let t1 = t0 + Duration::from_millis(elapsed_ms);
        limiter.decide("test", t1);
    }
}
