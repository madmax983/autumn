use std::process::Command;

/// Options controlling `autumn openapi` behaviour.
pub struct OpenApiOptions<'a> {
    pub package: Option<&'a str>,
    pub bin: Option<&'a str>,
    pub format: OutputFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputFormat {
    Json,
    Yaml,
}

impl std::str::FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "json" => Ok(Self::Json),
            "yaml" => Ok(Self::Yaml),
            other => Err(format!(
                "unknown format '{other}'; expected 'json' or 'yaml'"
            )),
        }
    }
}

/// Run `autumn openapi`.
pub fn run(opts: &OpenApiOptions<'_>) {
    eprintln!("\u{1F342} autumn openapi\n");
    crate::routes::compile_binary(opts.package, opts.bin);
    let binary = crate::routes::find_binary(opts.package, opts.bin);

    let format_env = match opts.format {
        OutputFormat::Json => "json",
        OutputFormat::Yaml => "yaml",
    };

    let output = Command::new(&binary)
        .env("AUTUMN_DUMP_OPENAPI", "1")
        .env("AUTUMN_OPENAPI_FORMAT", format_env)
        .output()
        .unwrap_or_else(|e| {
            eprintln!("\u{2717} Failed to run {}: {e}", binary.display());
            std::process::exit(1);
        });

    if !output.status.success() {
        eprintln!("\u{2717} Extraction failed");
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.trim().is_empty() {
            eprintln!("{stderr}");
        }
        std::process::exit(1);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    print!("{stdout}");
}
