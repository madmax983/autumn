#[cfg(test)]
use std::sync::{Mutex, OnceLock};

#[cfg(test)]
pub struct EnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: Vec<(&'static str, Option<String>)>,
}

#[cfg(test)]
impl EnvGuard {
    /// Safely updates multiple environment variables using a process-wide mutex.
    ///
    /// Since Cargo runs tests in parallel within the same process, modifying
    /// environment variables via `std::env::set_var` can cause data races and
    /// Undefined Behavior. `EnvGuard` prevents this by acquiring a static mutex.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use autumn::test_utils::EnvGuard;
    ///
    /// let _guard = EnvGuard::set_many(&[
    ///     ("AUTUMN_ENV", Some("test")),
    ///     ("SOME_VAR", None),
    /// ]);
    /// ```
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

#[cfg(test)]
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
