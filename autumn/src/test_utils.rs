#[cfg(test)]
use std::sync::{Mutex, OnceLock};
#[cfg(test)]
use std::{ffi::OsStr, ffi::OsString};

#[cfg(test)]
pub struct EnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: Vec<(&'static str, Option<OsString>)>,
}

#[cfg(test)]
impl EnvGuard {
    pub fn set_many(entries: &[(&'static str, Option<&str>)]) -> Self {
        let entries: Vec<(&'static str, Option<&OsStr>)> = entries
            .iter()
            .map(|(key, value)| (*key, value.map(OsStr::new)))
            .collect();
        Self::set_many_os(&entries)
    }

    pub fn set_many_os(entries: &[(&'static str, Option<&OsStr>)]) -> Self {
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let lock = ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("env mutex poisoned");
        let mut previous = Vec::with_capacity(entries.len());
        for (key, value) in entries {
            previous.push((*key, std::env::var_os(key)));
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
