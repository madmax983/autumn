use autumn_web::authorization::{Policy, PolicyRegistry};
use std::thread;
use autumn_web::authorization::BoxFuture;

struct DummyResource;
struct DummyPolicy;

impl Policy<DummyResource> for DummyPolicy {}

struct PanicPolicy;
impl Policy<DummyResource> for PanicPolicy {}

#[test]
#[should_panic(expected = "policy registry lock poisoned")]
fn authorization_registry_lock_poison() {
    let registry = PolicyRegistry::default();
    let reg_clone = registry.clone();

    // Spawn a thread that panics while holding the write lock.
    let _ = thread::spawn(move || {
        reg_clone.register_policy::<DummyResource, DummyPolicy>(DummyPolicy);
        // This second registration will panic inside the write lock
        reg_clone.register_policy::<DummyResource, PanicPolicy>(PanicPolicy);
    })
    .join();

    // Now this thread tries to read, which should panic because the lock is poisoned
    let _policy = registry.policy::<DummyResource>();
}
