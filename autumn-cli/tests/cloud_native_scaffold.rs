use std::fs;
use std::process::Command;

fn scaffold(project_name: &str) -> tempfile::TempDir {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let autumn_bin = env!("CARGO_BIN_EXE_autumn");

    let output = Command::new(autumn_bin)
        .args(["new", project_name])
        .current_dir(temp_dir.path())
        .output()
        .expect("failed to run `autumn new`");

    assert!(
        output.status.success(),
        "autumn new failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    temp_dir
}

#[test]
fn cloud_native_scaffold_emits_container_artifacts() {
    let temp_dir = scaffold("cloudy-app");
    let project_dir = temp_dir.path().join("cloudy-app");

    assert!(project_dir.join("Dockerfile").is_file());
    assert!(project_dir.join(".dockerignore").is_file());
}

#[test]
fn cloud_native_scaffold_includes_probe_and_telemetry_examples() {
    let temp_dir = scaffold("ops-app");
    let config = fs::read_to_string(temp_dir.path().join("ops-app/autumn.toml")).unwrap();

    assert!(config.contains(r#"# live_path = "/live""#));
    assert!(config.contains(r#"# ready_path = "/ready""#));
    assert!(config.contains(r#"# startup_path = "/startup""#));
    assert!(config.contains("# [telemetry]"));
    assert!(config.contains(r#"# enabled = true"#));
    assert!(config.contains(r#"# otlp_endpoint = "http://otel-collector:4317""#));
}

#[test]
fn cloud_native_scaffold_dockerfile_is_production_ready() {
    let temp_dir = scaffold("container-app");
    let project_dir = temp_dir.path().join("container-app");
    let dockerfile = fs::read_to_string(project_dir.join("Dockerfile")).unwrap();
    let dockerignore = fs::read_to_string(project_dir.join(".dockerignore")).unwrap();

    assert!(dockerfile.contains("FROM rust:1.86-bookworm AS builder"));
    assert!(dockerfile.contains("FROM debian:bookworm-slim AS runtime"));
    assert!(dockerfile.contains("curl -fsSL"));
    assert!(dockerfile.contains("target/autumn/tailwindcss"));
    assert!(dockerfile.contains("cargo build --release"));
    assert!(dockerfile.contains("COPY --from=builder /app/target/release/container-app"));
    assert!(dockerfile.contains("USER autumn"));
    assert!(dockerfile.contains(r#"CMD ["container-app"]"#));

    assert!(dockerignore.contains("/target"));
    assert!(dockerignore.contains("/.git"));
    assert!(dockerignore.contains("static/css/autumn.css"));
}
