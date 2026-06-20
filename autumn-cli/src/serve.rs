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
    /// Whether the daemon was built/started in release mode. Recovered by
    /// `restart` so a bare `autumn serve restart` keeps the optimized binary and
    /// the corresponding (prod) profile defaults. Defaults to `false` for address
    /// files written before this field existed.
    #[serde(default)]
    pub release: bool,
    /// The graceful-drain budget (`prestop_grace_secs + shutdown_timeout_secs`)
    /// resolved from the daemon's *own* environment and profile at start time.
    /// `stop` uses this as the authoritative budget so it never derives one from
    /// the (possibly different) `stop` invocation's environment. `None` for
    /// address files written before this field existed.
    #[serde(default)]
    pub stop_budget_secs: Option<u64>,
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
    #[cfg(unix)]
    let code = run_unix(action, opts);
    #[cfg(not(unix))]
    let code = run_non_unix(action, opts);
    std::process::exit(code);
}

/// Non-Unix entry point. The background-daemon lifecycle (`--daemon`, `stop`,
/// `status`, `restart`) is built on Unix domain sockets and POSIX signals and is
/// not supported here, but a plain foreground `autumn serve` still works: it
/// builds and runs the app binary, which binds TCP per its config.
#[cfg(not(unix))]
fn run_non_unix(action: Option<ServeAction>, opts: &ServeOptions) -> i32 {
    if action.is_some() || opts.daemon {
        eprintln!(
            "autumn serve: daemon mode (--daemon, stop, status, restart) is \
             currently supported on Unix only (Linux/macOS). Plain `autumn serve` \
             runs the app in the foreground on this platform."
        );
        return 1;
    }
    // Plain foreground server: builds and runs on the configured transport,
    // deferring (and, when not bundling Postgres, skipping) runtime-path setup.
    start_foreground(opts)
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
            // command's flags — not the original `start`. Recover managed-PG mode
            // and release mode from the running daemon's address file so a bare
            // `restart` keeps the project data dir, the longer readiness budget,
            // the optimized binary, and the matching profile defaults. Always
            // relaunch in the background.
            let running = running_daemon_addr(opts.package.as_deref());
            let keep_managed = opts.bundled_pg || running.as_ref().is_some_and(|a| a.managed_pg);
            let keep_release = opts.release || running.as_ref().is_some_and(|a| a.release);
            let _ = stop(opts);
            let daemon_opts = ServeOptions {
                daemon: true,
                bundled_pg: keep_managed,
                release: keep_release,
                ..opts.clone()
            };
            start(&daemon_opts)
        }
    }
}

/// Parse the address file under `paths`, if present and well-formed.
fn read_addr_file(paths: &RuntimePaths) -> Option<AddrFile> {
    std::fs::read_to_string(paths.addr_file())
        .ok()
        .and_then(|s| AddrFile::parse(&s).ok())
}

/// The currently-recorded daemon's address file, if present and parseable.
/// Best-effort: `None` if anything can't be read.
fn running_daemon_addr(package: Option<&str>) -> Option<AddrFile> {
    let paths = RuntimePaths::resolve(&project_identity(package)).ok()?;
    read_addr_file(&paths)
}

/// Whether a Unix socket at `path` has a live listener (a `connect` succeeds).
/// Used as a portable daemon-identity check where the OS can't verify a recorded
/// process start time (e.g. macOS).
#[cfg(unix)]
fn socket_is_live(path: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

#[cfg(not(unix))]
fn socket_is_live(_path: &Path) -> bool {
    false
}

/// Whether `rec` identifies our live daemon, guarding against PID reuse.
///
/// A recorded start time that matches the live process is conclusive (Linux).
/// Otherwise — no recorded start time, or a platform that can't report one such
/// as macOS — the PID alone is ambiguous, so require the daemon's socket to have
/// a live listener: an unrelated process that reused the PID would not be
/// listening there. This stops `status`/`stop` from acting on a reused PID.
fn confirmed_running(rec: &process::PidRecord, socket: &Path) -> bool {
    if !process::is_process_alive(rec.pid) {
        return false;
    }
    if let (Some(recorded), Some(current)) = (rec.start_time, process::process_start_time(rec.pid))
        && recorded == current
    {
        return true;
    }
    socket_is_live(socket)
}

/// Resolve the daemon PID for lifecycle commands. Prefers the authoritative
/// pidfile (which carries a start time for PID-reuse detection); if it is
/// missing or corrupt, falls back to the address file's PID — but only when a
/// live listener confirms a daemon is still serving on the socket, so a removed
/// pidfile doesn't leave the daemon unstoppable.
fn lifecycle_target(paths: &RuntimePaths, socket: &Path) -> Option<process::PidRecord> {
    if let Some(rec) = process::read_pidfile(&paths.pid_file()) {
        return Some(rec);
    }
    let addr = read_addr_file(paths)?;
    socket_is_live(socket).then_some(process::PidRecord {
        pid: addr.pid,
        start_time: None,
    })
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
    format!("{name}-{}", project_dir_hash(package))
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
///
/// For a workspace member selected with `-p`, hash the member's manifest dir so
/// the runtime namespace is identical whether the lifecycle command is run from
/// the workspace root or the member directory (otherwise `start -p api` and
/// `stop -p api` from different CWDs would target different daemons). Falls back
/// to the canonicalized CWD when no package is given or its manifest dir can't
/// be resolved.
fn project_dir_hash(package: Option<&str>) -> String {
    use sha2::{Digest, Sha256};
    let dir = package
        .and_then(crate::dev::find_manifest_dir)
        .or_else(|| std::env::current_dir().ok())
        .and_then(|d| std::fs::canonicalize(&d).ok().or(Some(d)))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    // A fixed SHA-256 over the canonical path — `DefaultHasher` is explicitly
    // not stable across std/toolchain versions, so a CLI upgrade while a daemon
    // is running must not change the namespace (which would orphan it). First
    // 4 bytes (8 hex) keep the resulting Unix socket path within `sun_path`.
    let digest = Sha256::digest(dir.to_string_lossy().as_bytes());
    hex::encode(&digest[..4])
}

/// Build, then start the server (foreground or daemon). Returns the exit code.
fn start(opts: &ServeOptions) -> i32 {
    if opts.daemon {
        start_daemon(opts)
    } else {
        start_foreground(opts)
    }
}

/// Build and launch the background daemon: requires the runtime dirs (pidfile,
/// socket, log) and rejects a second start while a live daemon holds the lock.
fn start_daemon(opts: &ServeOptions) -> i32 {
    let paths = resolve_paths(opts.package.as_deref());
    if let Err(e) = paths.ensure_dirs() {
        eprintln!("autumn serve: cannot create runtime dirs: {e}");
        return 1;
    }

    // Reject a second start while a live daemon holds the lock. Use the
    // socket-confirmed identity check so a stale pidfile whose PID was reused by
    // an unrelated process (notably on macOS, where start time isn't available)
    // doesn't block a legitimate start.
    if let Some(rec) = process::read_pidfile(&paths.pid_file())
        && confirmed_running(&rec, &paths.socket_file())
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
    spawn_daemon(&binary, &paths, opts)
}

/// Build and run a plain foreground server. Unlike the daemon, this needs no
/// pidfile/socket/log directory; only managed Postgres needs a persistent data
/// dir. Resolving the platform runtime dirs is therefore deferred and skipped
/// entirely for a non-bundled TCP server, so foreground `autumn serve` works in
/// minimal containers without a discoverable home/platform directory.
fn start_foreground(opts: &ServeOptions) -> i32 {
    eprintln!("\u{1F342} autumn serve\n");
    if !crate::dev::cargo_build(opts.package.as_deref(), opts.release) {
        eprintln!("\u{2717} Build failed. Fix the errors above and retry.");
        return 1;
    }
    let binary = crate::dev::find_binary(opts.package.as_deref(), opts.release);

    let paths = if opts.bundled_pg {
        let paths = resolve_paths(opts.package.as_deref());
        if let Err(e) = paths.ensure_dirs() {
            eprintln!("autumn serve: cannot create runtime dirs: {e}");
            return 1;
        }
        Some(paths)
    } else {
        None
    };
    run_foreground(&binary, paths.as_ref(), opts)
}

/// Base command for the app binary: for daemon starts bind the Unix socket via
/// config env, and (for managed-Postgres apps) point the bundled cluster at a
/// persistent dir. `paths` is `None` for a plain foreground server that needs no
/// runtime directories.
fn base_command(binary: &Path, paths: Option<&RuntimePaths>, opts: &ServeOptions) -> Command {
    let mut cmd = Command::new(binary);
    // For a workspace member selected with `-p`, run the child from the member's
    // manifest dir so its `autumn.toml`/profile and asset dirs resolve correctly
    // instead of the workspace-root CWD. Set both `current_dir` (covers CWD-
    // relative assets) and `AUTUMN_MANIFEST_DIR` (the config loader's explicit
    // override) so the member's config is loaded regardless of build mode.
    if let Some(pkg) = opts.package.as_deref()
        && let Some(dir) = crate::dev::find_manifest_dir(pkg)
    {
        cmd.current_dir(&dir);
        cmd.env("AUTUMN_MANIFEST_DIR", &dir);
    }
    // The Unix-domain socket is the daemon's private transport (for address-file
    // discovery and the thin client). Only force it for daemon starts: a plain
    // foreground `autumn serve` must stay on its configured `server.host`/`port`
    // (the app rejects `unix_socket` off-Unix anyway), so it remains reachable at
    // the expected TCP address like any production server.
    #[cfg(unix)]
    if opts.daemon
        && let Some(paths) = paths
    {
        cmd.env("AUTUMN_SERVER__UNIX_SOCKET", paths.socket_file());
    }
    if opts.bundled_pg
        && let Some(paths) = paths
    {
        cmd.env("AUTUMN_MANAGED_PG_DATA_DIR", paths.pg_data_dir());
    }
    cmd
}

/// Run the server in the foreground on its configured transport. On Unix this
/// `exec`s the app, replacing this process so there is no supervisor hop:
/// SIGTERM/SIGINT from systemd/Docker/k8s and the exit code flow straight to the
/// server and its graceful drain runs. No daemon pidfile/socket/address-file
/// machinery applies to a foreground run.
fn run_foreground(binary: &Path, paths: Option<&RuntimePaths>, opts: &ServeOptions) -> i32 {
    let mut cmd = base_command(binary, paths, opts);
    eprintln!("  Running {} in the foreground", binary.display());
    eprintln!("  Press Ctrl+C to stop\n");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // `exec` only returns on failure; on success this process *becomes* the
        // app, so termination signals reach it directly.
        let err = cmd.exec();
        eprintln!("\u{2717} Failed to start {}: {err}", binary.display());
        1
    }
    #[cfg(not(unix))]
    {
        match cmd.status() {
            Ok(status) => status.code().unwrap_or(1),
            Err(e) => {
                eprintln!("\u{2717} Failed to start {}: {e}", binary.display());
                1
            }
        }
    }
}

/// Create (truncating) the daemon log file with owner-only (`0600`) permissions
/// on Unix. The log captures the child's stdout/stderr — request data, panics,
/// and diagnostics — so other local users who can traverse the log dir must not
/// be able to read it.
fn create_private_log(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    // `mode(0o600)` only applies when *creating* the file; an existing log
    // (from an earlier version or manual creation) keeps its old, possibly
    // world-readable mode, so tighten it explicitly after opening.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    Ok(file)
}

/// Spawn the server detached into the background and supervise readiness.
fn spawn_daemon(binary: &Path, paths: &RuntimePaths, opts: &ServeOptions) -> i32 {
    let socket = paths.socket_file();

    // A live listener already owning the socket means a daemon is serving here
    // even if the pidfile is missing or stale. Our child would fail to bind (the
    // app refuses to clobber a live socket), but readiness probes `connect` and
    // would otherwise latch onto the *pre-existing* listener and report the
    // just-spawned child "ready". Detect that here and refuse, mirroring the
    // pidfile guard.
    #[cfg(unix)]
    if std::os::unix::net::UnixStream::connect(&socket).is_ok() {
        eprintln!(
            "autumn serve: already running (a live server owns {}). \
             Use `autumn serve stop` or `autumn serve restart`.",
            socket.display()
        );
        return 1;
    }

    let log_path = paths.log_file();
    let log = match create_private_log(&log_path) {
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

    let mut cmd = base_command(binary, Some(paths), opts);
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
            process::force_kill_group(pid);
            let _ = child.wait();
            return 1;
        }
        Err(AcquireError::Io(e)) => {
            eprintln!("autumn serve: cannot write pidfile: {e}");
            let _ = child.kill();
            process::force_kill_group(pid);
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
        if let Err(e) = write_addr_file(paths, pid, &socket, opts) {
            // Without the discovery file the daemon is unreachable to clients;
            // treat it like the pidfile failure — stop the child and fail.
            eprintln!(
                "autumn serve: cannot write address file {}: {e}",
                paths.addr_file().display()
            );
            let _ = child.kill();
            process::force_kill_group(pid);
            let _ = child.wait();
            cleanup(paths, &socket);
            return 1;
        }
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
            ready_timeout.as_secs(),
            log_path.display()
        );
        // Kill the whole group: a startup that timed out may have spawned
        // children (e.g. managed Postgres during `--bundled-pg` provisioning)
        // that `Child::kill()` alone would leave orphaned.
        let _ = child.kill();
        process::force_kill_group(pid);
        let _ = child.wait();
        cleanup(paths, &socket);
        1
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
        // Check child liveness *before* trusting the socket: a child that died
        // during startup (e.g. failed to bind because another listener owns the
        // socket) must not be reported ready by a `connect` that succeeds
        // against that unrelated listener.
        match child.try_wait() {
            // Child exited/crashed before binding — fail fast.
            Ok(Some(_)) | Err(_) => return false,
            Ok(None) => {}
        }
        #[cfg(unix)]
        if std::os::unix::net::UnixStream::connect(socket).is_ok() {
            return true;
        }
        #[cfg(not(unix))]
        if socket.exists() {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Write the address-discovery file with `0600` permissions.
///
/// # Errors
///
/// Returns the I/O error if the file cannot be written — a daemon without its
/// discovery file is unreachable to thin clients/agents, so the caller treats
/// this as a failed start.
fn write_addr_file(
    paths: &RuntimePaths,
    pid: u32,
    socket: &Path,
    opts: &ServeOptions,
) -> std::io::Result<()> {
    let addr = AddrFile {
        pid,
        transport: "unix".to_owned(),
        address: socket.display().to_string(),
        started_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        managed_pg: opts.bundled_pg,
        release: opts.release,
        // Resolve the budget now, from the daemon's own env/profile, so `stop`
        // doesn't have to (and can't) reconstruct it from a different shell.
        stop_budget_secs: Some(resolved_stop_budget_secs(opts)),
    };
    let path = paths.addr_file();
    std::fs::write(&path, addr.to_toml())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Stop the running daemon. Returns the exit code.
fn stop(opts: &ServeOptions) -> i32 {
    let paths = resolve_paths(opts.package.as_deref());
    let socket = paths.socket_file();
    let Some(rec) = lifecycle_target(&paths, &socket) else {
        println!("autumn serve: not running");
        return 0;
    };
    if !confirmed_running(&rec, &socket) {
        println!("autumn serve: not running (removed stale files)");
        cleanup(&paths, &socket);
        return 0;
    }

    // Prefer the budget the daemon recorded at start (resolved from its own
    // env/profile); fall back to recomputing it (release flag + config) only for
    // daemons started before that field was recorded.
    let addr = read_addr_file(&paths);
    let recorded_release = addr.as_ref().is_some_and(|a| a.release);
    let recorded_budget = addr.as_ref().and_then(|a| a.stop_budget_secs);
    process::stop_pid(
        rec.pid,
        stop_timeout(opts, recorded_release, recorded_budget),
    );
    cleanup(&paths, &socket);
    println!("autumn serve: stopped");
    0
}

/// Graceful-stop budget before escalating to `SIGKILL`.
///
/// Prefers `recorded_budget` — the drain budget the daemon resolved from its own
/// environment and profile at start and persisted in the address file — so a
/// `stop` from a different shell can't derive a shorter budget from its own env.
/// For daemons started before that field existed, falls back to recomputing it
/// here: base `autumn.toml` ← `[profile.<name>]` ← `autumn-<profile>.toml`
/// ← `AUTUMN_SERVER__*`, with the profile taken from `AUTUMN_ENV`/`AUTUMN_PROFILE`
/// else inferred (`prod` for a release daemon, else `dev`). A small buffer is
/// added on top of the configured drain.
fn stop_timeout(
    opts: &ServeOptions,
    recorded_release: bool,
    recorded_budget: Option<u64>,
) -> Duration {
    if let Some(secs) = recorded_budget {
        return Duration::from_secs(secs) + STOP_GRACE_BUFFER;
    }
    let base_dir = opts
        .package
        .as_deref()
        .and_then(crate::dev::find_manifest_dir)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let profile = effective_profile(recorded_release);
    let (prestop, shutdown) = resolve_shutdown_budget(&base_dir, Some(&profile));
    Duration::from_secs(prestop + shutdown) + STOP_GRACE_BUFFER
}

/// The drain budget (`prestop_grace_secs + shutdown_timeout_secs`) resolved from
/// the *current* (daemon's) environment and profile. Called at start so the
/// value persisted in the address file reflects the daemon's own settings.
fn resolved_stop_budget_secs(opts: &ServeOptions) -> u64 {
    let base_dir = opts
        .package
        .as_deref()
        .and_then(crate::dev::find_manifest_dir)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let profile = effective_profile(opts.release);
    let (prestop, shutdown) = resolve_shutdown_budget(&base_dir, Some(&profile));
    prestop + shutdown
}

/// The active profile: an explicit `AUTUMN_ENV`/`AUTUMN_PROFILE`, else the app's
/// build-mode default (`prod` for a release build, else `dev`).
fn effective_profile(release: bool) -> String {
    env_profile().unwrap_or_else(|| {
        if release {
            "prod".to_owned()
        } else {
            "dev".to_owned()
        }
    })
}

/// The active profile selector from the environment (`AUTUMN_ENV`, then the
/// legacy `AUTUMN_PROFILE`), if set to a non-empty value.
fn env_profile() -> Option<String> {
    std::env::var("AUTUMN_ENV")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var("AUTUMN_PROFILE")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
}

/// Resolve `(prestop_grace_secs, shutdown_timeout_secs)` with the app's layering
/// for the given active `profile`. Defaults match the prod/dev profile
/// smart-defaults for these keys.
fn resolve_shutdown_budget(base_dir: &Path, profile: Option<&str>) -> (u64, u64) {
    let mut prestop = 5u64;
    let mut shutdown = 30u64;

    // Base autumn.toml [server], then inline [profile.<name>].server overrides.
    if let Ok(contents) = std::fs::read_to_string(base_dir.join("autumn.toml"))
        && let Ok(table) = toml::from_str::<toml::Table>(&contents)
    {
        apply_server_slice(table.get("server"), &mut prestop, &mut shutdown);
        if let Some(prof) = profile {
            for name in profile_aliases(prof) {
                let section = table
                    .get("profile")
                    .and_then(|p| p.get(name.as_str()))
                    .and_then(|p| p.get("server"));
                apply_server_slice(section, &mut prestop, &mut shutdown);
            }
        }
    }

    // autumn-<profile>.toml [server] overrides. Only the first existing file is
    // loaded, in the app loader's preference order (the explicitly-selected
    // spelling first, else the canonical name), so we match the daemon's budget.
    if let Some(prof) = profile {
        for name in profile_file_lookup(prof) {
            if let Ok(contents) =
                std::fs::read_to_string(base_dir.join(format!("autumn-{name}.toml")))
                && let Ok(table) = toml::from_str::<toml::Table>(&contents)
            {
                apply_server_slice(table.get("server"), &mut prestop, &mut shutdown);
                break;
            }
        }
    }

    // Env overrides win (highest priority; read identically by the daemon).
    if let Some(v) = env_u64("AUTUMN_SERVER__PRESTOP_GRACE_SECS") {
        prestop = v;
    }
    if let Some(v) = env_u64("AUTUMN_SERVER__SHUTDOWN_TIMEOUT_SECS") {
        shutdown = v;
    }

    (prestop, shutdown)
}

/// Overlay `prestop_grace_secs`/`shutdown_timeout_secs` from a `[server]` slice.
fn apply_server_slice(server: Option<&toml::Value>, prestop: &mut u64, shutdown: &mut u64) {
    let Some(server) = server else { return };
    if let Some(v) = server
        .get("prestop_grace_secs")
        .and_then(toml::Value::as_integer)
        .and_then(|v| u64::try_from(v).ok())
    {
        *prestop = v;
    }
    if let Some(v) = server
        .get("shutdown_timeout_secs")
        .and_then(toml::Value::as_integer)
        .and_then(|v| u64::try_from(v).ok())
    {
        *shutdown = v;
    }
}

/// Inline `[profile.<name>]` section names plus legacy aliases, matching the
/// app's `profile_lookup_names` (all matching sections are merged in order, so
/// the canonical spelling, applied last, wins).
fn profile_aliases(profile: &str) -> Vec<String> {
    match profile.trim().to_ascii_lowercase().as_str() {
        "prod" | "production" => vec!["production".to_owned(), "prod".to_owned()],
        "dev" | "development" => vec!["development".to_owned(), "dev".to_owned()],
        _ => vec![profile.trim().to_owned()],
    }
}

/// Ordered `autumn-<name>.toml` lookup names, mirroring the app's
/// `profile_override_file_lookup_names`: only the first existing file is loaded,
/// preferring the explicitly-selected spelling, else the canonical name first.
fn profile_file_lookup(raw_profile: &str) -> Vec<String> {
    let raw = raw_profile.trim();
    match raw.to_ascii_lowercase().as_str() {
        "production" => vec!["production".to_owned(), "prod".to_owned()],
        "prod" => vec!["prod".to_owned(), "production".to_owned()],
        "development" => vec!["development".to_owned(), "dev".to_owned()],
        "dev" => vec!["dev".to_owned(), "development".to_owned()],
        _ => vec![raw.to_owned()],
    }
}

/// Parse a `u64` env var, ignoring empty or invalid values.
fn env_u64(key: &str) -> Option<u64> {
    std::env::var(key).ok()?.trim().parse::<u64>().ok()
}

/// Report daemon status. Exit code 0 = running, 3 = stopped.
fn status(opts: &ServeOptions) -> i32 {
    let paths = resolve_paths(opts.package.as_deref());
    let socket = paths.socket_file();
    let Some(rec) = lifecycle_target(&paths, &socket) else {
        println!("autumn serve: stopped");
        return 3;
    };
    if confirmed_running(&rec, &socket) {
        let address =
            read_addr_file(&paths).map_or_else(|| socket.display().to_string(), |a| a.address);
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
    remove_socket_if_not_live(socket);
}

/// Unlink the socket file only when it is a stale socket we can safely reclaim.
///
/// Two guards mirror the app's own bind-path policy:
/// - Skip a path that is **not a socket** (a regular file or other type the app
///   would refuse to clobber) so we never delete something we didn't create.
/// - Skip a socket still owned by a **live listener** (a `connect` succeeds) —
///   e.g. a foreign daemon, or our child that refused to bind over it and exited
///   — so we don't make that service unreachable.
///
/// After our own daemon exits its socket is a dead socket file (connect refused)
/// and is removed.
#[cfg(unix)]
fn remove_socket_if_not_live(socket: &Path) {
    use std::os::unix::fs::FileTypeExt;
    match std::fs::symlink_metadata(socket) {
        Ok(meta) if meta.file_type().is_socket() => {
            if std::os::unix::net::UnixStream::connect(socket).is_ok() {
                return; // live listener owns it
            }
            let _ = std::fs::remove_file(socket);
        }
        // Not a socket (or missing): leave it untouched.
        _ => {}
    }
}

#[cfg(not(unix))]
fn remove_socket_if_not_live(_socket: &Path) {}

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
            release: false,
            stop_budget_secs: Some(35),
        }
    }

    #[test]
    fn addr_file_release_defaults_false_for_legacy_files() {
        // Address files written before the `release` field omit it; parsing must
        // not fail and must default to false.
        let legacy = "pid = 7\ntransport = \"unix\"\naddress = \"/tmp/s.sock\"\n\
                      started_at = 1700000000\nmanaged_pg = false\n";
        let parsed = AddrFile::parse(legacy).expect("parse legacy addr file");
        assert!(!parsed.release);
    }

    #[test]
    fn addr_file_release_roundtrips() {
        let mut a = sample("unix", "/tmp/s.sock", true);
        a.release = true;
        let parsed = AddrFile::parse(&a.to_toml()).expect("parse");
        assert!(parsed.release);
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
        for key in [
            "pid",
            "transport",
            "address",
            "started_at",
            "managed_pg",
            "release",
            "stop_budget_secs",
        ] {
            assert!(table.contains_key(key), "missing key {key}");
        }
    }

    #[test]
    fn addr_file_stop_budget_defaults_none_for_legacy_files() {
        // Files written before the field omit it; parsing must default to None.
        let legacy = "pid = 7\ntransport = \"unix\"\naddress = \"/tmp/s.sock\"\n\
                      started_at = 1700000000\nmanaged_pg = false\nrelease = false\n";
        let parsed = AddrFile::parse(legacy).expect("parse legacy addr file");
        assert_eq!(parsed.stop_budget_secs, None);
    }

    #[test]
    fn project_identity_prefers_explicit_package() {
        let id = project_identity(Some("my-svc"));
        // Base name from the explicit package, plus a project-dir hash suffix
        // so unrelated checkouts with the same package name don't collide.
        assert!(id.starts_with("my-svc-"), "got {id}");
        assert!(id.len() > "my-svc-".len());
    }

    // Env vars touched by these tests; cleared so a polluted outer environment
    // can't skew the budget resolution.
    const SHUTDOWN_ENV: [&str; 4] = [
        "AUTUMN_ENV",
        "AUTUMN_PROFILE",
        "AUTUMN_SERVER__PRESTOP_GRACE_SECS",
        "AUTUMN_SERVER__SHUTDOWN_TIMEOUT_SECS",
    ];

    fn clear_shutdown_env() -> Vec<(&'static str, Option<&'static str>)> {
        SHUTDOWN_ENV.iter().map(|k| (*k, None)).collect()
    }

    #[test]
    fn shutdown_budget_defaults_when_no_config() {
        let dir = tempfile::tempdir().expect("tempdir");
        temp_env::with_vars(clear_shutdown_env(), || {
            assert_eq!(resolve_shutdown_budget(dir.path(), None), (5, 30));
        });
    }

    #[test]
    fn shutdown_budget_reads_base_server_section() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("autumn.toml"),
            "[server]\nprestop_grace_secs = 10\nshutdown_timeout_secs = 100\n",
        )
        .expect("write");
        temp_env::with_vars(clear_shutdown_env(), || {
            assert_eq!(resolve_shutdown_budget(dir.path(), None), (10, 100));
        });
    }

    #[test]
    fn shutdown_budget_env_override_wins() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("autumn.toml"),
            "[server]\nprestop_grace_secs = 10\nshutdown_timeout_secs = 100\n",
        )
        .expect("write");
        let mut vars = clear_shutdown_env();
        vars.push(("AUTUMN_SERVER__SHUTDOWN_TIMEOUT_SECS", Some("7")));
        temp_env::with_vars(vars, || {
            // Env wins for shutdown; prestop still comes from the file.
            assert_eq!(resolve_shutdown_budget(dir.path(), None), (10, 7));
        });
    }

    #[test]
    fn shutdown_budget_layers_profile_file_and_inline_section() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("autumn.toml"),
            "[server]\nprestop_grace_secs = 10\nshutdown_timeout_secs = 100\n\
             [profile.prod.server]\nprestop_grace_secs = 15\n",
        )
        .expect("write");
        std::fs::write(
            dir.path().join("autumn-prod.toml"),
            "[server]\nshutdown_timeout_secs = 200\n",
        )
        .expect("write");
        temp_env::with_vars(clear_shutdown_env(), || {
            // prestop from inline [profile.prod], shutdown from autumn-prod.toml.
            assert_eq!(resolve_shutdown_budget(dir.path(), Some("prod")), (15, 200));
        });
    }

    #[test]
    fn profile_aliases_cover_canonical_and_custom() {
        assert_eq!(profile_aliases("production"), vec!["production", "prod"]);
        assert_eq!(profile_aliases("DEV"), vec!["development", "dev"]);
        assert_eq!(profile_aliases("staging"), vec!["staging"]);
    }

    #[test]
    fn profile_file_lookup_prefers_selected_spelling() {
        // Mirrors the app loader: the canonical `prod` is preferred unless the
        // user explicitly selected the `production` spelling.
        assert_eq!(profile_file_lookup("prod"), vec!["prod", "production"]);
        assert_eq!(
            profile_file_lookup("production"),
            vec!["production", "prod"]
        );
        assert_eq!(profile_file_lookup("dev"), vec!["dev", "development"]);
        assert_eq!(profile_file_lookup("staging"), vec!["staging"]);
    }

    #[test]
    fn shutdown_budget_profile_file_prefers_prod_over_production() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("autumn.toml"),
            "[server]\nshutdown_timeout_secs = 30\n",
        )
        .expect("write");
        // Both spellings present; with AUTUMN_ENV=prod the app loads autumn-prod
        // first, so our budget must too.
        std::fs::write(
            dir.path().join("autumn-prod.toml"),
            "[server]\nshutdown_timeout_secs = 111\n",
        )
        .expect("write");
        std::fs::write(
            dir.path().join("autumn-production.toml"),
            "[server]\nshutdown_timeout_secs = 222\n",
        )
        .expect("write");
        temp_env::with_vars(clear_shutdown_env(), || {
            assert_eq!(resolve_shutdown_budget(dir.path(), Some("prod")).1, 111);
        });
    }

    #[cfg(unix)]
    #[test]
    fn confirmed_running_false_for_dead_pid() {
        let rec = process::PidRecord {
            pid: 2_147_483_640,
            start_time: None,
        };
        assert!(!confirmed_running(
            &rec,
            std::path::Path::new("/nonexistent/serve.sock")
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn confirmed_running_true_for_self_via_start_time() {
        // A matching start time is conclusive without needing a live socket.
        let pid = std::process::id();
        let rec = process::PidRecord {
            pid,
            start_time: process::process_start_time(pid),
        };
        assert!(confirmed_running(
            &rec,
            std::path::Path::new("/nonexistent/serve.sock")
        ));
    }

    #[test]
    fn lifecycle_target_prefers_pidfile() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = RuntimePaths::from_base(dir.path(), "p");
        paths.ensure_dirs().expect("dirs");
        std::fs::write(paths.pid_file(), "4242 0\n").expect("write pid");
        let rec = lifecycle_target(&paths, &paths.socket_file()).expect("target");
        assert_eq!(rec.pid, 4242);
    }

    #[test]
    fn lifecycle_target_addr_fallback_requires_live_socket() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = RuntimePaths::from_base(dir.path(), "p");
        paths.ensure_dirs().expect("dirs");
        // Address file present, pidfile absent, and no live listener on the
        // socket: we must not treat the recorded PID as the daemon.
        let addr = sample("unix", &paths.socket_file().display().to_string(), false);
        std::fs::write(paths.addr_file(), addr.to_toml()).expect("write addr");
        assert!(lifecycle_target(&paths, &paths.socket_file()).is_none());
    }
}
