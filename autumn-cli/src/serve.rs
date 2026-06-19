//! `autumn serve` — run the app as a production (non-watch) server, optionally
//! as a managed background daemon.
//!
//! Distinct from `autumn dev`: no file watching and no hot-reload. Instead it
//! provides a daemon lifecycle (`--daemon`, `stop`, `status`, `restart`) backed
//! by a PID lockfile, binds a Unix domain socket under a platform runtime dir,
//! and writes an address-discovery file so a thin client (or an agent) can find
//! the running service. Graceful shutdown reuses the app's existing lame-duck
//! drain via `SIGTERM`.

use crate::paths::RuntimePaths;
use crate::process::{self, AcquireError};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Lifecycle subcommand for `autumn serve`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServeAction {
    /// Stop the running daemon.
    Stop,
    /// Report whether the daemon is running and where it is reachable.
    Status,
    /// Stop (if running) then start in the background.
    Restart,
}

/// Options shared across `serve` invocations.
#[derive(Debug, Clone)]
pub struct ServeOptions {
    /// Package to build/run (for workspaces).
    pub package: Option<String>,
    /// Run in the background as a managed daemon.
    pub daemon: bool,
    /// Build in release mode (optimized production binary).
    pub release: bool,
    /// Recorded in the address file; set when the app bundles managed Postgres.
    pub bundled_pg: bool,
}

/// How long to wait for a freshly-spawned daemon to become reachable.
const READY_TIMEOUT: Duration = Duration::from_secs(30);
/// How long to wait for a graceful stop before force-killing.
const STOP_TIMEOUT: Duration = Duration::from_secs(35);

/// Contents of the address-discovery file (`serve.addr`): how a client reaches
/// the running daemon. Serialized as TOML.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddrFile {
    /// PID of the server process.
    pub pid: u32,
    /// Transport: `"unix"` or `"tcp"`.
    pub transport: String,
    /// Socket path (unix) or `host:port` (tcp).
    pub address: String,
    /// Unix-epoch seconds when the daemon was started.
    pub started_at: u64,
    /// Whether the app supervises a bundled/managed Postgres.
    pub managed_pg: bool,
}

impl AddrFile {
    /// Serialize to TOML.
    #[must_use]
    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("AddrFile serializes to TOML")
    }

    /// Parse from TOML.
    ///
    /// # Errors
    ///
    /// Returns a TOML deserialization error if the contents are malformed.
    pub fn parse(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }
}

/// Entry point dispatched from `main::run_command`.
pub fn run(action: Option<ServeAction>, opts: &ServeOptions) {
    let code = match action {
        None => start(opts),
        Some(ServeAction::Stop) => stop(opts),
        Some(ServeAction::Status) => status(opts),
        Some(ServeAction::Restart) => {
            let _ = stop(opts);
            start(opts)
        }
    };
    std::process::exit(code);
}

/// Resolve the project's runtime paths, exiting on failure.
fn resolve_paths(package: Option<&str>) -> RuntimePaths {
    let project = project_identity(package);
    RuntimePaths::resolve(&project).unwrap_or_else(|e| {
        eprintln!("autumn serve: {e}");
        std::process::exit(1);
    })
}

/// Derive a stable project identity for namespacing runtime dirs: the explicit
/// package, else the current crate's `[package].name`, else the cwd dir name.
fn project_identity(package: Option<&str>) -> String {
    if let Some(pkg) = package {
        return pkg.to_owned();
    }
    if let Ok(contents) = std::fs::read_to_string("Cargo.toml")
        && let Ok(table) = toml::from_str::<toml::Table>(&contents)
        && let Some(name) = table
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(toml::Value::as_str)
    {
        return name.to_owned();
    }
    std::env::current_dir()
        .ok()
        .and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "autumn-app".to_owned())
}

/// Build, then start the server (foreground or daemon). Returns the exit code.
fn start(opts: &ServeOptions) -> i32 {
    let paths = resolve_paths(opts.package.as_deref());
    if let Err(e) = paths.ensure_dirs() {
        eprintln!("autumn serve: cannot create runtime dirs: {e}");
        return 1;
    }

    // Reject a second start while a live daemon holds the lock.
    if let Some(pid) = process::read_pidfile(&paths.pid_file())
        && process::is_process_alive(pid)
    {
        eprintln!(
            "autumn serve: already running (pid {pid}). \
             Use `autumn serve stop` or `autumn serve restart`."
        );
        return 1;
    }

    eprintln!("\u{1F342} autumn serve\n");
    if !crate::dev::cargo_build(opts.package.as_deref(), opts.release) {
        eprintln!("\u{2717} Build failed. Fix the errors above and retry.");
        return 1;
    }
    let binary = crate::dev::find_binary(opts.package.as_deref(), opts.release);

    if opts.daemon {
        spawn_daemon(&binary, &paths, opts)
    } else {
        run_foreground(&binary, &paths, opts)
    }
}

/// Base command for the app binary: bind the Unix socket via config env, and
/// (for managed-Postgres apps) point the bundled cluster at a persistent dir.
fn base_command(binary: &Path, paths: &RuntimePaths, opts: &ServeOptions) -> Command {
    let mut cmd = Command::new(binary);
    cmd.env("AUTUMN_SERVER__UNIX_SOCKET", paths.socket_file());
    if opts.bundled_pg {
        cmd.env("AUTUMN_MANAGED_PG_DATA_DIR", paths.pg_data_dir());
    }
    cmd
}

/// Run the server in the foreground, blocking until it exits. Ctrl-C reaches
/// the child directly (shared process group) and triggers its graceful drain.
fn run_foreground(binary: &Path, paths: &RuntimePaths, opts: &ServeOptions) -> i32 {
    let socket = paths.socket_file();
    let mut child = match base_command(binary, paths, opts).spawn() {
        Ok(child) => child,
        Err(e) => {
            eprintln!("\u{2717} Failed to start {}: {e}", binary.display());
            return 1;
        }
    };

    if let Err(code) = record_started(paths, child.id(), &socket, opts, || {
        let _ = child.kill();
    }) {
        return code;
    }

    // Install a no-op SIGINT handler so Ctrl-C does not kill this supervisor
    // before the child drains; the child (same process group) receives SIGINT
    // and shuts down gracefully, after which `wait()` returns.
    let _ = ctrlc::set_handler(|| {});

    eprintln!("  Listening on unix:{}", socket.display());
    eprintln!("  Address file: {}", paths.addr_file().display());
    eprintln!("  Press Ctrl+C to stop\n");

    let status = child.wait();
    cleanup(paths, &socket);
    status.ok().and_then(|s| s.code()).unwrap_or(0)
}

/// Spawn the server detached into the background and supervise readiness.
fn spawn_daemon(binary: &Path, paths: &RuntimePaths, opts: &ServeOptions) -> i32 {
    let socket = paths.socket_file();
    let log_path = paths.log_file();
    let log = match std::fs::File::create(&log_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("autumn serve: cannot open log file {}: {e}", log_path.display());
            return 1;
        }
    };
    let log_err = match log.try_clone() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("autumn serve: cannot duplicate log handle: {e}");
            return 1;
        }
    };

    let mut cmd = base_command(binary, paths, opts);
    cmd.stdin(std::process::Stdio::null())
        .stdout(log)
        .stderr(log_err);
    detach(&mut cmd);

    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            eprintln!("\u{2717} Failed to start daemon: {e}");
            return 1;
        }
    };
    let pid = child.id();
    // Drop the handle without waiting; on Unix the detached child keeps running
    // and is reparented to init when this supervisor exits.
    drop(child);

    match process::acquire_pidfile(&paths.pid_file(), pid) {
        Ok(()) => {}
        Err(AcquireError::AlreadyRunning(existing)) => {
            eprintln!("autumn serve: already running (pid {existing}).");
            process::stop_pid(pid, Duration::from_secs(5));
            return 1;
        }
        Err(AcquireError::Io(e)) => {
            eprintln!("autumn serve: cannot write pidfile: {e}");
            process::stop_pid(pid, Duration::from_secs(5));
            return 1;
        }
    }

    if wait_for_ready(&socket, pid, READY_TIMEOUT) {
        write_addr_file(paths, pid, &socket, opts.bundled_pg);
        println!("autumn serve: started (pid {pid}) on unix:{}", socket.display());
        println!("  address file: {}", paths.addr_file().display());
        println!("  logs: {}", log_path.display());
        0
    } else {
        eprintln!(
            "autumn serve: daemon did not become ready within {}s; see {}",
            READY_TIMEOUT.as_secs(),
            log_path.display()
        );
        process::stop_pid(pid, Duration::from_secs(5));
        cleanup(paths, &socket);
        1
    }
}

/// Record the pidfile + address file for a started server. Calls `on_conflict`
/// (e.g. to kill the just-spawned child) when another daemon already holds the
/// lock. Returns `Err(exit_code)` on failure.
fn record_started(
    paths: &RuntimePaths,
    pid: u32,
    socket: &Path,
    opts: &ServeOptions,
    on_conflict: impl FnOnce(),
) -> Result<(), i32> {
    match process::acquire_pidfile(&paths.pid_file(), pid) {
        Ok(()) => {
            write_addr_file(paths, pid, socket, opts.bundled_pg);
            Ok(())
        }
        Err(AcquireError::AlreadyRunning(existing)) => {
            eprintln!("autumn serve: already running (pid {existing}).");
            on_conflict();
            Err(1)
        }
        Err(AcquireError::Io(e)) => {
            eprintln!("autumn serve: cannot write pidfile: {e}");
            on_conflict();
            Err(1)
        }
    }
}

/// Apply OS-specific flags to detach the spawned process from the terminal.
#[cfg(unix)]
fn detach(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    // New process group: terminal job-control signals (Ctrl-C) won't reach it.
    cmd.process_group(0);
}

#[cfg(windows)]
fn detach(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
}

#[cfg(not(any(unix, windows)))]
fn detach(_cmd: &mut Command) {}

/// Poll until the socket appears (daemon is reachable) or the process dies /
/// the timeout elapses.
fn wait_for_ready(socket: &Path, pid: u32, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    loop {
        if socket.exists() {
            return true;
        }
        if !process::is_process_alive(pid) {
            return false;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Write the address-discovery file with `0600` permissions.
fn write_addr_file(paths: &RuntimePaths, pid: u32, socket: &Path, managed_pg: bool) {
    let addr = AddrFile {
        pid,
        transport: "unix".to_owned(),
        address: socket.display().to_string(),
        started_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        managed_pg,
    };
    let path = paths.addr_file();
    if let Err(e) = std::fs::write(&path, addr.to_toml()) {
        eprintln!("autumn serve: warning: could not write address file: {e}");
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
}

/// Stop the running daemon. Returns the exit code.
fn stop(opts: &ServeOptions) -> i32 {
    let paths = resolve_paths(opts.package.as_deref());
    let socket = paths.socket_file();
    let Some(pid) = process::read_pidfile(&paths.pid_file()) else {
        println!("autumn serve: not running");
        return 0;
    };
    if !process::is_process_alive(pid) {
        println!("autumn serve: not running (removed stale pidfile)");
        cleanup(&paths, &socket);
        return 0;
    }

    process::stop_pid(pid, STOP_TIMEOUT);
    cleanup(&paths, &socket);
    println!("autumn serve: stopped");
    0
}

/// Report daemon status. Exit code 0 = running, 3 = stopped.
fn status(opts: &ServeOptions) -> i32 {
    let paths = resolve_paths(opts.package.as_deref());
    let Some(pid) = process::read_pidfile(&paths.pid_file()) else {
        println!("autumn serve: stopped");
        return 3;
    };
    if process::is_process_alive(pid) {
        let address = std::fs::read_to_string(paths.addr_file())
            .ok()
            .and_then(|s| AddrFile::parse(&s).ok())
            .map_or_else(
                || paths.socket_file().display().to_string(),
                |a| a.address,
            );
        println!("autumn serve: running (pid {pid}) on unix:{address}");
        0
    } else {
        println!(
            "autumn serve: stopped (stale pidfile at {})",
            paths.pid_file().display()
        );
        3
    }
}

/// Best-effort removal of the pidfile, address file, and socket.
fn cleanup(paths: &RuntimePaths, socket: &Path) {
    let _ = std::fs::remove_file(paths.pid_file());
    let _ = std::fs::remove_file(paths.addr_file());
    let _ = std::fs::remove_file(socket);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(transport: &str, address: &str, managed_pg: bool) -> AddrFile {
        AddrFile {
            pid: 4242,
            transport: transport.to_owned(),
            address: address.to_owned(),
            started_at: 1_700_000_000,
            managed_pg,
        }
    }

    #[test]
    fn serialize_addr_file_unix_roundtrips() {
        let a = sample("unix", "/run/user/1000/autumn/demo/serve.sock", false);
        let parsed = AddrFile::parse(&a.to_toml()).expect("parse");
        assert_eq!(a, parsed);
    }

    #[test]
    fn serialize_addr_file_tcp_roundtrips() {
        let a = sample("tcp", "127.0.0.1:3000", true);
        let parsed = AddrFile::parse(&a.to_toml()).expect("parse");
        assert_eq!(a, parsed);
    }

    #[test]
    fn addr_file_is_valid_toml() {
        let a = sample("unix", "/tmp/x.sock", false);
        let table: toml::Table = toml::from_str(&a.to_toml()).expect("valid toml");
        for key in ["pid", "transport", "address", "started_at", "managed_pg"] {
            assert!(table.contains_key(key), "missing key {key}");
        }
    }

    #[test]
    fn project_identity_prefers_explicit_package() {
        assert_eq!(project_identity(Some("my-svc")), "my-svc");
    }
}
