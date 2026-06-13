//! `autumn canary` — drive canary rollback / promotion at the framework level.
//!
//! Autumn does not own the load-balancer traffic split (that stays a platform
//! concern). These commands drive the framework primitives a canary controller
//! depends on:
//!
//! - `autumn canary rollback` writes a flag file
//!   (`tmp/autumn-canary-rollback.json`) that a running canary replica polls.
//!   On seeing it, the replica flips `/ready` to 503, drains in-flight
//!   requests, and exits cleanly — no manual `SIGTERM` required.
//! - `autumn canary promote` removes the flag file (clears any pending
//!   rollback) and prints the platform step needed to shift traffic to 100 %.
//! - `autumn canary status` reports whether a rollback is currently pending.
//!
//! The flag file lives inside the replica's working directory, so a controller
//! targets a specific canary container (e.g. `fly ssh console --machine <id>`
//! or `kubectl exec <canary-pod>`).

use std::path::{Path, PathBuf};

use autumn_web::canary::{CANARY_ROLLBACK_FLAG_FILE, CanaryState, RollbackSignal};

/// Options for `autumn canary rollback`.
pub struct RollbackOptions<'a> {
    pub reason: Option<&'a str>,
    pub requested_by: Option<&'a str>,
    /// Override the default flag file path (used in tests).
    pub flag_file: Option<&'a Path>,
}

fn resolved_flag_path(override_: Option<&Path>) -> PathBuf {
    override_.map_or_else(|| PathBuf::from(CANARY_ROLLBACK_FLAG_FILE), Path::to_owned)
}

/// Trigger a canary rollback: write the flag file and print confirmation.
pub fn run_rollback(opts: &RollbackOptions<'_>) {
    let path = resolved_flag_path(opts.flag_file);
    let signal = RollbackSignal {
        reason: opts.reason.map(str::to_owned),
        requested_by: opts.requested_by.map(str::to_owned),
    };

    match CanaryState::write_rollback_flag(&path, &signal) {
        Ok(()) => {
            eprintln!("\u{1F342} Canary ROLLBACK signalled");
            eprintln!("   Flag file: {}", path.display());
            if let Some(reason) = &signal.reason {
                eprintln!("   Reason:    {reason}");
            }
            if let Some(by) = &signal.requested_by {
                eprintln!("   By:        {by}");
            }
            eprintln!();
            eprintln!("   The canary replica will flip /ready to 503, drain, and exit");
            eprintln!("   within ~500 ms. Reset platform traffic weight to 0% for the");
            eprintln!("   canary, then redeploy stable.");
        }
        Err(e) => {
            eprintln!("\u{274C} Failed to write canary rollback flag: {e}");
            std::process::exit(1);
        }
    }
}

/// Promote the canary: clear any pending rollback flag and print guidance.
pub fn run_promote(flag_file: Option<&Path>) {
    let path = resolved_flag_path(flag_file);
    match CanaryState::remove_rollback_flag(&path) {
        Ok(removed) => {
            eprintln!("\u{1F342} Canary PROMOTE");
            if removed {
                eprintln!("   Cleared a pending rollback flag.");
            } else {
                eprintln!("   No rollback was pending.");
            }
            eprintln!();
            eprintln!("   Framework state is clear. Shift platform traffic to 100% for the");
            eprintln!("   new version (e.g. `fly scale` / set machine weight, or relabel the");
            eprintln!("   Kubernetes Service selector) to complete the promotion.");
        }
        Err(e) => {
            eprintln!("\u{274C} Failed to clear canary rollback flag: {e}");
            std::process::exit(1);
        }
    }
}

/// Report whether a rollback is currently pending.
pub fn run_status(flag_file: Option<&Path>) {
    let path = resolved_flag_path(flag_file);
    match CanaryState::load_rollback_flag(&path) {
        Ok(Some(signal)) => {
            eprintln!("\u{1F342} Canary status: ROLLBACK PENDING");
            if let Some(reason) = &signal.reason {
                eprintln!("   Reason: {reason}");
            }
            if let Some(by) = &signal.requested_by {
                eprintln!("   By:     {by}");
            }
        }
        Ok(None) => {
            eprintln!("\u{1F342} Canary status: no rollback pending");
        }
        Err(e) => {
            eprintln!("\u{274C} Failed to read canary rollback flag: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollback_writes_flag_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("canary-rollback.json");
        run_rollback(&RollbackOptions {
            reason: Some("p99 latency exceeded"),
            requested_by: Some("ci-controller"),
            flag_file: Some(&path),
        });
        assert!(path.exists());
        let signal = CanaryState::load_rollback_flag(&path).unwrap().unwrap();
        assert_eq!(signal.reason.as_deref(), Some("p99 latency exceeded"));
        assert_eq!(signal.requested_by.as_deref(), Some("ci-controller"));
    }

    #[test]
    fn promote_removes_flag_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("canary-rollback.json");
        CanaryState::write_rollback_flag(&path, &RollbackSignal::default()).unwrap();
        run_promote(Some(&path));
        assert!(!path.exists());
    }

    #[test]
    fn promote_is_noop_when_not_pending() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("canary-rollback.json");
        // Must not panic when no flag exists.
        run_promote(Some(&path));
        assert!(!path.exists());
    }

    #[test]
    fn status_reads_pending_flag() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("canary-rollback.json");
        // No flag → no panic.
        run_status(Some(&path));
        // With flag → no panic.
        CanaryState::write_rollback_flag(
            &path,
            &RollbackSignal {
                reason: Some("manual".into()),
                requested_by: None,
            },
        )
        .unwrap();
        run_status(Some(&path));
    }
}
