mod build_support;

fn main() {
    println!("cargo:rerun-if-changed=src/");
    println!("cargo:rerun-if-changed=static/css/input.css");
    println!("cargo:rerun-if-changed=tailwind.config.js");
    println!("cargo:rerun-if-changed=static/css/autumn.css");
    println!("cargo:rerun-if-env-changed=AUTUMN_REQUIRE_TAILWIND");

    let Some(tailwind) = find_tailwind_cli() else {
        return;
    };

    let output = std::process::Command::new(&tailwind)
        .args([
            "-i",
            "static/css/input.css",
            "-o",
            "static/css/autumn.css",
            "--content",
            "src/**/*.rs",
            "--minify",
        ])
        .output();

    match output {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            handle_tailwind_unavailable(&format!(
                "Tailwind CSS CLI exited with status {}: {stderr}",
                output.status
            ));
        }
        Err(error) => {
            handle_tailwind_unavailable(&format!("failed to run Tailwind CSS CLI: {error}"));
        }
    }
}

fn handle_tailwind_unavailable(reason: &str) {
    let require_tailwind = build_support::require_tailwind_from_env();
    match build_support::tailwind_failure_action(require_tailwind) {
        build_support::TailwindFailureAction::FailBuild => {
            panic!("{reason}; AUTUMN_REQUIRE_TAILWIND is set");
        }
        build_support::TailwindFailureAction::SkipRegeneration => {
            println!("cargo:warning={reason}; skipping static/css/autumn.css regeneration");
        }
    }
}

fn find_tailwind_cli() -> Option<std::path::PathBuf> {
    // 1. Check workspace target directory (from `autumn setup`).
    if let Ok(out_dir) = std::env::var("OUT_DIR") {
        let out_path = std::path::PathBuf::from(out_dir);
        if let Some(target_dir) = out_path.ancestors().nth(4) {
            let bin_name = if cfg!(windows) {
                "tailwindcss.exe"
            } else {
                "tailwindcss"
            };
            let local = target_dir.join("autumn").join(bin_name);
            if local.exists() {
                return Some(local);
            }
        }
    }

    // 2. Check PATH
    if let Some(path) = which("tailwindcss") {
        return Some(path);
    }

    None
}

fn which(binary: &str) -> Option<std::path::PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(binary);
        if candidate.exists() {
            return Some(candidate);
        }
        #[cfg(target_os = "windows")]
        {
            let candidate_exe = dir.join(format!("{binary}.exe"));
            if candidate_exe.exists() {
                return Some(candidate_exe);
            }
        }
    }
    None
}
