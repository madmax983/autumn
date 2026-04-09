use std::sync::{Mutex, OnceLock};

/// RAII guard for safely mutating process-wide environment variables in tests.
///
/// Because Cargo runs tests concurrently within the same process by default,
/// mutating environment variables using `std::env::set_var` can cause undefined behavior
/// or flaky tests. This struct serializes access to environment mutation using a static,
/// cross-module Mutex lock.
pub struct EnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: Vec<(&'static str, Option<String>)>,
}

impl EnvGuard {
    /// Sets many environment variables while acquiring the global lock.
    ///
    /// # Panics
    /// Panics if the internal static mutex is poisoned.
    pub fn set_many(entries: &[(&'static str, Option<&str>)]) -> Self {
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let lock = ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("env mutex poisoned");

        let mut previous = Vec::with_capacity(entries.len());
        for (key, value) in entries {
            previous.push((*key, std::env::var(key).ok()));
            match value {
                Some(value) => {
                    // SAFETY: test-only helper serializes environment mutation with a process-wide mutex.
                    unsafe { std::env::set_var(key, value) };
                }
                None => {
                    // SAFETY: test-only helper serializes environment mutation with a process-wide mutex.
                    unsafe { std::env::remove_var(key) };
                }
            }
        }
        Self {
            _lock: lock,
            previous,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, previous) in self.previous.iter().rev() {
            if let Some(previous) = previous {
                // SAFETY: test-only helper serializes environment mutation with a process-wide mutex.
                unsafe { std::env::set_var(key, previous) };
            } else {
                // SAFETY: test-only helper serializes environment mutation with a process-wide mutex.
                unsafe { std::env::remove_var(key) };
            }
        }
    }
}
