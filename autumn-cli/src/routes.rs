//! `autumn routes` -- list mounted routes without booting the dev server.
//!
//! Compiles the target binary (debug profile), runs it with
//! `AUTUMN_DUMP_ROUTES=1`, and parses the JSON route listing from its
//! stdout. Applies any user-requested filters, then displays the result
//! as either a human-readable table or machine-readable JSON.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

/// Output format for `autumn routes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputFormat {
    Table,
    Json,
}

impl std::str::FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "table" => Ok(Self::Table),
            "json" => Ok(Self::Json),
            other => Err(format!(
                "unknown format '{other}'; expected 'table' or 'json'"
            )),
        }
    }
}

/// Deserialized route entry received from the binary's JSON output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteInfo {
    pub method: String,
    pub path: String,
    pub handler: String,
    pub source: String,
    pub middleware: Vec<String>,
}

/// Options controlling `autumn routes` behaviour.
pub struct RoutesOptions<'a> {
    pub package: Option<&'a str>,
    pub format: OutputFormat,
    pub filter: Option<&'a str>,
    pub methods: &'a [String],
    pub user_only: bool,
}

/// Run `autumn routes`.
pub fn run(opts: &RoutesOptions<'_>) {
    eprintln!("\u{1F342} autumn routes\n");
    compile_binary(opts.package);
    let binary = find_binary(opts.package);

    let output = Command::new(&binary)
        .env("AUTUMN_DUMP_ROUTES", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .unwrap_or_else(|e| {
            eprintln!("\u{2717} Failed to run {}: {e}", binary.display());
            std::process::exit(1);
        });

    if !output.status.success() {
        eprintln!(
            "\u{2717} Binary exited with status {} while dumping routes",
            output.status
        );
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut routes: Vec<RouteInfo> = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        eprintln!("\u{2717} Failed to parse route listing JSON: {e}");
        eprintln!("Raw output: {stdout}");
        std::process::exit(1);
    });

    // Apply filters and sort
    routes = apply_filters(routes, opts.filter, opts.methods, opts.user_only);
    sort_routes(&mut routes);

    match &opts.format {
        OutputFormat::Table => print_table(&routes),
        OutputFormat::Json => print_json(&routes),
    }
}

/// Filter routes by path prefix, HTTP methods, and/or source.
pub fn apply_filters(
    routes: Vec<RouteInfo>,
    filter: Option<&str>,
    methods: &[String],
    user_only: bool,
) -> Vec<RouteInfo> {
    routes
        .into_iter()
        .filter(|r| {
            if filter.is_some_and(|prefix| !r.path.starts_with(prefix)) {
                return false;
            }
            if !methods.is_empty() && !methods.iter().any(|m| m.eq_ignore_ascii_case(&r.method)) {
                return false;
            }
            if user_only && r.source == "framework" {
                return false;
            }
            true
        })
        .collect()
}

/// Sort routes by path (lexicographic) then method (lexicographic).
pub fn sort_routes(routes: &mut [RouteInfo]) {
    routes.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.method.cmp(&b.method)));
}

/// Print routes as a human-readable aligned table.
pub fn print_table(routes: &[RouteInfo]) {
    if routes.is_empty() {
        println!("No routes found.");
        return;
    }

    let table = format_table(routes);
    print!("{table}");
}

/// Build the table string (extracted for testability).
pub fn format_table(routes: &[RouteInfo]) -> String {
    const HEADERS: [&str; 5] = ["Method", "Path", "Handler", "Source", "Middleware"];

    // Compute column widths
    let widths = compute_column_widths(routes, &HEADERS);

    let mut out = String::new();

    // Header row
    out.push_str(&format_row(
        &HEADERS.map(std::borrow::ToOwned::to_owned),
        &widths,
    ));
    out.push('\n');

    // Separator row
    for (i, &w) in widths.iter().enumerate() {
        if i > 0 {
            out.push_str("  ");
        }
        out.push_str(&"-".repeat(w));
    }
    out.push('\n');

    // Data rows
    for route in routes {
        let middleware = if route.middleware.is_empty() {
            String::new()
        } else {
            route.middleware.join(", ")
        };
        let cells = [
            route.method.clone(),
            route.path.clone(),
            route.handler.clone(),
            route.source.clone(),
            middleware,
        ];
        out.push_str(&format_row(&cells, &widths));
        out.push('\n');
    }

    out
}

fn compute_column_widths(routes: &[RouteInfo], headers: &[&str; 5]) -> [usize; 5] {
    let mut widths = [0usize; 5];
    for (i, h) in headers.iter().enumerate() {
        widths[i] = h.len();
    }
    for route in routes {
        let middleware = if route.middleware.is_empty() {
            String::new()
        } else {
            route.middleware.join(", ")
        };
        let cols = [
            route.method.len(),
            route.path.len(),
            route.handler.len(),
            route.source.len(),
            middleware.len(),
        ];
        for (i, &w) in cols.iter().enumerate() {
            if w > widths[i] {
                widths[i] = w;
            }
        }
    }
    widths
}

fn format_row(cells: &[String; 5], widths: &[usize; 5]) -> String {
    cells
        .iter()
        .zip(widths.iter())
        .enumerate()
        .map(|(i, (cell, &w))| {
            if i == 0 {
                format!("{cell:<w$}")
            } else {
                format!("  {cell:<w$}")
            }
        })
        .collect::<String>()
        .trim_end()
        .to_owned()
}

/// Print routes as pretty JSON.
pub fn print_json(routes: &[RouteInfo]) {
    let json =
        serde_json::to_string_pretty(routes).unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}"));
    println!("{json}");
}

// ── Binary discovery (mirrored from build.rs) ──────────────────────────────

fn find_binary(package: Option<&str>) -> PathBuf {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version=1", "--no-deps"])
        .output()
        .expect("failed to run cargo metadata");

    if !output.status.success() {
        eprintln!("\u{2717} Failed to read cargo metadata");
        std::process::exit(1);
    }

    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse cargo metadata");
    let cwd = std::env::current_dir().expect("current dir");

    resolve_binary_from_metadata(&metadata, package, &cwd).unwrap_or_else(|error| {
        eprintln!("\u{2717} {error}");
        std::process::exit(1);
    })
}

fn resolve_binary_from_metadata(
    metadata: &serde_json::Value,
    package: Option<&str>,
    cwd: &Path,
) -> Result<PathBuf, String> {
    let target_dir = metadata["target_directory"]
        .as_str()
        .ok_or("target_directory missing from cargo metadata")?;
    let packages = metadata["packages"]
        .as_array()
        .ok_or("packages missing from cargo metadata")?;

    let matching_packages: Vec<_> = package.map_or_else(
        || {
            packages
                .iter()
                .filter(|pkg| {
                    pkg["manifest_path"]
                        .as_str()
                        .and_then(|manifest| Path::new(manifest).parent())
                        .is_some_and(|dir| dir.starts_with(cwd))
                })
                .collect()
        },
        |pkg_name| {
            packages
                .iter()
                .filter(|pkg| pkg["name"].as_str() == Some(pkg_name))
                .collect()
        },
    );

    let bin_name = matching_packages
        .iter()
        .find_map(|pkg| {
            pkg["targets"].as_array()?.iter().find_map(|t| {
                let is_bin = t["kind"].as_array()?.iter().any(|k| k == "bin");
                if is_bin {
                    t["name"].as_str().map(String::from)
                } else {
                    None
                }
            })
        })
        .ok_or_else(|| {
            package.map_or_else(
                || "no binary target found in current package".to_owned(),
                |pkg_name| format!("no binary target found in package '{pkg_name}'"),
            )
        })?;

    let mut path = PathBuf::from(target_dir);
    path.push("debug");
    path.push(bin_name);

    if cfg!(windows) {
        path.set_extension("exe");
    }

    Ok(path)
}

// ── Also compile the binary before running ─────────────────────────────────

fn compile_binary(package: Option<&str>) {
    let mut cargo = Command::new("cargo");
    cargo.arg("build");
    if let Some(pkg) = package {
        cargo.args(["-p", pkg]);
    }

    let status = cargo.status().expect("failed to run cargo build");
    if !status.success() {
        eprintln!("\u{2717} Compilation failed");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_route(method: &str, path: &str, source: &str) -> RouteInfo {
        RouteInfo {
            method: method.to_owned(),
            path: path.to_owned(),
            handler: format!("{}_handler", path.trim_start_matches('/').replace('/', "_")),
            source: source.to_owned(),
            middleware: vec![],
        }
    }

    fn sample_routes() -> Vec<RouteInfo> {
        vec![
            make_route("GET", "/posts", "user"),
            make_route("POST", "/posts", "user"),
            make_route("GET", "/posts/{id}", "user"),
            make_route("GET", "/actuator/health", "framework"),
            make_route("GET", "/about", "user"),
            make_route("GET", "/api/posts", "plugin:harvest"),
        ]
    }

    // ── OutputFormat parsing ───────────────────────────────────────────────

    #[test]
    fn parse_format_table() {
        let f: OutputFormat = "table".parse().unwrap();
        assert_eq!(f, OutputFormat::Table);
    }

    #[test]
    fn parse_format_json() {
        let f: OutputFormat = "json".parse().unwrap();
        assert_eq!(f, OutputFormat::Json);
    }

    #[test]
    fn parse_format_case_insensitive() {
        let f: OutputFormat = "JSON".parse().unwrap();
        assert_eq!(f, OutputFormat::Json);
        let f: OutputFormat = "Table".parse().unwrap();
        assert_eq!(f, OutputFormat::Table);
    }

    #[test]
    fn parse_format_unknown_is_error() {
        let result: Result<OutputFormat, _> = "xml".parse();
        assert!(result.is_err());
    }

    // ── apply_filters ──────────────────────────────────────────────────────

    #[test]
    fn filter_no_constraints_returns_all() {
        let routes = sample_routes();
        let count = routes.len();
        let result = apply_filters(routes, None, &[], false);
        assert_eq!(result.len(), count);
    }

    #[test]
    fn filter_by_path_prefix() {
        let routes = sample_routes();
        let result = apply_filters(routes, Some("/posts"), &[], false);
        assert!(result.iter().all(|r| r.path.starts_with("/posts")));
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn filter_path_prefix_exact_match() {
        let routes = sample_routes();
        let result = apply_filters(routes, Some("/about"), &[], false);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].path, "/about");
    }

    #[test]
    fn filter_by_method_get() {
        let routes = sample_routes();
        let methods = vec!["GET".to_owned()];
        let result = apply_filters(routes, None, &methods, false);
        assert!(result.iter().all(|r| r.method == "GET"));
    }

    #[test]
    fn filter_by_method_post() {
        let routes = sample_routes();
        let methods = vec!["POST".to_owned()];
        let result = apply_filters(routes, None, &methods, false);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].method, "POST");
    }

    #[test]
    fn filter_by_multiple_methods() {
        let routes = sample_routes();
        let methods = vec!["GET".to_owned(), "POST".to_owned()];
        let result = apply_filters(routes, None, &methods, false);
        assert!(
            result
                .iter()
                .all(|r| r.method == "GET" || r.method == "POST")
        );
    }

    #[test]
    fn filter_method_case_insensitive() {
        let routes = sample_routes();
        let methods = vec!["get".to_owned()];
        let result = apply_filters(routes, None, &methods, false);
        assert!(!result.is_empty());
        assert!(result.iter().all(|r| r.method == "GET"));
    }

    #[test]
    fn filter_user_only_excludes_framework() {
        let routes = sample_routes();
        let result = apply_filters(routes, None, &[], true);
        assert!(result.iter().all(|r| r.source != "framework"));
    }

    #[test]
    fn filter_user_only_keeps_plugin_routes() {
        let routes = sample_routes();
        let result = apply_filters(routes, None, &[], true);
        assert!(
            result.iter().any(|r| r.source.starts_with("plugin:")),
            "plugin routes should be kept with --user-only"
        );
    }

    #[test]
    fn filter_combines_path_and_method() {
        let routes = sample_routes();
        let methods = vec!["GET".to_owned()];
        let result = apply_filters(routes, Some("/posts"), &methods, false);
        assert!(
            result
                .iter()
                .all(|r| r.path.starts_with("/posts") && r.method == "GET")
        );
        assert_eq!(result.len(), 2);
    }

    // ── sort_routes ────────────────────────────────────────────────────────

    #[test]
    fn sort_routes_by_path_then_method() {
        let mut routes = vec![
            make_route("POST", "/posts", "user"),
            make_route("GET", "/about", "user"),
            make_route("GET", "/posts", "user"),
        ];
        sort_routes(&mut routes);
        assert_eq!(routes[0].path, "/about");
        assert_eq!(routes[1].path, "/posts");
        assert_eq!(routes[1].method, "GET");
        assert_eq!(routes[2].path, "/posts");
        assert_eq!(routes[2].method, "POST");
    }

    // ── format_table ──────────────────────────────────────────────────────

    #[test]
    fn format_table_contains_headers() {
        let routes = vec![make_route("GET", "/posts", "user")];
        let table = format_table(&routes);
        assert!(table.contains("Method"), "missing Method header");
        assert!(table.contains("Path"), "missing Path header");
        assert!(table.contains("Handler"), "missing Handler header");
        assert!(table.contains("Source"), "missing Source header");
        assert!(table.contains("Middleware"), "missing Middleware header");
    }

    #[test]
    fn format_table_contains_route_data() {
        let routes = vec![make_route("GET", "/posts", "user")];
        let table = format_table(&routes);
        assert!(table.contains("GET"), "missing method");
        assert!(table.contains("/posts"), "missing path");
        assert!(table.contains("user"), "missing source");
    }

    #[test]
    fn format_table_has_separator_line() {
        let routes = vec![make_route("GET", "/", "user")];
        let table = format_table(&routes);
        assert!(table.contains("---"), "missing separator line");
    }

    #[test]
    fn format_table_empty_routes_still_has_headers() {
        let routes: Vec<RouteInfo> = vec![];
        let table = format_table(&routes);
        assert!(table.contains("Method"));
    }

    #[test]
    fn format_table_middleware_shown_when_present() {
        let route = RouteInfo {
            method: "GET".to_owned(),
            path: "/admin".to_owned(),
            handler: "admin".to_owned(),
            source: "user".to_owned(),
            middleware: vec!["secured".to_owned(), "cached(60s)".to_owned()],
        };
        let table = format_table(&[route]);
        assert!(table.contains("secured"), "missing middleware label");
        assert!(table.contains("cached(60s)"), "missing middleware label");
    }

    // ── print_json ─────────────────────────────────────────────────────────

    #[test]
    fn print_json_produces_valid_json() {
        let routes = sample_routes();
        let json_str = serde_json::to_string_pretty(&routes).unwrap();
        let parsed: Vec<RouteInfo> = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed.len(), routes.len());
    }

    // ── resolve_binary_from_metadata ──────────────────────────────────────

    #[test]
    fn resolve_binary_by_package_name() {
        let metadata = serde_json::json!({
            "target_directory": "/tmp/target",
            "packages": [{
                "name": "hello",
                "manifest_path": "/projects/hello/Cargo.toml",
                "targets": [{
                    "name": "hello",
                    "kind": ["bin"],
                    "src_path": "/projects/hello/src/main.rs"
                }]
            }]
        });
        let result = resolve_binary_from_metadata(&metadata, Some("hello"), Path::new("/projects"));
        let expected = if cfg!(windows) {
            PathBuf::from("/tmp/target/debug/hello.exe")
        } else {
            PathBuf::from("/tmp/target/debug/hello")
        };
        assert_eq!(result.unwrap(), expected);
    }

    #[test]
    fn resolve_binary_by_cwd() {
        let metadata = serde_json::json!({
            "target_directory": "/tmp/target",
            "packages": [{
                "name": "hello",
                "manifest_path": "/projects/hello/Cargo.toml",
                "targets": [{
                    "name": "hello",
                    "kind": ["bin"],
                    "src_path": "/projects/hello/src/main.rs"
                }]
            }]
        });
        let result = resolve_binary_from_metadata(&metadata, None, Path::new("/projects/hello"));
        let expected = if cfg!(windows) {
            PathBuf::from("/tmp/target/debug/hello.exe")
        } else {
            PathBuf::from("/tmp/target/debug/hello")
        };
        assert_eq!(result.unwrap(), expected);
    }

    #[test]
    fn resolve_binary_reports_missing_package() {
        let metadata = serde_json::json!({
            "target_directory": "/tmp/target",
            "packages": [{
                "name": "hello",
                "manifest_path": "/projects/hello/Cargo.toml",
                "targets": [{"name": "hello", "kind": ["bin"]}]
            }]
        });
        let result =
            resolve_binary_from_metadata(&metadata, Some("missing"), Path::new("/projects"));
        assert!(result.unwrap_err().contains("package 'missing'"));
    }
}
