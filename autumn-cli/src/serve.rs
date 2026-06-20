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
use std::path::{Path, PathBuf};
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
    /// Profile to force on the spawned app via `AUTUMN_ENV`. `None` for a normal
    /// start (the child inherits the shell's environment); set by `restart` to
    /// restore the original daemon's profile when the restart shell doesn't have
    /// one.
    pub profile: Option<String>,
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
    /// The explicit profile (`AUTUMN_ENV`/`AUTUMN_PROFILE`) the daemon was
    /// started with, recorded so `restart` can restore it. `None` when the daemon
    /// relied on the build-mode default (or for older address files).
    #[serde(default)]
    pub profile: Option<String>,
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
            // Preserve the original daemon's profile: prefer one set on *this*
            // restart's environment, else restore what the running daemon
            // recorded, so a bare `restart` doesn't silently fall back to `dev`.
            let keep_profile =
                env_profile().or_else(|| running.as_ref().and_then(|a| a.profile.clone()));
            let _ = stop(opts);
            let daemon_opts = ServeOptions {
                daemon: true,
                bundled_pg: keep_managed,
                release: keep_release,
                profile: keep_profile,
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

/// The PID of the process listening on the Unix socket at `path`, via
/// `SO_PEERCRED`. `None` when it can't be determined — not connectable, or a
/// platform without a peer-PID syscall (macOS/BSD).
#[cfg(target_os = "linux")]
fn socket_owner_pid(path: &Path) -> Option<u32> {
    let stream = std::os::unix::net::UnixStream::connect(path).ok()?;
    let cred =
        nix::sys::socket::getsockopt(&stream, nix::sys::socket::sockopt::PeerCredentials).ok()?;
    u32::try_from(cred.pid()).ok()
}

#[cfg(not(target_os = "linux"))]
fn socket_owner_pid(_path: &Path) -> Option<u32> {
    None
}

/// Best-effort check that the daemon serving on `socket` is `pid`: a definitive
/// `SO_PEERCRED` match on Linux, otherwise socket liveness (so a reused PID that
/// is genuinely not the listener is rejected where the kernel can tell us).
fn socket_identity_matches(socket: &Path, pid: u32) -> bool {
    socket_owner_pid(socket).map_or_else(|| socket_is_live(socket), |owner| owner == pid)
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
    match (rec.start_time, process::process_start_time(rec.pid)) {
        // Identity is known on both sides (Linux): trust the comparison and do
        // *not* fall back to the socket — a reused PID with a different start
        // time is stale even if some daemon happens to be listening.
        (Some(recorded), Some(current)) => recorded == current,
        // Identity unknown (no recorded start time, or a platform like macOS
        // that can't report one): confirm the PID actually owns the socket
        // (SO_PEERCRED on Linux, else liveness), so a reused PID isn't accepted.
        _ => socket_identity_matches(socket, rec.pid),
    }
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
    // Trust the address-file PID only if it actually owns the socket (so a reused
    // PID isn't signalled); where peer-PID is unavailable, fall back to liveness.
    socket_identity_matches(socket, addr.pid).then_some(process::PidRecord {
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

/// A short, stable hash distinguishing same-named projects in different
/// locations.
///
/// For a workspace member selected with `-p`, the namespace is anchored to the
/// workspace root plus the package name, so it is identical whether the
/// lifecycle command runs from the workspace root or the member directory
/// (otherwise `start -p api` and `stop -p api` from different CWDs would target
/// different daemons). Falls back to the canonicalized CWD when no package is
/// given.
///
/// This is deliberately **metadata-free**: it walks the filesystem rather than
/// spawning `cargo metadata`, so `stop`/`status` resolve the same namespace as
/// `start` even when the toolchain is slow, offline, or unavailable — and a
/// daemon started under one resolution is never orphaned by a later one.
fn project_dir_hash(package: Option<&str>) -> String {
    use sha2::{Digest, Sha256};
    // NUL separates the two fields so no root path / package name pair can be
    // confused for another (paths cannot contain NUL).
    let key = package.map_or_else(
        || canonical_cwd().display().to_string(),
        |pkg| format!("{}\0{pkg}", identity_root().display()),
    );
    // A fixed SHA-256 over the key — `DefaultHasher` is explicitly not stable
    // across std/toolchain versions, so a CLI upgrade while a daemon is running
    // must not change the namespace (which would orphan it). First 4 bytes
    // (8 hex) keep the resulting Unix socket path within `sun_path`.
    hex::encode(&Sha256::digest(key.as_bytes())[..4])
}

/// The canonicalized current working directory, falling back to the raw cwd and
/// finally `"."` so this never fails.
fn canonical_cwd() -> PathBuf {
    std::env::current_dir()
        .ok()
        .and_then(|d| std::fs::canonicalize(&d).ok().or(Some(d)))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The directory that anchors a `-p <member>` namespace.
///
/// Walks ancestors of the CWD looking for the nearest `Cargo.toml` whose
/// manifest declares a `[workspace]` table — that workspace root is stable from
/// anywhere inside the tree. If none is found (a standalone crate), the nearest
/// ancestor containing any `Cargo.toml` is used; failing that, the CWD itself.
fn identity_root() -> PathBuf {
    let cwd = canonical_cwd();
    workspace_anchor_from(&cwd).unwrap_or(cwd)
}

/// Pure ancestor walk for [`identity_root`]: returns the workspace root (nearest
/// ancestor whose `Cargo.toml` has a `[workspace]` table), else the nearest
/// ancestor containing any `Cargo.toml`, else `None`. Separated from CWD lookup
/// so it can be unit-tested against a fixture tree.
fn workspace_anchor_from(start: &Path) -> Option<PathBuf> {
    let mut nearest_manifest: Option<PathBuf> = None;
    for dir in start.ancestors() {
        let Ok(contents) = std::fs::read_to_string(dir.join("Cargo.toml")) else {
            continue;
        };
        if nearest_manifest.is_none() {
            nearest_manifest = Some(dir.to_path_buf());
        }
        if toml::from_str::<toml::Table>(&contents).is_ok_and(|t| t.contains_key("workspace")) {
            return Some(dir.to_path_buf());
        }
    }
    nearest_manifest
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
        // The standard nested-config env var feeds the default loader. We also
        // set a dedicated out-of-band override the framework re-applies *after*
        // config loading, so a custom `with_config_loader` that ignores env
        // can't drop the daemon's socket and strand it on TCP.
        cmd.env("AUTUMN_SERVER__UNIX_SOCKET", paths.socket_file());
        cmd.env("AUTUMN_SERVE_FORCE_UNIX_SOCKET", paths.socket_file());
    }
    if opts.bundled_pg
        && let Some(paths) = paths
    {
        cmd.env("AUTUMN_MANAGED_PG_DATA_DIR", paths.pg_data_dir());
    }
    // Restore an explicit profile (set by `restart`) so the relaunched daemon
    // loads the same config as the original even when the restart shell didn't
    // set `AUTUMN_ENV`.
    if let Some(profile) = &opts.profile {
        cmd.env("AUTUMN_ENV", profile);
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

/// Launch the detached daemon child and record its pidfile, under a separate
/// startup lock. On success returns the running child plus the still-held
/// startup-lock path (the caller releases it only once the daemon is ready, so a
/// concurrent start can't race in during readiness). `Err(exit_code)` on failure
/// (after cleaning up and releasing the lock). The pidfile only ever holds the
/// child's pid, so concurrent lifecycle commands never see the launcher.
fn launch_daemon_child(
    binary: &Path,
    paths: &RuntimePaths,
    opts: &ServeOptions,
    socket: &Path,
) -> Result<(std::process::Child, PathBuf), i32> {
    // A live listener already owning the socket means a daemon is serving here
    // even if the pidfile is missing or stale; refuse so readiness can't latch
    // onto the pre-existing listener (mirrors the pidfile guard).
    #[cfg(unix)]
    if std::os::unix::net::UnixStream::connect(socket).is_ok() {
        eprintln!(
            "autumn serve: already running (a live server owns {}). \
             Use `autumn serve stop` or `autumn serve restart`.",
            socket.display()
        );
        return Err(1);
    }

    // Separate startup lock (claimed with our own pid) so two concurrent starts
    // can't both spawn — but it is NOT the pidfile, so a concurrent `stop`/
    // `status` during startup never signals the launcher.
    let start_lock = paths.pid_file().with_file_name("serve.startlock");
    match process::acquire_pidfile(&start_lock, std::process::id()) {
        Ok(()) => {}
        Err(AcquireError::AlreadyRunning(existing)) => {
            eprintln!("autumn serve: another `autumn serve` is starting (pid {existing}).");
            return Err(1);
        }
        Err(AcquireError::Io(e)) => {
            eprintln!("autumn serve: cannot take startup lock: {e}");
            return Err(1);
        }
    }
    let log = match create_private_log(&paths.log_file()) {
        Ok(f) => f,
        Err(e) => {
            eprintln!(
                "autumn serve: cannot open log file {}: {e}",
                paths.log_file().display()
            );
            let _ = std::fs::remove_file(&start_lock);
            cleanup(paths, socket);
            return Err(1);
        }
    };
    let Ok(log_err) = log.try_clone() else {
        eprintln!("autumn serve: cannot duplicate log handle");
        let _ = std::fs::remove_file(&start_lock);
        cleanup(paths, socket);
        return Err(1);
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
            let _ = std::fs::remove_file(&start_lock);
            cleanup(paths, socket);
            return Err(1);
        }
    };
    let pid = child.id();

    // The live-socket guard at the top proved no daemon is serving this socket,
    // and we hold the startup lock, so any existing pidfile is from a daemon
    // that has already exited. On platforms that don't record a process start
    // time (macOS/BSD, or an older pidfile), `acquire_pidfile` can't tell that
    // crashed daemon from an unrelated process that reused its PID, and would
    // reject the start as `AlreadyRunning` — permanently blocking restart until
    // the pidfile is deleted by hand. Since the socket is provably dead, clear
    // such an unverifiable pidfile so the acquire below can reclaim it.
    #[cfg(unix)]
    if process::read_pidfile(&paths.pid_file()).is_some_and(|r| r.start_time.is_none()) {
        let _ = std::fs::remove_file(paths.pid_file());
    }

    // Record the real pidfile (child's pid). Keep the startup lock held — the
    // caller releases it only after the daemon is ready, so a concurrent start
    // can't spawn a second child while this one is still binding.
    if let Err(e) = process::acquire_pidfile(&paths.pid_file(), pid) {
        match e {
            AcquireError::AlreadyRunning(existing) => {
                eprintln!("autumn serve: already running (pid {existing}).");
            }
            AcquireError::Io(e) => eprintln!("autumn serve: cannot write pidfile: {e}"),
        }
        let _ = child.kill();
        process::force_kill_group(pid);
        let _ = child.wait();
        let _ = std::fs::remove_file(&start_lock);
        cleanup(paths, socket);
        return Err(1);
    }
    Ok((child, start_lock))
}

/// Spawn the server detached into the background and supervise readiness.
fn spawn_daemon(binary: &Path, paths: &RuntimePaths, opts: &ServeOptions) -> i32 {
    let socket = paths.socket_file();
    let (mut child, start_lock) = match launch_daemon_child(binary, paths, opts, &socket) {
        Ok(launched) => launched,
        Err(code) => return code,
    };
    let pid = child.id();
    let log_path = paths.log_file();
    // The startup lock is held until the daemon is ready, then released here on
    // every path so a concurrent start can't race in during the bind window.
    let release_lock = || {
        let _ = std::fs::remove_file(&start_lock);
    };

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
            // Postgres `setsid`s out of the daemon's group, so the group kill
            // above can't reach it; reap it directly via `postmaster.pid`.
            if opts.bundled_pg {
                reap_managed_postgres(paths);
            }
            release_lock();
            cleanup(paths, &socket);
            return 1;
        }
        release_lock();
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
        // children that `Child::kill()` alone would leave orphaned.
        let _ = child.kill();
        process::force_kill_group(pid);
        let _ = child.wait();
        // A managed Postgres started during `--bundled-pg` provisioning leaves
        // the daemon's process group (it `setsid`s itself) and no `serve.addr`
        // was written for a later `stop` to consult, so the group kill above
        // can't reach it. Reap it directly via `postmaster.pid` so a readiness
        // timeout doesn't strand the cluster on the data dir/port.
        if opts.bundled_pg {
            reap_managed_postgres(paths);
        }
        release_lock();
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
        // The startup barrier 503s every request until the app finishes its
        // startup hooks/migrations (`mark_startup_complete`). Binding the socket
        // happens *before* that, so a bare `connect` would report success while
        // the app is still initializing. Only a non-503 response (barrier
        // lifted) counts as ready; `StillStarting` (503) and `Unreachable` (not
        // bound yet) both keep polling until the timeout.
        #[cfg(unix)]
        if matches!(probe_startup_over_socket(socket), ProbeOutcome::Ready) {
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

/// Result of probing the daemon's startup state over its Unix socket.
#[cfg(unix)]
enum ProbeOutcome {
    /// The socket isn't accepting connections yet (app not bound).
    Unreachable,
    /// Connected and the startup barrier is still returning 503.
    StillStarting,
    /// Connected and the app responded with a non-503 status — startup is
    /// complete (or the socket speaks something other than the barrier, in
    /// which case socket liveness is the best signal available).
    Ready,
}

/// Issue a minimal HTTP/1.1 request over the daemon's Unix socket and classify
/// the response by status line.
///
/// Autumn mounts a startup barrier that returns `503 Service Unavailable` for
/// every path until startup completes; we deliberately request a path that is
/// not a real route nor a health probe, so before completion we see the
/// barrier's 503 and after completion a cheap `404` (no side effects, no DB
/// hit). A reply that can't be parsed as HTTP is treated as `Ready` so a daemon
/// that for any reason isn't speaking the barrier still falls back to socket
/// liveness rather than hanging until timeout.
#[cfg(unix)]
fn probe_startup_over_socket(socket: &Path) -> ProbeOutcome {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    let Ok(mut stream) = UnixStream::connect(socket) else {
        return ProbeOutcome::Unreachable;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let request = "GET /.autumn-serve-readiness HTTP/1.1\r\n\
         Host: localhost\r\n\
         Connection: close\r\n\r\n";
    if stream.write_all(request.as_bytes()).is_err() {
        // Connected but the write failed before any response — listener is up
        // but not serving yet; poll again.
        return ProbeOutcome::StillStarting;
    }
    // The status line is the first line of the response; 64 bytes covers
    // `HTTP/1.1 NNN <reason>` comfortably without reading the whole body.
    let mut buf = [0u8; 64];
    let mut read = 0;
    while read < buf.len() {
        match stream.read(&mut buf[read..]) {
            Ok(n) if n > 0 => {
                read += n;
                if buf[..read].contains(&b'\n') {
                    break;
                }
            }
            // EOF (`Ok(0)`) or a read error: stop with whatever we have.
            _ => break,
        }
    }
    // 503 is the startup barrier (still initializing); any other status means
    // it lifted. A `None` (no parseable status line) falls back to socket
    // liveness so we never hang waiting on a daemon that isn't the barrier.
    if parse_http_status(&buf[..read]) == Some(503) {
        ProbeOutcome::StillStarting
    } else {
        ProbeOutcome::Ready
    }
}

/// Parse the numeric status code out of an HTTP/1.x status line
/// (`HTTP/1.1 503 ...`). Returns `None` if the bytes don't look like one.
#[cfg(unix)]
fn parse_http_status(bytes: &[u8]) -> Option<u16> {
    let line = std::str::from_utf8(bytes).ok()?;
    let mut parts = line.split_whitespace();
    let version = parts.next()?;
    if !version.starts_with("HTTP/") {
        return None;
    }
    parts.next()?.parse::<u16>().ok()
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
        // Record the explicit profile (a `restart` override, else this shell's
        // env) so a later `restart` can restore it.
        profile: opts.profile.clone().or_else(env_profile),
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
    // Read the address file up front: even when the app PID is stale, a managed
    // cluster it launched can still be alive (Postgres `setsid`s out of the
    // daemon's process group, so it survives the app's crash/kill).
    let addr = read_addr_file(&paths);
    if !confirmed_running(&rec, &socket) {
        // The daemon is gone, but if it owned a managed cluster reap that too —
        // otherwise a crash before `stop` leaves Postgres holding the data
        // dir/port until manual cleanup.
        if addr.as_ref().is_some_and(|a| a.managed_pg) {
            reap_managed_postgres(&paths);
        }
        println!("autumn serve: not running (removed stale files)");
        cleanup(&paths, &socket);
        return 0;
    }

    // Prefer the budget the daemon recorded at start (resolved from its own
    // env/profile); fall back to recomputing it (release flag + config) only for
    // daemons started before that field was recorded.
    let recorded_release = addr.as_ref().is_some_and(|a| a.release);
    let recorded_budget = addr.as_ref().and_then(|a| a.stop_budget_secs);
    process::stop_record(&rec, stop_timeout(opts, recorded_release, recorded_budget));
    // For a managed-Postgres daemon, reap the cluster too: the app's `on_shutdown`
    // hook can be cancelled when its drain budget is exhausted (or skipped on a
    // forced exit), and Postgres `setsid`s itself so a process-group kill won't
    // reach it. Stop it directly via its `postmaster.pid` so it doesn't keep
    // holding the data dir/port after `stop` reports success.
    if addr.as_ref().is_some_and(|a| a.managed_pg) {
        reap_managed_postgres(&paths);
    }
    cleanup(&paths, &socket);
    println!("autumn serve: stopped");
    0
}

/// Reap a managed Postgres cluster the daemon may have left running. Reads the
/// postmaster pid from the data dir's `postmaster.pid` and, if it's still alive,
/// requests a fast shutdown and escalates. No-op when the cluster already
/// stopped cleanly (pidfile absent or the postmaster is gone).
fn reap_managed_postgres(paths: &RuntimePaths) {
    let pidfile = paths.pg_data_dir().join("postmaster.pid");
    let Some(pid) = std::fs::read_to_string(&pidfile)
        .ok()
        .and_then(|s| s.lines().next()?.trim().parse::<u32>().ok())
    else {
        return;
    };
    if process::is_process_alive(pid) {
        process::stop_postmaster(pid, Duration::from_secs(10));
    }
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
    let profile = effective_profile(None, recorded_release);
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
    let profile = effective_profile(opts.profile.as_deref(), opts.release);
    let (prestop, shutdown) = resolve_shutdown_budget(&base_dir, Some(&profile));
    prestop + shutdown
}

/// The active profile: an explicit override (`profile_override`, e.g. a `restart`
/// restoration), then the environment (`AUTUMN_ENV`/`AUTUMN_PROFILE`), then the
/// app's build-mode default (`prod` for a release build, else `dev`).
fn effective_profile(profile_override: Option<&str>, release: bool) -> String {
    profile_override
        .map(ToOwned::to_owned)
        .or_else(env_profile)
        .unwrap_or_else(|| {
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
            profile: Some("dev".to_owned()),
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
            "profile",
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

    #[test]
    fn workspace_anchor_is_stable_from_root_and_member() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = std::fs::canonicalize(tmp.path()).expect("canonicalize root");
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/api\"]\n",
        )
        .expect("write workspace manifest");
        let member = root.join("crates/api");
        std::fs::create_dir_all(&member).expect("create member dir");
        std::fs::write(
            member.join("Cargo.toml"),
            "[package]\nname = \"api\"\nversion = \"0.1.0\"\n",
        )
        .expect("write member manifest");

        // The namespace anchor must be the workspace root whether resolved from
        // the root or from the member directory, so `start -p api` and
        // `stop -p api` from different CWDs target the same daemon.
        assert_eq!(workspace_anchor_from(&root), Some(root.clone()));
        assert_eq!(workspace_anchor_from(&member), Some(root));
    }

    #[cfg(unix)]
    #[test]
    fn parse_http_status_reads_status_code() {
        assert_eq!(
            parse_http_status(b"HTTP/1.1 503 Service Unavailable\r\n"),
            Some(503)
        );
        assert_eq!(parse_http_status(b"HTTP/1.0 404 Not Found\r\n"), Some(404));
        assert_eq!(parse_http_status(b"HTTP/1.1 200 OK\r\n"), Some(200));
        // Non-HTTP / truncated input yields no status so the caller falls back
        // to socket liveness instead of misreading it as "still starting".
        assert_eq!(parse_http_status(b"garbage"), None);
        assert_eq!(parse_http_status(b""), None);
        assert_eq!(parse_http_status(b"HTTP/1.1 \r\n"), None);
    }

    #[test]
    fn workspace_anchor_falls_back_to_nearest_manifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = std::fs::canonicalize(tmp.path()).expect("canonicalize root");
        // A standalone crate with no `[workspace]` table anywhere above.
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"solo\"\nversion = \"0.1.0\"\n",
        )
        .expect("write manifest");
        let nested = root.join("src");
        std::fs::create_dir_all(&nested).expect("create nested dir");
        assert_eq!(workspace_anchor_from(&nested), Some(root));
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

    #[test]
    fn effective_profile_prefers_override_then_env_then_build_mode() {
        temp_env::with_vars(
            [("AUTUMN_ENV", None::<&str>), ("AUTUMN_PROFILE", None)],
            || {
                assert_eq!(effective_profile(Some("staging"), false), "staging");
                assert_eq!(effective_profile(None, true), "prod");
                assert_eq!(effective_profile(None, false), "dev");
            },
        );
        temp_env::with_vars(
            [("AUTUMN_ENV", Some("qa")), ("AUTUMN_PROFILE", None::<&str>)],
            || {
                // Env beats the build-mode default; an explicit override beats env.
                assert_eq!(effective_profile(None, true), "qa");
                assert_eq!(effective_profile(Some("explicit"), true), "explicit");
            },
        );
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
