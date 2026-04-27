use clap::{Parser, Subcommand};

mod build;
mod dev;
mod export;
mod generate;
mod migrate;
mod monitor;
mod new;
mod setup;
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
    /// Scaffold models, migrations, and CRUD code for a new resource.
    ///
    /// Three subcommands collapse the repetitive five-file dance of adding
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
    ///   Vec<u8>, Bytea               (BYTEA)
    ///   Option<...>                  (any of the above, nullable)
    ///
    /// # Example
    ///
    ///   autumn generate scaffold Post title:String body:Text published:bool
    #[command(subcommand, verbatim_doc_comment)]
    Generate(GenerateCommands),
}

/// Subcommands for `autumn migrate`.
#[derive(Subcommand)]
enum MigrateCommands {
    /// Show migration status (applied and pending)
    Status,
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
    /// Generate model, migration, repository, HTML routes, smoke test, and
    /// register the new routes in `src/main.rs`.
    Scaffold {
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
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
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
        Commands::New { name } => new::run(&name),
        Commands::Setup { force } => setup::run(force),
        Commands::Generate(cmd) => match cmd {
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
            GenerateCommands::Scaffold {
                name,
                fields,
                dry_run,
                force,
            } => generate::scaffold::run(&name, &fields, generate::Flags { dry_run, force }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_new_subcommand() {
        let cli = Cli::try_parse_from(["autumn", "new", "my-app"]).unwrap();
        match cli.command {
            Commands::New { ref name } => {
                assert_eq!(name, "my-app");
            }
            _ => panic!("expected New command"),
        }
    }

    #[test]
    fn parse_new_with_underscores() {
        let cli = Cli::try_parse_from(["autumn", "new", "my_app"]).unwrap();
        match cli.command {
            Commands::New { ref name } => {
                assert_eq!(name, "my_app");
            }
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
    fn parse_generate_without_subcommand_is_error() {
        assert!(Cli::try_parse_from(["autumn", "generate"]).is_err());
    }

    #[test]
    fn parse_generate_model_without_name_is_error() {
        assert!(Cli::try_parse_from(["autumn", "generate", "model"]).is_err());
    }
}
