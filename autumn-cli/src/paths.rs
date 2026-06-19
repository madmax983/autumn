//! Platform-appropriate runtime/data/log directories for `autumn serve`.
//!
//! Daemon state (PID lockfile, Unix socket, address-discovery file), the
//! managed-Postgres data dir, and log files must live under per-OS standard
//! locations (XDG on Linux, `~/Library/Application Support` on macOS,
//! `%APPDATA%`/`%LOCALAPPDATA%` on Windows) — never the current working
//! directory, `/etc`, or `/tmp` (a predictable path under `/tmp` is a symlink
//! hazard for the `0600` socket).
//!
//! [`RuntimePaths::resolve`] performs the real per-OS resolution and honours
//! the `AUTUMN_RUNTIME_DIR` override used by integration tests.
//! [`RuntimePaths::from_base`] is the pure, fs-free seam the unit tests drive.

use std::path::{Path, PathBuf};

/// Environment variable that overrides the runtime base directory. When set,
/// all daemon paths are rooted at `$AUTUMN_RUNTIME_DIR/<project>`. Primarily
/// used by tests to point the daemon at a `tempdir`.
pub const RUNTIME_DIR_ENV: &str = "AUTUMN_RUNTIME_DIR";

/// Qualifier/organization/application triple used to derive platform dirs.
const QUALIFIER: &str = "dev";
const ORGANIZATION: &str = "autumn";

/// Errors resolving platform directories.
#[derive(Debug, thiserror::Error)]
pub enum PathsError {
    /// No home/standard directory could be determined for the current user.
    #[error("could not determine a platform data directory for this user")]
    NoPlatformDir,
}

/// Resolved set of base directories for a single project's daemon.
///
/// Accessors append fixed leaf names so the layout is identical whether the
/// paths were resolved from the OS or constructed from a test base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaths {
    /// PID lockfile, Unix socket, and address-discovery file live here.
    runtime: PathBuf,
    /// Daemon log files are written here.
    logs: PathBuf,
}

impl RuntimePaths {
    /// Resolve platform directories for `project`.
    ///
    /// Honours `AUTUMN_RUNTIME_DIR` (rooting everything at
    /// `$AUTUMN_RUNTIME_DIR/<project>`); otherwise uses the per-OS standard
    /// locations via the `directories` crate.
    ///
    /// # Errors
    ///
    /// Returns [`PathsError::NoPlatformDir`] when no standard directory can be
    /// determined for the current user.
    pub fn resolve(project: &str) -> Result<Self, PathsError> {
        if let Some(base) = std::env::var_os(RUNTIME_DIR_ENV) {
            return Ok(Self::from_base(Path::new(&base), project));
        }

        let dirs = directories::ProjectDirs::from(QUALIFIER, ORGANIZATION, project)
            .ok_or(PathsError::NoPlatformDir)?;

        // `runtime_dir()` is `Some` only on Linux when `XDG_RUNTIME_DIR` is set;
        // fall back to a `run/` subdir of the data dir on macOS/Windows and on
        // headless Linux so the daemon never writes to cwd or `/tmp`.
        let runtime = dirs
            .runtime_dir()
            .map_or_else(|| dirs.data_dir().join("run"), Path::to_path_buf);
        // Prefer the XDG state dir for logs on Linux; otherwise nest under data.
        let logs = dirs
            .state_dir()
            .map_or_else(|| dirs.data_dir().join("logs"), |s| s.join("logs"));

        Ok(Self { runtime, logs })
    }

    /// Construct paths rooted at `base/<project>` without touching the
    /// filesystem or environment. This is the pure unit-test seam.
    #[must_use]
    pub fn from_base(base: &Path, project: &str) -> Self {
        let root = base.join(project);
        Self {
            runtime: root.clone(),
            logs: root,
        }
    }

    /// PID lockfile path (`<runtime>/serve.pid`).
    #[must_use]
    pub fn pid_file(&self) -> PathBuf {
        self.runtime.join("serve.pid")
    }

    /// Unix domain socket path (`<runtime>/serve.sock`).
    #[must_use]
    pub fn socket_file(&self) -> PathBuf {
        self.runtime.join("serve.sock")
    }

    /// Address-discovery file path (`<runtime>/serve.addr`).
    #[must_use]
    pub fn addr_file(&self) -> PathBuf {
        self.runtime.join("serve.addr")
    }

    /// Daemon log file path (`<logs>/serve.log`).
    #[must_use]
    pub fn log_file(&self) -> PathBuf {
        self.logs.join("serve.log")
    }

    /// Create the runtime and log directories if they do not exist.
    ///
    /// # Errors
    ///
    /// Returns the first I/O error encountered creating a directory.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.runtime)?;
        std::fs::create_dir_all(&self.logs)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_from_base_layout() {
        let paths = RuntimePaths::from_base(Path::new("/var/run"), "demo");
        assert_eq!(paths.pid_file(), Path::new("/var/run/demo/serve.pid"));
        assert_eq!(paths.socket_file(), Path::new("/var/run/demo/serve.sock"));
        assert_eq!(paths.addr_file(), Path::new("/var/run/demo/serve.addr"));
        assert_eq!(paths.log_file(), Path::new("/var/run/demo/serve.log"));
    }

    #[test]
    fn paths_socket_absolute_under_base() {
        let paths = RuntimePaths::from_base(Path::new("/tmp/xdg-base"), "svc");
        let sock = paths.socket_file();
        assert!(sock.is_absolute());
        assert!(sock.starts_with("/tmp/xdg-base"));
    }

    #[test]
    fn resolve_honors_env_override() {
        let dir = tempfile::tempdir().expect("tempdir");
        temp_env::with_var(RUNTIME_DIR_ENV, Some(dir.path()), || {
            let paths = RuntimePaths::resolve("demo").expect("resolve with override");
            assert_eq!(paths.pid_file(), dir.path().join("demo").join("serve.pid"));
            assert!(paths.socket_file().starts_with(dir.path()));
        });
    }
}
