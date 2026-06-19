//! Shared child-process and PID-file helpers for `autumn dev` and
//! `autumn serve`.
//!
//! Centralizes the SIGTERM-then-force-kill shutdown sequence (previously inline
//! in `dev.rs`) and adds the PID/lockfile primitives the daemon lifecycle needs:
//! liveness probing, atomic lock acquisition with stale-PID reclamation, and
//! bounded waits for a process to exit.

use std::path::Path;
#[cfg(unix)]
use std::process::Child;
use std::time::Duration;

/// What an existing PID lockfile tells us about the previous owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PidState {
    /// No usable PID recorded (missing or unparseable) — safe to claim.
    Free,
    /// A PID was recorded but that process is gone — stale, safe to reclaim.
    Stale,
    /// A PID was recorded and that process is alive — the daemon is running.
    Alive(u32),
}

/// Classify an existing lockfile from its parsed PID and a liveness check.
///
/// Pure decision function (no syscalls/fs) so the lock policy is unit-testable.
#[must_use]
pub const fn classify(recorded_pid: Option<u32>, alive: bool) -> PidState {
    match recorded_pid {
        Some(pid) if alive => PidState::Alive(pid),
        Some(_) => PidState::Stale,
        None => PidState::Free,
    }
}

/// Why acquiring the PID lockfile failed.
#[derive(Debug)]
pub enum AcquireError {
    /// A live daemon already holds the lock (its PID).
    AlreadyRunning(u32),
    /// An I/O error occurred manipulating the lockfile.
    Io(std::io::Error),
}

impl std::fmt::Display for AcquireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyRunning(pid) => {
                write!(f, "a daemon is already running (pid {pid})")
            }
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for AcquireError {}

/// Read and parse the PID recorded in a lockfile, if any.
#[must_use]
pub fn read_pidfile(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path)
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()
}

/// Atomically acquire the PID lockfile for `pid`, reclaiming a stale lock left
/// by a crashed previous run.
///
/// Creates the file with `O_EXCL`. If it already exists, the recorded PID is
/// probed: a live owner yields [`AcquireError::AlreadyRunning`] (the caller
/// should refuse to start); a dead owner is reclaimed and creation retried once.
///
/// # Errors
///
/// Returns [`AcquireError::AlreadyRunning`] if a live daemon holds the lock, or
/// [`AcquireError::Io`] on filesystem errors.
pub fn acquire_pidfile(path: &Path, pid: u32) -> Result<(), AcquireError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(AcquireError::Io)?;
    }
    for attempt in 0..2 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(mut file) => {
                use std::io::Write;
                return file
                    .write_all(format!("{pid}\n").as_bytes())
                    .map_err(AcquireError::Io);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let recorded = read_pidfile(path);
                let alive = recorded.is_some_and(is_process_alive);
                match classify(recorded, alive) {
                    PidState::Alive(existing) => {
                        return Err(AcquireError::AlreadyRunning(existing));
                    }
                    // Stale or unparseable: reclaim and retry the exclusive create.
                    // A concurrent deletion (NotFound) is fine — the next
                    // iteration's `create_new` will win.
                    PidState::Stale | PidState::Free if attempt == 0 => {
                        if let Err(err) = std::fs::remove_file(path)
                            && err.kind() != std::io::ErrorKind::NotFound
                        {
                            return Err(AcquireError::Io(err));
                        }
                    }
                    _ => return Err(AcquireError::Io(e)),
                }
            }
            Err(e) => return Err(AcquireError::Io(e)),
        }
    }
    Err(AcquireError::Io(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not acquire pid lockfile after reclaiming stale lock",
    )))
}

/// Validate a raw PID for use with `kill(2)`: positive and representable.
#[cfg(unix)]
#[must_use]
pub fn validate_pid_for_kill(pid: u32) -> Option<libc::pid_t> {
    let cast_pid = pid.try_into().ok()?;
    if cast_pid > 0 { Some(cast_pid) } else { None }
}

/// Whether a process with `pid` is currently alive.
///
/// Uses `kill(pid, 0)`: success or `EPERM` (exists but not ours) means alive;
/// `ESRCH` means gone.
#[cfg(unix)]
#[must_use]
pub fn is_process_alive(pid: u32) -> bool {
    let Some(p) = validate_pid_for_kill(pid) else {
        return false;
    };
    match nix::sys::signal::kill(nix::unistd::Pid::from_raw(p), None) {
        // Ok = signalable; EPERM = exists but owned by another user.
        Ok(()) | Err(nix::errno::Errno::EPERM) => true,
        Err(_) => false,
    }
}

/// Non-Unix fallback: conservatively assume a recorded PID is alive so we never
/// steal another instance's lock. (Daemon mode is Unix-first.)
#[cfg(not(unix))]
#[must_use]
pub fn is_process_alive(_pid: u32) -> bool {
    true
}

/// Send `SIGTERM` to `pid` for graceful shutdown.
///
/// # Errors
///
/// Returns an error if the signal cannot be delivered.
#[cfg(unix)]
pub fn signal_terminate(pid: u32) -> std::io::Result<()> {
    let p = validate_pid_for_kill(pid)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid pid"))?;
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(p),
        nix::sys::signal::Signal::SIGTERM,
    )
    .map_err(|e| std::io::Error::other(e.to_string()))
}

/// Force-kill `pid` with `SIGKILL`.
#[cfg(unix)]
pub fn force_kill(pid: u32) {
    if let Some(p) = validate_pid_for_kill(pid) {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(p),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
}

/// Wait up to `timeout` for `pid` to exit, polling its liveness.
/// Returns `true` if the process exited within the timeout.
#[cfg(unix)]
#[must_use]
pub fn wait_for_pid_exit(pid: u32, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    while is_process_alive(pid) {
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

/// Gracefully stop `pid`: send `SIGTERM`, wait up to `timeout` for it to drain
/// and exit, then force-kill if it is still alive.
#[cfg(unix)]
pub fn stop_pid(pid: u32, timeout: Duration) {
    let _ = signal_terminate(pid);
    if !wait_for_pid_exit(pid, timeout) {
        force_kill(pid);
        let _ = wait_for_pid_exit(pid, Duration::from_secs(5));
    }
}

/// Non-Unix fallback: no graceful-signal mechanism in this MVP.
#[cfg(not(unix))]
pub fn stop_pid(_pid: u32, _timeout: Duration) {}

/// Wait for a child process with a timeout. Returns `Err(())` if it did not
/// exit before `timeout` elapsed.
///
/// # Errors
///
/// Returns `Err(())` on timeout or if the child cannot be reaped.
#[cfg(unix)]
pub fn wait_with_timeout(child: &mut Child, timeout: Duration) -> Result<(), ()> {
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    return Err(());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_state_free_creates() {
        assert_eq!(classify(None, false), PidState::Free);
        assert_eq!(classify(None, true), PidState::Free);
    }

    #[test]
    fn pid_state_alive_rejected() {
        assert_eq!(classify(Some(123), true), PidState::Alive(123));
    }

    #[test]
    fn pid_state_stale_reclaimed() {
        assert_eq!(classify(Some(123), false), PidState::Stale);
    }

    #[test]
    fn write_pidfile_roundtrips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("serve.pid");
        let pid = std::process::id();
        acquire_pidfile(&path, pid).expect("acquire");
        assert_eq!(read_pidfile(&path), Some(pid));
    }

    #[test]
    fn create_new_rejects_live_pid() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("serve.pid");
        // Our own PID is alive; a second acquire must be rejected.
        acquire_pidfile(&path, std::process::id()).expect("first acquire");
        match acquire_pidfile(&path, std::process::id() + 1) {
            Err(AcquireError::AlreadyRunning(pid)) => assert_eq!(pid, std::process::id()),
            other => panic!("expected AlreadyRunning, got {other:?}"),
        }
    }

    #[test]
    fn stale_pidfile_reclaimed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("serve.pid");
        // A very high PID that is not running on any sane system.
        std::fs::write(&path, "2147483640\n").expect("seed stale pidfile");
        acquire_pidfile(&path, std::process::id()).expect("stale lock should be reclaimed");
        assert_eq!(read_pidfile(&path), Some(std::process::id()));
    }

    #[cfg(unix)]
    #[test]
    fn current_process_is_alive() {
        assert!(is_process_alive(std::process::id()));
    }

    #[cfg(unix)]
    #[test]
    fn high_unused_pid_is_not_alive() {
        assert!(!is_process_alive(2_147_483_640));
    }

    #[cfg(unix)]
    #[test]
    fn wait_with_timeout_succeeds_for_fast_process() {
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn true");
        std::thread::sleep(Duration::from_millis(50));
        assert!(wait_with_timeout(&mut child, Duration::from_secs(2)).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn wait_with_timeout_times_out_for_long_process() {
        let mut child = std::process::Command::new("sleep")
            .arg("60")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn sleep");
        assert!(wait_with_timeout(&mut child, Duration::from_millis(100)).is_err());
        let _ = child.kill();
        let _ = child.wait();
    }

    #[cfg(unix)]
    mod havoc_proptest {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn safe_pid_cast(pid in proptest::num::u32::ANY) {
                if let Some(safe_pid) = validate_pid_for_kill(pid) {
                    assert!(safe_pid > 0, "Safe PID must be strictly positive");
                }
            }
        }
    }
}
