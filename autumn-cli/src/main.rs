use clap::{Parser, Subcommand};

mod build;
mod check;
mod dev;
mod doctor;
mod export;
mod generate;
mod migrate;
mod monitor;
mod new;
mod release;
mod routes;
mod seed;
mod setup;
mod task;
mod token;
/// The Autumn web framework CLI.
#[derive(Parser)]
#[command(name = "autumn", version, about = "The Autumn web framework CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// Available subcommands.
#[derive(Subcommand)]
enum Commands {
    /// Create a new Autumn project
    New {
        /// Project name (must be a valid Rust package name)
        name: String,
        /// Scaffold the optional i18n module (Project Fluent translations
        /// at `i18n/en.ftl`, the `[i18n]` block in `autumn.toml`, and the
        /// `i18n` feature flag on `autumn-web`).
        #[arg(long)]
        with_i18n: bool,
        /// Scaffold a stub `src/bin/seed.rs` for database seeding (default off)
        #[arg(long)]
        with_seed: bool,
    },
    /// Pre-render static routes to dist/
    Build {
        /// Build in debug mode instead of release
        #[arg(long)]
        debug: bool,
        /// Package to build (for workspaces)
        #[arg(short, long)]
        package: Option<String>,
    },
    /// Start the dev server with hot reload (watch mode)
    Dev {
        /// Package to run (for workspaces)
        #[arg(short, long)]
        package: Option<String>,
        /// Log all registered routes, tasks, middleware, and config at startup
        #[arg(long)]
        show_config: bool,
    },
    /// Download and configure external tools (Tailwind CSS)
    Setup {
        /// Re-download even if the binary already exists
        #[arg(long)]
        force: bool,
    },
    /// Run or inspect database migrations
    Migrate {
        #[command(subcommand)]
        action: Option<MigrateCommands>,
    },
    /// Live monitoring dashboard for a running Autumn application
    Monitor {
        /// URL of the running Autumn application
        #[arg(short, long, default_value = "http://localhost:3000")]
        url: String,
        /// Polling interval in seconds
        #[arg(short, long, default_value = "1")]
        interval: u64,
    },
    /// Export an offline diagnostic snapshot of the application
    Export {
        /// URL of the running Autumn application
        #[arg(short, long, default_value = "http://localhost:3000")]
        url: String,
        /// Output file for diagnostics
        #[arg(short, long, default_value = "autumn-diag.json")]
        output: String,
    },
    /// Run the project's seed binary to populate the database with representative data.
    ///
    /// Requires `src/bin/seed.rs` (a Cargo binary named `seed`) to exist.
    /// If it is missing, `autumn seed` prints an actionable error and exits 1.
    ///
    /// `autumn seed` checks for pending migrations before running and exits 1
    /// if any are found — run `autumn migrate` first.
    Seed {
        /// Profile forwarded to the seed binary via `AUTUMN_ENV`
        /// (default: `dev`).
        #[arg(long, default_value = "dev")]
        profile: String,
        /// Package to run (for workspaces)
        #[arg(short, long)]
        package: Option<String>,
    },
    /// Run or list one-off operational tasks registered by the application.
    Task {
        /// Package to run (for workspaces).
        #[arg(short, long)]
        package: Option<String>,
        /// Binary target to run (for packages with multiple bin targets).
        #[arg(long, value_name = "BIN")]
        bin: Option<String>,
        /// Profile forwarded to the app binary via `AUTUMN_ENV`.
        #[arg(long, default_value = "dev")]
        profile: String,
        /// List registered tasks instead of running one.
        #[arg(long)]
        list: bool,
        /// Task name to run.
        name: Option<String>,
        /// Arguments forwarded to the task, e.g. `--user-id 42`.
        #[arg(
            value_name = "ARGS",
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        args: Vec<String>,
    },
    /// Scaffold models, migrations, and CRUD code for a new resource.
    ///
    /// Four subcommands collapse the repetitive five-file dance of adding
    /// a resource — `#[model]` struct, Diesel migration, schema entry,
    /// `#[repository]`, route handlers, Maud templates, `routes![]`
    /// registration, smoke test — into a single command.
    ///
    /// # Field-type DSL
    ///
    /// Fields are passed as `name:Type` tokens. Supported types:
    ///
    ///   String, Text                 (TEXT)
    ///   i32, i64                     (INTEGER, BIGINT)
    ///   bool                         (BOOLEAN)
    ///   f32, f64                     (REAL, DOUBLE PRECISION)
    ///   Uuid                         (UUID)
    ///   `NaiveDateTime`, `DateTime`      (TIMESTAMP, TIMESTAMPTZ)
    ///   `Vec<u8>`, Bytea               (BYTEA)
    ///   Option<...>                  (any of the above, nullable)
    ///
    /// # Example
    ///
    ///   autumn generate scaffold Post title:String body:Text published:bool
    #[command(subcommand, verbatim_doc_comment)]
    Generate(GenerateCommands),

    /// Scaffold production deployment artifacts (Dockerfile, .dockerignore,
    /// runtime config template, and optional target-specific files).
    ///
    /// Run from the project root directory. Does not overwrite existing files
    /// unless `--force` is given.
    ///
    /// # Examples
    ///
    ///   autumn release init
    ///   autumn release init --force
    ///   autumn release init --target fly
    ///   autumn release init --target docker-compose
    #[command(subcommand, verbatim_doc_comment)]
    Release(ReleaseCommands),

    /// Issue and revoke API bearer tokens backed by the `api_tokens` table.
    ///
    /// Requires the `api_tokens` table to exist (run `autumn migrate` first,
    /// with `API_TOKEN_MIGRATIONS` included in your app's migration set).
    /// The database URL is read from `autumn.toml` or the `DATABASE_URL` /
    /// `AUTUMN_DATABASE__URL` environment variables.
    ///
    /// # Examples
    ///
    ///   autumn token issue user:42
    ///   autumn token revoke `<RAW_TOKEN>`
    #[command(subcommand, verbatim_doc_comment)]
    Token(TokenCommands),

    /// Run accessibility (WCAG 2.1 AA) checks against rendered HTML.
    ///
    /// `autumn check --a11y` runs a pure-Rust static HTML analysis pass and
    /// reports Critical and Serious violations that would block a11y compliance.
    /// Point it at a running Autumn app with `--url`, or supply raw HTML via
    /// `--html` for CI pre-render workflows.
    ///
    /// # Examples
    ///
    ///   autumn check --a11y --url http://localhost:3000
    ///   autumn check --a11y --html "$(cat dist/index.html)"
    #[command(verbatim_doc_comment)]
    Check {
        /// Run the WCAG 2.1 AA accessibility audit.
        #[arg(long)]
        a11y: bool,
        /// URL of a running Autumn app to audit (fetches the root page).
        #[arg(long, value_name = "URL")]
        url: Option<String>,
        /// Inline HTML string to audit instead of fetching from a URL.
        #[arg(long, value_name = "HTML")]
        html: Option<String>,
        /// Fail only on Critical violations; treat Serious as warnings.
        #[arg(long)]
        critical_only: bool,
    },

    /// Check the local environment and project configuration for common
    /// first-run problems (Rust MSRV, autumn.toml validity, database
    /// connectivity, port availability, Tailwind binary, and more).
    Doctor {
        /// Emit machine-readable JSON instead of human-readable text.
        #[arg(long)]
        json: bool,
        /// Treat warnings as failures (exit 1 on any ⚠️).
        #[arg(long)]
        strict: bool,
    },

    /// Print every mounted route — method, path, handler, source, middleware.
    ///
    /// Compiles the application (debug profile) and introspects its route
    /// table without starting the HTTP server or connecting to a database.
    ///
    /// Rows are stable-sorted by path, then method, so the output is
    /// diff-friendly. Redirect to a file and `git diff` two snapshots to
    /// audit route changes between commits.
    Routes {
        /// Package to inspect (for workspaces).
        #[arg(short, long)]
        package: Option<String>,
        /// Binary target to inspect (for packages with multiple bin targets).
        #[arg(long, value_name = "BIN")]
        bin: Option<String>,
        /// Output format.
        #[arg(long, default_value = "table", value_name = "FORMAT")]
        format: String,
        /// Show only routes whose path starts with PREFIX (positional shorthand for --filter).
        #[arg(value_name = "PREFIX")]
        prefix: Option<String>,
        /// Show only routes whose path starts with FILTER.
        #[arg(long, value_name = "FILTER")]
        filter: Option<String>,
        /// Restrict to one or more HTTP methods (comma-separated, e.g. `GET,POST`).
        #[arg(long, value_delimiter = ',', value_name = "METHOD")]
        method: Vec<String>,
        /// Hide framework-internal routes (`/actuator/*`, probes, htmx assets).
        #[arg(long)]
        user_only: bool,
    },
}

/// Subcommands for `autumn migrate`.
#[derive(Subcommand)]
enum MigrateCommands {
    /// Show migration status (applied and pending)
    Status,
}

/// Subcommands for `autumn token`.
#[derive(Subcommand)]
enum TokenCommands {
    /// Issue a new API bearer token for a principal and print it to stdout.
    ///
    /// The token is generated with 256 bits of OS-backed randomness and stored
    /// as a SHA-256 hash. It is printed **once** — there is no way to recover
    /// it later. Store it securely (e.g. in a secrets manager).
    ///
    /// # Example
    ///
    ///   TOKEN=$(autumn token issue user:42)
    ///   curl -H "Authorization: Bearer $TOKEN" <http://localhost:3000/api/data>
    #[command(verbatim_doc_comment)]
    Issue {
        /// Principal identifier to associate with the token (e.g. `user:42`).
        principal_id: String,
    },
    /// Revoke an existing API bearer token.
    ///
    /// Hashes the provided raw token and sets `revoked_at` in the database.
    /// Subsequent requests presenting the token will receive `401 Unauthorized`.
    ///
    /// # Example
    ///
    ///   autumn token revoke `<RAW_TOKEN>`
    #[command(verbatim_doc_comment)]
    Revoke {
        /// The raw bearer token string to revoke.
        raw_token: String,
    },
}

/// Subcommands for `autumn release`.
#[derive(Subcommand)]
enum ReleaseCommands {
    /// Emit production-ready deployment files at the project root.
    ///
    /// Default (no --target): Dockerfile + .dockerignore + autumn.production.toml.example.
    /// --target fly        : also emits fly.toml.
    /// --target docker-compose : also emits docker-compose.yml with app + Postgres.
    Init {
        /// Overwrite existing files instead of erroring on collision.
        #[arg(long)]
        force: bool,
        /// Deployment target: fly | docker-compose (omit for bare Dockerfile).
        #[arg(long, value_name = "TARGET")]
        target: Option<String>,
    },
}

/// Subcommands for `autumn generate`.
#[derive(Subcommand)]
enum GenerateCommands {
    /// Generate a `#[model]` struct, Diesel migration, and schema entry.
    ///
    /// Example:
    ///
    ///   autumn generate model Post title:String body:Text published:bool
    #[command(verbatim_doc_comment)]
    Model {
        /// Resource name (`PascalCase` or `snake_case`, e.g. `Post`).
        name: String,
        /// Field DSL tokens, each `name:Type`.
        fields: Vec<String>,
        /// Print the file plan and exit without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Overwrite existing files instead of erroring on collision.
        #[arg(long)]
        force: bool,
    },
    /// Generate an empty Diesel migration directory.
    ///
    /// When the migration name follows the `Add<Field>To<Table>` or
    /// `Remove<Field>From<Table>` convention, the generator emits the
    /// matching `ALTER TABLE` statements automatically.
    Migration {
        /// Migration name (`PascalCase` or `snake_case`).
        name: String,
        /// Field DSL tokens — only used for `Add…To…` / `Remove…From…` names.
        fields: Vec<String>,
        /// Print the file plan and exit without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Overwrite existing files instead of erroring on collision.
        #[arg(long)]
        force: bool,
    },
    /// Generate a one-off operational `#[task]` skeleton.
    Task {
        /// Task function name (`snake_case`, e.g. `cleanup_users`).
        name: String,
        /// Print the file plan and exit without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Overwrite existing files instead of erroring on collision.
        #[arg(long)]
        force: bool,
    },
    /// Generate model, migration, repository, HTML routes, smoke test, and
    /// register the new routes in `src/main.rs`.
    Scaffold {
        /// Resource name (`PascalCase` or `snake_case`, e.g. `Post`).
        name: String,
        /// Field DSL tokens, each `name:Type`.
        fields: Vec<String>,
        /// Add `#[indexed]` and a SQL index for this field. Repeatable.
        #[arg(long, value_name = "FIELD")]
        index: Vec<String>,
        /// Add a validator rule, e.g. `url=url` or `title=length:min=1,max=200`.
        #[arg(long, value_name = "FIELD=RULE")]
        validate: Vec<String>,
        /// Add `#[default]` and a SQL default, e.g. `alive=true`.
        #[arg(long, value_name = "FIELD=VALUE")]
        default: Vec<String>,
        /// Add a derived repository query, e.g. `find_by_tag:tag`.
        #[arg(long, value_name = "METHOD:FIELD")]
        query: Vec<String>,
        /// Print the file plan and exit without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Overwrite existing files instead of erroring on collision.
        #[arg(long)]
        force: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    run_command(cli.command);
}

fn run_command(command: Commands) {
    match command {
        Commands::Build { debug, package } => build::run(debug, package.as_deref()),
        Commands::Dev {
            package,
            show_config,
        } => dev::run(package.as_deref(), show_config),
        Commands::Migrate { action } => {
            let action = match action {
                Some(MigrateCommands::Status) => migrate::MigrateAction::Status,
                None => migrate::MigrateAction::Run,
            };
            migrate::run(action);
        }
        Commands::Monitor { url, interval } => monitor::run(&url, interval),
        Commands::Export { url, output } => export::run(&url, &output),
        Commands::New {
            name,
            with_i18n,
            with_seed,
        } => new::run(
            &name,
            new::GenerateOptions {
                with_i18n,
                with_seed,
            },
        ),
        Commands::Seed { profile, package } => seed::run(&profile, package.as_deref()),
        Commands::Task {
            package,
            bin,
            profile,
            list,
            name,
            args,
        } => run_task_command(
            package.as_deref(),
            bin.as_deref(),
            &profile,
            list,
            name.as_deref(),
            &args,
        ),
        Commands::Setup { force } => setup::run(force),
        Commands::Routes {
            package,
            bin,
            format,
            prefix,
            filter,
            method,
            user_only,
        } => run_routes_command(
            package.as_deref(),
            bin.as_deref(),
            &format,
            prefix.as_deref(),
            filter.as_deref(),
            &method,
            user_only,
        ),
        Commands::Release(cmd) => run_release_command(cmd),
        Commands::Token(cmd) => match cmd {
            TokenCommands::Issue { principal_id } => token::run_issue(&principal_id),
            TokenCommands::Revoke { raw_token } => token::run_revoke(&raw_token),
        },
        Commands::Check {
            a11y,
            url,
            html,
            critical_only,
        } => {
            if a11y {
                let opts = check::A11yCheckOptions {
                    url: url.clone(),
                    html,
                    critical_only,
                };
                let label = url.as_deref().unwrap_or("<inline>");
                match check::run_a11y_check(&opts) {
                    Ok(violations) => {
                        if check::print_report(&violations, label) {
                            std::process::exit(1);
                        }
                    }
                    Err(e) => {
                        eprintln!("autumn check --a11y: {e}");
                        std::process::exit(1);
                    }
                }
            } else {
                eprintln!("autumn check: specify at least one check flag (e.g. --a11y)");
                std::process::exit(1);
            }
        }
        Commands::Doctor { json, strict } => {
            doctor::run(doctor::DoctorOptions { json, strict });
        }
        Commands::Generate(cmd) => run_generate_command(cmd),
    }
}

fn run_task_command(
    package: Option<&str>,
    bin: Option<&str>,
    profile: &str,
    list: bool,
    name: Option<&str>,
    args: &[String],
) {
    task::run(&task::TaskOptions {
        package,
        bin,
        profile,
        list,
        name,
        args,
    });
}

fn run_routes_command(
    package: Option<&str>,
    bin: Option<&str>,
    format: &str,
    prefix: Option<&str>,
    filter: Option<&str>,
    method: &[String],
    user_only: bool,
) {
    let fmt = format.parse().unwrap_or_else(|e| {
        eprintln!("autumn routes: {e}");
        std::process::exit(1);
    });
    // Positional prefix takes precedence over --filter when both are given.
    let effective_filter = prefix.or(filter);
    routes::run(&routes::RoutesOptions {
        package,
        bin,
        format: fmt,
        filter: effective_filter,
        methods: method,
        user_only,
    });
}

fn run_release_command(cmd: ReleaseCommands) {
    match cmd {
        ReleaseCommands::Init { force, target } => {
            let t = target.as_deref().map_or(release::Target::Default, |s| {
                s.parse().unwrap_or_else(|e| {
                    eprintln!("autumn release init: {e}");
                    std::process::exit(1);
                })
            });
            release::run(release::ReleaseAction::Init { force, target: t });
        }
    }
}

fn run_generate_command(cmd: GenerateCommands) {
    match cmd {
        GenerateCommands::Model {
            name,
            fields,
            dry_run,
            force,
        } => generate::model::run(&name, &fields, generate::Flags { dry_run, force }),
        GenerateCommands::Migration {
            name,
            fields,
            dry_run,
            force,
        } => generate::migration::run(&name, &fields, generate::Flags { dry_run, force }),
        GenerateCommands::Task {
            name,
            dry_run,
            force,
        } => generate::task::run(&name, generate::Flags { dry_run, force }),
        GenerateCommands::Scaffold {
            name,
            fields,
            index,
            validate,
            default,
            query,
            dry_run,
            force,
        } => {
            let options = generate::scaffold::ScaffoldOptions {
                model: generate::model::ModelOptions {
                    indexes: index,
                    validations: validate,
                    defaults: default,
                },
                queries: query,
            };
            generate::scaffold::run(&name, &fields, generate::Flags { dry_run, force }, &options);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_new_subcommand() {
        let cli = Cli::try_parse_from(["autumn", "new", "my-app"]).unwrap();
        match cli.command {
            Commands::New { ref name, .. } => {
                assert_eq!(name, "my-app");
            }
            _ => panic!("expected New command"),
        }
    }

    #[test]
    fn parse_new_with_underscores() {
        let cli = Cli::try_parse_from(["autumn", "new", "my_app"]).unwrap();
        match cli.command {
            Commands::New { ref name, .. } => {
                assert_eq!(name, "my_app");
            }
            _ => panic!("expected New command"),
        }
    }

    #[test]
    fn parse_new_with_i18n_flag() {
        let cli = Cli::try_parse_from(["autumn", "new", "my-app", "--with-i18n"]).unwrap();
        match cli.command {
            Commands::New {
                ref name,
                with_i18n,
                ..
            } => {
                assert_eq!(name, "my-app");
                assert!(with_i18n);
            }
            _ => panic!("expected New command"),
        }
    }

    #[test]
    fn parse_new_without_i18n_flag_defaults_off() {
        let cli = Cli::try_parse_from(["autumn", "new", "my-app"]).unwrap();
        match cli.command {
            Commands::New { with_i18n, .. } => assert!(!with_i18n),
            _ => panic!("expected New command"),
        }
    }

    #[test]
    fn parse_setup_subcommand() {
        let cli = Cli::try_parse_from(["autumn", "setup"]).unwrap();
        assert!(matches!(cli.command, Commands::Setup { force: false }));
    }

    #[test]
    fn parse_setup_with_force() {
        let cli = Cli::try_parse_from(["autumn", "setup", "--force"]).unwrap();
        assert!(matches!(cli.command, Commands::Setup { force: true }));
    }

    #[test]
    fn new_rejects_removed_wasm_flag() {
        assert!(Cli::try_parse_from(["autumn", "new", "my-app", "--wasm"]).is_err());
    }

    #[test]
    fn setup_rejects_removed_wasm_flag() {
        assert!(Cli::try_parse_from(["autumn", "setup", "--wasm"]).is_err());
    }

    #[test]
    fn no_args_is_error() {
        assert!(Cli::try_parse_from(["autumn"]).is_err());
    }

    #[test]
    fn new_missing_name_is_error() {
        assert!(Cli::try_parse_from(["autumn", "new"]).is_err());
    }

    #[test]
    fn parse_build_subcommand() {
        let cli = Cli::try_parse_from(["autumn", "build"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Build {
                debug: false,
                package: None
            }
        ));
    }

    #[test]
    fn parse_build_debug() {
        let cli = Cli::try_parse_from(["autumn", "build", "--debug"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Build {
                debug: true,
                package: None
            }
        ));
    }

    #[test]
    fn parse_build_with_package() {
        let cli = Cli::try_parse_from(["autumn", "build", "-p", "blog"]).unwrap();
        match cli.command {
            Commands::Build { debug, package } => {
                assert!(!debug);
                assert_eq!(package.as_deref(), Some("blog"));
            }
            _ => panic!("expected Build command"),
        }
    }

    #[test]
    fn parse_build_with_long_package() {
        let cli = Cli::try_parse_from(["autumn", "build", "--package", "blog", "--debug"]).unwrap();
        match cli.command {
            Commands::Build { debug, package } => {
                assert!(debug);
                assert_eq!(package.as_deref(), Some("blog"));
            }
            _ => panic!("expected Build command"),
        }
    }

    #[test]
    fn parse_dev_subcommand() {
        let cli = Cli::try_parse_from(["autumn", "dev"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Dev {
                package: None,
                show_config: false
            }
        ));
    }

    #[test]
    fn parse_dev_with_package() {
        let cli = Cli::try_parse_from(["autumn", "dev", "-p", "hello"]).unwrap();
        match cli.command {
            Commands::Dev {
                package,
                show_config,
            } => {
                assert_eq!(package.as_deref(), Some("hello"));
                assert!(!show_config);
            }
            _ => panic!("expected Dev command"),
        }
    }

    #[test]
    fn parse_dev_with_show_config() {
        let cli = Cli::try_parse_from(["autumn", "dev", "--show-config"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Dev {
                package: None,
                show_config: true
            }
        ));
    }

    #[test]
    fn parse_migrate_subcommand() {
        let cli = Cli::try_parse_from(["autumn", "migrate"]).unwrap();
        assert!(matches!(cli.command, Commands::Migrate { action: None }));
    }

    #[test]
    fn parse_migrate_status() {
        let cli = Cli::try_parse_from(["autumn", "migrate", "status"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Migrate {
                action: Some(MigrateCommands::Status)
            }
        ));
    }

    #[test]
    fn parse_monitor_defaults() {
        let cli = Cli::try_parse_from(["autumn", "monitor"]).unwrap();
        match cli.command {
            Commands::Monitor { url, interval } => {
                assert_eq!(url, "http://localhost:3000");
                assert_eq!(interval, 1);
            }
            _ => panic!("expected Monitor command"),
        }
    }

    #[test]
    fn parse_monitor_custom_url() {
        let cli = Cli::try_parse_from(["autumn", "monitor", "-u", "http://prod:8080", "-i", "5"])
            .unwrap();
        match cli.command {
            Commands::Monitor { url, interval } => {
                assert_eq!(url, "http://prod:8080");
                assert_eq!(interval, 5);
            }
            _ => panic!("expected Monitor command"),
        }
    }

    #[test]
    fn parse_export_defaults() {
        let cli = Cli::try_parse_from(["autumn", "export"]).unwrap();
        match cli.command {
            Commands::Export { url, output } => {
                assert_eq!(url, "http://localhost:3000");
                assert_eq!(output, "autumn-diag.json");
            }
            _ => panic!("expected Export command"),
        }
    }

    #[test]
    fn parse_export_custom() {
        let cli = Cli::try_parse_from([
            "autumn",
            "export",
            "-u",
            "http://prod:8080",
            "-o",
            "snapshot.json",
        ])
        .unwrap();
        match cli.command {
            Commands::Export { url, output } => {
                assert_eq!(url, "http://prod:8080");
                assert_eq!(output, "snapshot.json");
            }
            _ => panic!("expected Export command"),
        }
    }

    #[test]
    fn unknown_subcommand_is_error() {
        assert!(Cli::try_parse_from(["autumn", "bogus"]).is_err());
    }

    #[test]
    fn parse_generate_model() {
        let cli = Cli::try_parse_from([
            "autumn",
            "generate",
            "model",
            "Post",
            "title:String",
            "body:Text",
        ])
        .unwrap();
        let Commands::Generate(GenerateCommands::Model {
            name,
            fields,
            dry_run,
            force,
        }) = cli.command
        else {
            panic!("expected generate model");
        };
        assert_eq!(name, "Post");
        assert_eq!(fields, vec!["title:String", "body:Text"]);
        assert!(!dry_run);
        assert!(!force);
    }

    #[test]
    fn parse_generate_model_with_flags() {
        let cli = Cli::try_parse_from([
            "autumn",
            "generate",
            "model",
            "Post",
            "--dry-run",
            "--force",
        ])
        .unwrap();
        let Commands::Generate(GenerateCommands::Model { dry_run, force, .. }) = cli.command else {
            panic!("expected generate model");
        };
        assert!(dry_run);
        assert!(force);
    }

    #[test]
    fn parse_generate_migration() {
        let cli = Cli::try_parse_from([
            "autumn",
            "generate",
            "migration",
            "AddTitleToPosts",
            "title:String",
        ])
        .unwrap();
        let Commands::Generate(GenerateCommands::Migration { name, fields, .. }) = cli.command
        else {
            panic!("expected generate migration");
        };
        assert_eq!(name, "AddTitleToPosts");
        assert_eq!(fields, vec!["title:String"]);
    }

    #[test]
    fn parse_generate_task() {
        let cli = Cli::try_parse_from(["autumn", "generate", "task", "cleanup_users", "--dry-run"])
            .unwrap();
        let Commands::Generate(GenerateCommands::Task {
            name,
            dry_run,
            force,
        }) = cli.command
        else {
            panic!("expected generate task");
        };
        assert_eq!(name, "cleanup_users");
        assert!(dry_run);
        assert!(!force);
    }

    #[test]
    fn parse_generate_scaffold() {
        let cli = Cli::try_parse_from([
            "autumn",
            "generate",
            "scaffold",
            "Post",
            "title:String",
            "body:Text",
            "published:bool",
        ])
        .unwrap();
        let Commands::Generate(GenerateCommands::Scaffold { name, fields, .. }) = cli.command
        else {
            panic!("expected generate scaffold");
        };
        assert_eq!(name, "Post");
        assert_eq!(fields.len(), 3);
    }

    #[test]
    fn parse_generate_scaffold_metadata_flags() {
        let cli = Cli::try_parse_from([
            "autumn",
            "generate",
            "scaffold",
            "Bookmark",
            "url:String",
            "alive:bool",
            "--index",
            "url",
            "--validate",
            "url=url",
            "--default",
            "alive=true",
            "--query",
            "find_by_alive:alive",
        ])
        .unwrap();
        let Commands::Generate(GenerateCommands::Scaffold {
            index,
            validate,
            default,
            query,
            ..
        }) = cli.command
        else {
            panic!("expected generate scaffold");
        };
        assert_eq!(index, vec!["url"]);
        assert_eq!(validate, vec!["url=url"]);
        assert_eq!(default, vec!["alive=true"]);
        assert_eq!(query, vec!["find_by_alive:alive"]);
    }

    #[test]
    fn parse_generate_without_subcommand_is_error() {
        assert!(Cli::try_parse_from(["autumn", "generate"]).is_err());
    }

    #[test]
    fn parse_generate_model_without_name_is_error() {
        assert!(Cli::try_parse_from(["autumn", "generate", "model"]).is_err());
    }

    // ── autumn seed tests ──────────────────────────────────────────────────

    #[test]
    fn parse_seed_defaults() {
        let cli = Cli::try_parse_from(["autumn", "seed"]).unwrap();
        match cli.command {
            Commands::Seed { profile, package } => {
                assert_eq!(profile, "dev");
                assert!(package.is_none());
            }
            _ => panic!("expected Seed command"),
        }
    }

    #[test]
    fn parse_seed_with_profile() {
        let cli = Cli::try_parse_from(["autumn", "seed", "--profile", "demo"]).unwrap();
        match cli.command {
            Commands::Seed { profile, .. } => {
                assert_eq!(profile, "demo");
            }
            _ => panic!("expected Seed command"),
        }
    }

    #[test]
    fn parse_seed_with_package() {
        let cli = Cli::try_parse_from(["autumn", "seed", "-p", "my-app"]).unwrap();
        match cli.command {
            Commands::Seed { package, .. } => {
                assert_eq!(package.as_deref(), Some("my-app"));
            }
            _ => panic!("expected Seed command"),
        }
    }

    #[test]
    fn parse_seed_test_profile() {
        let cli = Cli::try_parse_from(["autumn", "seed", "--profile", "test"]).unwrap();
        match cli.command {
            Commands::Seed { profile, .. } => assert_eq!(profile, "test"),
            _ => panic!("expected Seed command"),
        }
    }

    #[test]
    fn parse_seed_prod_profile() {
        let cli = Cli::try_parse_from(["autumn", "seed", "--profile", "prod"]).unwrap();
        match cli.command {
            Commands::Seed { profile, .. } => assert_eq!(profile, "prod"),
            _ => panic!("expected Seed command"),
        }
    }

    // ── autumn routes tests ────────────────────────────────────────────────

    #[test]
    fn parse_task_run_with_cli_args() {
        let cli =
            Cli::try_parse_from(["autumn", "task", "cleanup-user", "--user-id", "42"]).unwrap();
        match cli.command {
            Commands::Task {
                name,
                args,
                list,
                profile,
                package,
                bin,
            } => {
                assert_eq!(name.as_deref(), Some("cleanup-user"));
                assert_eq!(args, vec!["--user-id", "42"]);
                assert!(!list);
                assert_eq!(profile, "dev");
                assert!(package.is_none());
                assert!(bin.is_none());
            }
            _ => panic!("expected Task command"),
        }
    }

    #[test]
    fn parse_task_list_with_package_and_bin() {
        let cli = Cli::try_parse_from([
            "autumn",
            "task",
            "--list",
            "--package",
            "blog",
            "--bin",
            "blog",
        ])
        .unwrap();
        match cli.command {
            Commands::Task {
                name,
                list,
                package,
                bin,
                ..
            } => {
                assert!(name.is_none());
                assert!(list);
                assert_eq!(package.as_deref(), Some("blog"));
                assert_eq!(bin.as_deref(), Some("blog"));
            }
            _ => panic!("expected Task command"),
        }
    }

    #[test]
    fn parse_task_with_profile() {
        let cli =
            Cli::try_parse_from(["autumn", "task", "--profile", "prod", "cleanup-user"]).unwrap();
        match cli.command {
            Commands::Task { profile, name, .. } => {
                assert_eq!(profile, "prod");
                assert_eq!(name.as_deref(), Some("cleanup-user"));
            }
            _ => panic!("expected Task command"),
        }
    }

    #[test]
    fn parse_routes_defaults() {
        let cli = Cli::try_parse_from(["autumn", "routes"]).unwrap();
        match cli.command {
            Commands::Routes {
                package,
                bin,
                format,
                prefix,
                filter,
                method,
                user_only,
            } => {
                assert!(package.is_none());
                assert!(bin.is_none());
                assert_eq!(format, "table");
                assert!(prefix.is_none());
                assert!(filter.is_none());
                assert!(method.is_empty());
                assert!(!user_only);
            }
            _ => panic!("expected Routes command"),
        }
    }

    #[test]
    fn parse_routes_with_package() {
        let cli = Cli::try_parse_from(["autumn", "routes", "-p", "blog"]).unwrap();
        match cli.command {
            Commands::Routes { package, .. } => {
                assert_eq!(package.as_deref(), Some("blog"));
            }
            _ => panic!("expected Routes command"),
        }
    }

    #[test]
    fn parse_routes_with_long_package() {
        let cli = Cli::try_parse_from(["autumn", "routes", "--package", "my-app"]).unwrap();
        match cli.command {
            Commands::Routes { package, .. } => {
                assert_eq!(package.as_deref(), Some("my-app"));
            }
            _ => panic!("expected Routes command"),
        }
    }

    #[test]
    fn parse_routes_format_json() {
        let cli = Cli::try_parse_from(["autumn", "routes", "--format", "json"]).unwrap();
        match cli.command {
            Commands::Routes { format, .. } => {
                assert_eq!(format, "json");
            }
            _ => panic!("expected Routes command"),
        }
    }

    #[test]
    fn parse_routes_with_filter() {
        let cli = Cli::try_parse_from(["autumn", "routes", "--filter", "/api"]).unwrap();
        match cli.command {
            Commands::Routes { filter, .. } => {
                assert_eq!(filter.as_deref(), Some("/api"));
            }
            _ => panic!("expected Routes command"),
        }
    }

    #[test]
    fn parse_routes_with_method() {
        let cli = Cli::try_parse_from(["autumn", "routes", "--method", "GET"]).unwrap();
        match cli.command {
            Commands::Routes { method, .. } => {
                assert_eq!(method, vec!["GET"]);
            }
            _ => panic!("expected Routes command"),
        }
    }

    #[test]
    fn parse_routes_with_multiple_methods() {
        let cli = Cli::try_parse_from(["autumn", "routes", "--method", "GET,POST"]).unwrap();
        match cli.command {
            Commands::Routes { method, .. } => {
                assert_eq!(method, vec!["GET", "POST"]);
            }
            _ => panic!("expected Routes command"),
        }
    }

    #[test]
    fn parse_routes_with_user_only() {
        let cli = Cli::try_parse_from(["autumn", "routes", "--user-only"]).unwrap();
        match cli.command {
            Commands::Routes { user_only, .. } => {
                assert!(user_only);
            }
            _ => panic!("expected Routes command"),
        }
    }

    #[test]
    fn parse_routes_with_bin() {
        let cli = Cli::try_parse_from(["autumn", "routes", "--bin", "server"]).unwrap();
        match cli.command {
            Commands::Routes { bin, .. } => {
                assert_eq!(bin.as_deref(), Some("server"));
            }
            _ => panic!("expected Routes command"),
        }
    }

    #[test]
    fn parse_routes_all_options() {
        let cli = Cli::try_parse_from([
            "autumn",
            "routes",
            "-p",
            "blog",
            "--format",
            "json",
            "--filter",
            "/api",
            "--method",
            "GET,POST",
            "--user-only",
        ])
        .unwrap();
        match cli.command {
            Commands::Routes {
                package,
                bin,
                format,
                prefix,
                filter,
                method,
                user_only,
            } => {
                assert_eq!(package.as_deref(), Some("blog"));
                assert!(bin.is_none());
                assert_eq!(format, "json");
                assert!(prefix.is_none());
                assert_eq!(filter.as_deref(), Some("/api"));
                assert_eq!(method, vec!["GET", "POST"]);
                assert!(user_only);
            }
            _ => panic!("expected Routes command"),
        }
    }

    #[test]
    fn parse_routes_positional_prefix() {
        let cli = Cli::try_parse_from(["autumn", "routes", "/api"]).unwrap();
        match cli.command {
            Commands::Routes { prefix, filter, .. } => {
                assert_eq!(prefix.as_deref(), Some("/api"));
                assert!(filter.is_none());
            }
            _ => panic!("expected Routes command"),
        }
    }

    #[test]
    fn parse_routes_positional_prefix_with_package() {
        let cli = Cli::try_parse_from(["autumn", "routes", "-p", "blog", "/api"]).unwrap();
        match cli.command {
            Commands::Routes {
                package, prefix, ..
            } => {
                assert_eq!(package.as_deref(), Some("blog"));
                assert_eq!(prefix.as_deref(), Some("/api"));
            }
            _ => panic!("expected Routes command"),
        }
    }

    // ── autumn doctor tests ────────────────────────────────────────────────

    #[test]
    fn parse_doctor_defaults() {
        let cli = Cli::try_parse_from(["autumn", "doctor"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Doctor {
                json: false,
                strict: false
            }
        ));
    }

    #[test]
    fn parse_doctor_json_flag() {
        let cli = Cli::try_parse_from(["autumn", "doctor", "--json"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Doctor {
                json: true,
                strict: false
            }
        ));
    }

    #[test]
    fn parse_doctor_strict_flag() {
        let cli = Cli::try_parse_from(["autumn", "doctor", "--strict"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Doctor {
                json: false,
                strict: true
            }
        ));
    }

    #[test]
    fn parse_doctor_json_and_strict() {
        let cli = Cli::try_parse_from(["autumn", "doctor", "--json", "--strict"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Doctor {
                json: true,
                strict: true
            }
        ));
    }

    // ── autumn release tests ───────────────────────────────────────────────

    #[test]
    fn parse_release_init_defaults() {
        let cli = Cli::try_parse_from(["autumn", "release", "init"]).unwrap();
        let Commands::Release(ReleaseCommands::Init { force, target }) = cli.command else {
            panic!("expected release init");
        };
        assert!(!force);
        assert!(target.is_none());
    }

    #[test]
    fn parse_release_init_with_force() {
        let cli = Cli::try_parse_from(["autumn", "release", "init", "--force"]).unwrap();
        let Commands::Release(ReleaseCommands::Init { force, target }) = cli.command else {
            panic!("expected release init");
        };
        assert!(force);
        assert!(target.is_none());
    }

    #[test]
    fn parse_release_init_with_fly_target() {
        let cli = Cli::try_parse_from(["autumn", "release", "init", "--target", "fly"]).unwrap();
        let Commands::Release(ReleaseCommands::Init { force, target }) = cli.command else {
            panic!("expected release init");
        };
        assert!(!force);
        assert_eq!(target.as_deref(), Some("fly"));
    }

    #[test]
    fn parse_release_init_with_docker_compose_target() {
        let cli = Cli::try_parse_from(["autumn", "release", "init", "--target", "docker-compose"])
            .unwrap();
        let Commands::Release(ReleaseCommands::Init { target, .. }) = cli.command else {
            panic!("expected release init");
        };
        assert_eq!(target.as_deref(), Some("docker-compose"));
    }

    #[test]
    fn parse_release_init_force_and_target() {
        let cli = Cli::try_parse_from(["autumn", "release", "init", "--force", "--target", "fly"])
            .unwrap();
        let Commands::Release(ReleaseCommands::Init { force, target }) = cli.command else {
            panic!("expected release init");
        };
        assert!(force);
        assert_eq!(target.as_deref(), Some("fly"));
    }

    #[test]
    fn parse_release_without_subcommand_is_error() {
        assert!(Cli::try_parse_from(["autumn", "release"]).is_err());
    }

    // ── autumn new --with-seed tests ───────────────────────────────────────

    #[test]
    fn parse_new_without_with_seed_defaults_false() {
        let cli = Cli::try_parse_from(["autumn", "new", "my-app"]).unwrap();
        match cli.command {
            Commands::New {
                name, with_seed, ..
            } => {
                assert_eq!(name, "my-app");
                assert!(!with_seed);
            }
            _ => panic!("expected New command"),
        }
    }

    #[test]
    fn parse_new_with_with_seed_flag() {
        let cli = Cli::try_parse_from(["autumn", "new", "my-app", "--with-seed"]).unwrap();
        match cli.command {
            Commands::New {
                name, with_seed, ..
            } => {
                assert_eq!(name, "my-app");
                assert!(with_seed);
            }
            _ => panic!("expected New command"),
        }
    }

    #[test]
    fn parse_new_with_i18n_and_seed_flags() {
        let cli =
            Cli::try_parse_from(["autumn", "new", "my-app", "--with-i18n", "--with-seed"]).unwrap();
        match cli.command {
            Commands::New {
                name,
                with_i18n,
                with_seed,
            } => {
                assert_eq!(name, "my-app");
                assert!(with_i18n);
                assert!(with_seed);
            }
            _ => panic!("expected New command"),
        }
    }

    // ── autumn token tests ─────────────────────────────────────────────────

    #[test]
    fn parse_token_issue() {
        let cli = Cli::try_parse_from(["autumn", "token", "issue", "user:42"]).unwrap();
        let Commands::Token(TokenCommands::Issue { principal_id }) = cli.command else {
            panic!("expected token issue");
        };
        assert_eq!(principal_id, "user:42");
    }

    #[test]
    fn parse_token_revoke() {
        let cli = Cli::try_parse_from(["autumn", "token", "revoke", "abc123deadbeef"]).unwrap();
        let Commands::Token(TokenCommands::Revoke { raw_token }) = cli.command else {
            panic!("expected token revoke");
        };
        assert_eq!(raw_token, "abc123deadbeef");
    }

    #[test]
    fn parse_token_without_subcommand_is_error() {
        assert!(Cli::try_parse_from(["autumn", "token"]).is_err());
    }

    #[test]
    fn parse_token_issue_without_principal_is_error() {
        assert!(Cli::try_parse_from(["autumn", "token", "issue"]).is_err());
    }

    #[test]
    fn parse_token_revoke_without_token_is_error() {
        assert!(Cli::try_parse_from(["autumn", "token", "revoke"]).is_err());
    }
}
