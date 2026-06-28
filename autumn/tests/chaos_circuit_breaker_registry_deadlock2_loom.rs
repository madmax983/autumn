#![cfg(feature = "ws")]
use autumn_web::circuit_breaker::{CircuitBreakerPolicy, CircuitBreakerRegistry};
use loom::thread;
use std::sync::Arc;

#[test]
fn circuit_breaker_registry_deadlock_2() {
    loom::model(|| {
        let registry = Arc::new(CircuitBreakerRegistry::new());
        let config = CircuitBreakerPolicy::default();
        let name = "test_circuit";
        let breaker = registry.get_or_create_with_config(name, config.clone());

        let r1 = registry.clone();
        let c1 = config.clone();
        let t1 = thread::spawn(move || {
            r1.get_or_create_with_config(name, c1.clone());
        });

        let b1 = breaker.clone();
        let t2 = thread::spawn(move || {
            b1.failure_ratio();
        });

        t1.join().unwrap();
        t2.join().unwrap();
    });
}
