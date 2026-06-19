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
    run_foreground_non_unix(opts)
}

/// Foreground server on non-Unix platforms: build, then run the app binary in
/// the foreground (it binds TCP per its config, inheriting this terminal so
/// Ctrl-C reaches it). None of the pidfile/socket/address-file machinery applies
/// here — that is part of the Unix daemon lifecycle.
#[cfg(not(unix))]
fn run_foreground_non_unix(opts: &ServeOptions) -> i32 {
    let paths = resolve_paths(opts.package.as_deref());
    if let Err(e) = paths.ensure_dirs() {
        eprintln!("autumn serve: cannot create runtime dirs: {e}");
        return 1;
    }
    eprintln!("\u{1F342} autumn serve\n");
    if !crate::dev::cargo_build(opts.package.as_deref(), opts.release) {
        eprintln!("\u{2717} Build failed. Fix the errors above and retry.");
        return 1;
    }
    let binary = crate::dev::find_binary(opts.package.as_deref(), opts.release);
    eprintln!("  Running {} in the foreground", binary.display());
    eprintln!("  Press Ctrl+C to stop\n");
    // `base_command` sets the managed-Postgres data dir when bundled; the
    // Unix-socket env is compiled out off-Unix, so the app binds TCP.
    match base_command(&binary, &paths, opts).status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(e) => {
            eprintln!("\u{2717} Failed to start {}: {e}", binary.display());
            1
        }
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
    use std::hash::{Hash, Hasher};
    let dir = package
        .and_then(crate::dev::find_manifest_dir)
        .or_else(|| std::env::current_dir().ok())
        .and_then(|d| std::fs::canonicalize(&d).ok().or(Some(d)))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
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
    options.open(path)
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

    process::stop_pid(rec.pid, stop_timeout(opts));
    cleanup(&paths, &socket);
    println!("autumn serve: stopped");
    0
}

/// Graceful-stop budget before escalating to `SIGKILL`, derived from the app's
/// own `[server]` shutdown config (`prestop_grace_secs` plus
/// `shutdown_timeout_secs` plus a small buffer) so we never cut off a drain the
/// server is configured to allow.
///
/// Mirrors the app's layering for these two keys: base `autumn.toml`
/// ← `[profile.<name>]` ← `autumn-<profile>.toml` ← `AUTUMN_SERVER__*` env. The
/// active profile is read from `AUTUMN_ENV`/`AUTUMN_PROFILE`; the app's
/// build-mode auto-detection (release ⇒ `prod`) isn't observable to a separate
/// `stop` invocation, so profile *file* layering applies only when the profile
/// is set explicitly. Env overrides — read identically by the daemon — always
/// apply, so a deployment configuring shutdown via `AUTUMN_SERVER__*` is honored.
/// Config is read from the same directory the daemon used (a `-p` member's
/// manifest dir, else the current directory).
fn stop_timeout(opts: &ServeOptions) -> Duration {
    let base_dir = opts
        .package
        .as_deref()
        .and_then(crate::dev::find_manifest_dir)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let (prestop, shutdown) = resolve_shutdown_budget(&base_dir);
    Duration::from_secs(prestop + shutdown) + STOP_GRACE_BUFFER
}

/// Resolve `(prestop_grace_secs, shutdown_timeout_secs)` with the app's layering.
/// Defaults match the prod/dev profile smart-defaults for these keys.
fn resolve_shutdown_budget(base_dir: &Path) -> (u64, u64) {
    let mut prestop = 5u64;
    let mut shutdown = 30u64;

    let profile = std::env::var("AUTUMN_ENV")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var("AUTUMN_PROFILE")
                .ok()
                .filter(|s| !s.trim().is_empty())
        });

    // Base autumn.toml [server], then inline [profile.<name>].server overrides.
    if let Ok(contents) = std::fs::read_to_string(base_dir.join("autumn.toml"))
        && let Ok(table) = toml::from_str::<toml::Table>(&contents)
    {
        apply_server_slice(table.get("server"), &mut prestop, &mut shutdown);
        if let Some(prof) = profile.as_deref() {
            for name in profile_aliases(prof) {
                let section = table
                    .get("profile")
                    .and_then(|p| p.get(name.as_str()))
                    .and_then(|p| p.get("server"));
                apply_server_slice(section, &mut prestop, &mut shutdown);
            }
        }
    }

    // autumn-<profile>.toml [server] overrides (first match wins).
    if let Some(prof) = profile.as_deref() {
        for name in profile_aliases(prof) {
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

/// Profile name plus legacy aliases, matching the app's lookup order.
fn profile_aliases(profile: &str) -> Vec<String> {
    match profile.trim().to_ascii_lowercase().as_str() {
        "prod" | "production" => vec!["production".to_owned(), "prod".to_owned()],
        "dev" | "development" => vec!["development".to_owned(), "dev".to_owned()],
        _ => vec![profile.trim().to_owned()],
    }
}

/// Parse a `u64` env var, ignoring empty or invalid values.
fn env_u64(key: &str) -> Option<u64> {
    std::env::var(key).ok()?.trim().parse::<u64>().ok()
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
    remove_socket_if_not_live(socket);
}

/// Unlink the socket file only when no live listener still owns it.
///
/// After our daemon exits the socket is stale (a `connect` is refused) and safe
/// to remove. But if a *foreign* live listener owns this path — e.g. our child
/// refused to bind over it (`prepare_unix_socket_path`) and exited — unlinking
/// would make that service unreachable, so leave it in place.
fn remove_socket_if_not_live(socket: &Path) {
    #[cfg(unix)]
    if std::os::unix::net::UnixStream::connect(socket).is_ok() {
        return;
    }
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
            assert_eq!(resolve_shutdown_budget(dir.path()), (5, 30));
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
            assert_eq!(resolve_shutdown_budget(dir.path()), (10, 100));
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
            assert_eq!(resolve_shutdown_budget(dir.path()), (10, 7));
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
        let mut vars = clear_shutdown_env();
        vars.push(("AUTUMN_ENV", Some("prod")));
        temp_env::with_vars(vars, || {
            // prestop from inline [profile.prod], shutdown from autumn-prod.toml.
            assert_eq!(resolve_shutdown_budget(dir.path()), (15, 200));
        });
    }

    #[test]
    fn profile_aliases_cover_canonical_and_custom() {
        assert_eq!(profile_aliases("production"), vec!["production", "prod"]);
        assert_eq!(profile_aliases("DEV"), vec!["development", "dev"]);
        assert_eq!(profile_aliases("staging"), vec!["staging"]);
    }
}
