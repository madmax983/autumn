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
/// Readiness budget when managed Postgres must provision on first boot
/// (download/extract + `initdb` can take well over the default before the app
/// binds its socket).
const READY_TIMEOUT_MANAGED_PG: Duration = Duration::from_secs(300);
/// Extra seconds added to the app's configured shutdown budget before a
/// graceful `stop` escalates to `SIGKILL`.
const STOP_GRACE_BUFFER: Duration = Duration::from_secs(5);
/// Fallback stop budget when the app's shutdown config can't be read.
const DEFAULT_STOP_TIMEOUT: Duration = Duration::from_secs(40);

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
    // Daemon mode is built on Unix domain sockets and POSIX signals; the
    // Windows path isn't supported yet. Fail clearly instead of binding TCP and
    // exiting (the app rejects `unix_socket` off-Unix) or signalling nothing.
    #[cfg(not(unix))]
    {
        let _ = (action, opts);
        eprintln!(
            "autumn serve: daemon mode is currently supported on Unix only \
             (Linux/macOS). Run the app binary directly on this platform."
        );
        std::process::exit(1);
    }
    #[cfg(unix)]
    {
        let code = run_unix(action, opts);
        std::process::exit(code);
    }
}

/// Unix implementation of `run` (daemon lifecycle).
#[cfg(unix)]
fn run_unix(action: Option<ServeAction>, opts: &ServeOptions) -> i32 {
    match action {
        None => start(opts),
        Some(ServeAction::Stop) => stop(opts),
        Some(ServeAction::Status) => status(opts),
        Some(ServeAction::Restart) => {
            // `restart` is a fresh invocation, so `opts` reflects the restart
            // command's flags — not the original `start`. Recover managed-PG
            // mode from the running daemon's address file so a bare `restart`
            // keeps the project data dir, the longer readiness budget, and the
            // managed_pg flag. Always relaunch in the background.
            let keep_managed =
                opts.bundled_pg || running_daemon_is_managed(opts.package.as_deref());
            let _ = stop(opts);
            let daemon_opts = ServeOptions {
                daemon: true,
                bundled_pg: keep_managed,
                ..opts.clone()
            };
            start(&daemon_opts)
        }
    }
}

/// Whether the currently-recorded daemon was started in managed-Postgres mode,
/// read from its address file. Best-effort: false if anything can't be read.
#[cfg(unix)]
fn running_daemon_is_managed(package: Option<&str>) -> bool {
    let Ok(paths) = RuntimePaths::resolve(&project_identity(package)) else {
        return false;
    };
    std::fs::read_to_string(paths.addr_file())
        .ok()
        .and_then(|s| AddrFile::parse(&s).ok())
        .is_some_and(|a| a.managed_pg)
}

/// Resolve the project's runtime paths, exiting on failure.
fn resolve_paths(package: Option<&str>) -> RuntimePaths {
    let project = project_identity(package);
    RuntimePaths::resolve(&project).unwrap_or_else(|e| {
        eprintln!("autumn serve: {e}");
        std::process::exit(1);
    })
}

/// Derive a stable project identity for namespacing runtime dirs.
///
/// The base name is the explicit package, else the current crate's
/// `[package].name`, else the cwd dir name. A short hash of the absolute
/// project directory is appended so two unrelated checkouts that share a
/// package name (e.g. two clones both named `api`) never collide on the same
/// pidfile / socket / managed-Postgres data dir.
fn project_identity(package: Option<&str>) -> String {
    let name = package
        .map(ToOwned::to_owned)
        .or_else(read_package_name)
        .unwrap_or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "autumn-app".to_owned())
        });
    format!("{name}-{}", project_dir_hash())
}

/// Read `[package].name` from the `Cargo.toml` in the current directory.
fn read_package_name() -> Option<String> {
    let contents = std::fs::read_to_string("Cargo.toml").ok()?;
    let table = toml::from_str::<toml::Table>(&contents).ok()?;
    table
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(toml::Value::as_str)
        .map(ToOwned::to_owned)
}

/// A short, stable hash of the absolute project directory, distinguishing
/// same-named projects in different locations.
fn project_dir_hash() -> String {
    use std::hash::{Hash, Hasher};
    let dir = std::env::current_dir()
        .and_then(std::fs::canonicalize)
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    dir.hash(&mut hasher);
    format!("{:08x}", hasher.finish() & 0xffff_ffff)
}

/// Build, then start the server (foreground or daemon). Returns the exit code.
fn start(opts: &ServeOptions) -> i32 {
    let paths = resolve_paths(opts.package.as_deref());
    if let Err(e) = paths.ensure_dirs() {
        eprintln!("autumn serve: cannot create runtime dirs: {e}");
        return 1;
    }

    // Reject a second start while a live daemon holds the lock.
    if let Some(rec) = process::read_pidfile(&paths.pid_file())
        && process::is_record_alive(&rec)
    {
        eprintln!(
            "autumn serve: already running (pid {}). \
             Use `autumn serve stop` or `autumn serve restart`.",
            rec.pid
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
    // Unix-socket binding is Unix-only; on other platforms the app rejects the
    // setting and exits, so leave it unset there (the app binds TCP instead).
    #[cfg(unix)]
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
    // Propagate the child's exit code. A signal death (`code()` is `None`) or a
    // failed `wait` is an abnormal exit, so report non-zero — supervisors and
    // scripts must not see a crash as success.
    status.map_or(1, |s| s.code().unwrap_or(1))
}

/// Spawn the server detached into the background and supervise readiness.
fn spawn_daemon(binary: &Path, paths: &RuntimePaths, opts: &ServeOptions) -> i32 {
    let socket = paths.socket_file();
    let log_path = paths.log_file();
    let log = match std::fs::File::create(&log_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!(
                "autumn serve: cannot open log file {}: {e}",
                log_path.display()
            );
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

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            eprintln!("\u{2717} Failed to start daemon: {e}");
            return 1;
        }
    };
    let pid = child.id();

    match process::acquire_pidfile(&paths.pid_file(), pid) {
        Ok(()) => {}
        Err(AcquireError::AlreadyRunning(existing)) => {
            eprintln!("autumn serve: already running (pid {existing}).");
            let _ = child.kill();
            let _ = child.wait();
            return 1;
        }
        Err(AcquireError::Io(e)) => {
            eprintln!("autumn serve: cannot write pidfile: {e}");
            let _ = child.kill();
            let _ = child.wait();
            return 1;
        }
    }

    let ready_timeout = if opts.bundled_pg {
        READY_TIMEOUT_MANAGED_PG
    } else {
        READY_TIMEOUT
    };
    if wait_for_ready(&socket, &mut child, ready_timeout) {
        write_addr_file(paths, pid, &socket, opts.bundled_pg);
        println!(
            "autumn serve: started (pid {pid}) on unix:{}",
            socket.display()
        );
        println!("  address file: {}", paths.addr_file().display());
        println!("  logs: {}", log_path.display());
        // Detach: leave the running child to be reparented to init on exit.
        // (Dropping the handle does not signal or reap the live process.)
        drop(child);
        0
    } else {
        eprintln!(
            "autumn serve: daemon did not become ready within {}s; see {}",
            READY_TIMEOUT.as_secs(),
            log_path.display()
        );
        let _ = child.kill();
        let _ = child.wait();
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

/// Poll until the daemon is actually accepting connections on `socket`, or the
/// child exits/crashes, or the timeout elapses.
///
/// We probe with an actual `UnixStream::connect` rather than checking path
/// existence: a stale socket left by a crashed previous daemon exists on disk
/// but refuses connections, so existence alone would report a false ready.
/// `child.try_wait()` detects a child that died during startup (a zombie keeps
/// `kill(pid, 0)` alive, which would otherwise hang the full timeout).
fn wait_for_ready(socket: &Path, child: &mut std::process::Child, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    loop {
        #[cfg(unix)]
        if std::os::unix::net::UnixStream::connect(socket).is_ok() {
            return true;
        }
        #[cfg(not(unix))]
        if socket.exists() {
            return true;
        }
        match child.try_wait() {
            // Child exited/crashed before binding — fail fast.
            Ok(Some(_)) | Err(_) => return false,
            Ok(None) => {}
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
            .map_or(0, |d| d.as_secs()),
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
    let Some(rec) = process::read_pidfile(&paths.pid_file()) else {
        println!("autumn serve: not running");
        return 0;
    };
    if !process::is_record_alive(&rec) {
        println!("autumn serve: not running (removed stale pidfile)");
        cleanup(&paths, &socket);
        return 0;
    }

    process::stop_pid(rec.pid, stop_timeout());
    cleanup(&paths, &socket);
    println!("autumn serve: stopped");
    0
}

/// Graceful-stop budget before escalating to `SIGKILL`, derived from the app's
/// own `[server]` shutdown config (`prestop_grace_secs` plus
/// `shutdown_timeout_secs` plus a small buffer) so we never cut off a drain the
/// server is configured to allow. Falls back to a sane default when
/// `autumn.toml` is absent or unreadable.
fn stop_timeout() -> Duration {
    #[derive(serde::Deserialize)]
    struct ServerSlice {
        prestop_grace_secs: Option<u64>,
        shutdown_timeout_secs: Option<u64>,
    }
    #[derive(serde::Deserialize)]
    struct TomlSlice {
        server: Option<ServerSlice>,
    }
    let Ok(contents) = std::fs::read_to_string("autumn.toml") else {
        return DEFAULT_STOP_TIMEOUT;
    };
    let Ok(slice) = toml::from_str::<TomlSlice>(&contents) else {
        return DEFAULT_STOP_TIMEOUT;
    };
    let server = slice.server.unwrap_or(ServerSlice {
        prestop_grace_secs: None,
        shutdown_timeout_secs: None,
    });
    let budget =
        server.prestop_grace_secs.unwrap_or(5) + server.shutdown_timeout_secs.unwrap_or(30);
    Duration::from_secs(budget) + STOP_GRACE_BUFFER
}

/// Report daemon status. Exit code 0 = running, 3 = stopped.
fn status(opts: &ServeOptions) -> i32 {
    let paths = resolve_paths(opts.package.as_deref());
    let Some(rec) = process::read_pidfile(&paths.pid_file()) else {
        println!("autumn serve: stopped");
        return 3;
    };
    if process::is_record_alive(&rec) {
        let address = std::fs::read_to_string(paths.addr_file())
            .ok()
            .and_then(|s| AddrFile::parse(&s).ok())
            .map_or_else(|| paths.socket_file().display().to_string(), |a| a.address);
        println!("autumn serve: running (pid {}) on unix:{address}", rec.pid);
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
        let id = project_identity(Some("my-svc"));
        // Base name from the explicit package, plus a project-dir hash suffix
        // so unrelated checkouts with the same package name don't collide.
        assert!(id.starts_with("my-svc-"), "got {id}");
        assert!(id.len() > "my-svc-".len());
    }
}
