use autumn_web::AppState;
use autumn_web::config::{AutumnConfig, DatabaseConfig};
use autumn_web::db;
use autumn_web_harvest::{
    HarvestMode, HarvestRunner, HarvestRunnerResources, HarvestRuntimeConfig,
};
use reddit_clone::harvest_runtime::harvest_builder;
use reddit_clone::live_events;
use thiserror::Error;

#[derive(Debug, Error)]
enum RunnerConfigError {
    #[error(
        "reddit-clone harvest runner requires database.url because its activities touch app tables"
    )]
    MissingAppDatabaseUrl,
    #[error(
        "reddit-clone harvest runner requires at least one of harvest.worker_enabled or harvest.scheduler_enabled to be true"
    )]
    NoLocalOwnership,
    #[error("reddit-clone harvest runner requires harvest.database.url when harvest.mode is {0:?}")]
    MissingHarvestDatabaseUrl(HarvestMode),
}

#[derive(Debug, Error)]
enum RunnerMainError {
    #[error(transparent)]
    Config(#[from] autumn_web::config::ConfigError),
    #[error(transparent)]
    RunnerConfig(#[from] RunnerConfigError),
    #[error("failed to create database pool: {0}")]
    Pool(String),
    #[error("failed to start harvest runner: {0}")]
    Startup(String),
    #[error("failed waiting for ctrl-c: {0}")]
    Signal(#[from] std::io::Error),
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("reddit-clone harvest runner failed: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), RunnerMainError> {
    let app_config = AutumnConfig::load()?;
    let harvest_config = HarvestRuntimeConfig::load()?;
    validate_runner_ownership(&harvest_config)?;

    let app_database_url = resolve_app_database_url(&app_config)?;
    let harvest_database_url = resolve_harvest_database_url(&app_config, &harvest_config)?;
    let app_pool = build_pool(app_database_url, app_config.database.pool_size)?;
    let harvest_pool = build_pool(harvest_database_url, app_config.database.pool_size)?;
    let profile = app_config.profile.as_deref().unwrap_or("default");
    let app_state = detached_runner_state(profile).with_pool(app_pool.clone());
    live_events::install_live_event_bus(&app_state)
        .await
        .map_err(|error| RunnerMainError::Startup(error.to_string()))?;
    let runner = HarvestRunner::start(
        harvest_builder().build(),
        &harvest_config,
        HarvestRunnerResources::new(harvest_pool)
            .with_app_pool(app_pool)
            .with_app_state(app_state),
    )
    .map_err(|error| RunnerMainError::Startup(error.to_string()))?;

    eprintln!(
        "reddit-clone Harvest runner started (profile={profile}, mode={:?}, worker_enabled={}, scheduler_enabled={})",
        harvest_config.mode, harvest_config.worker_enabled, harvest_config.scheduler_enabled
    );

    tokio::signal::ctrl_c().await?;
    eprintln!("shutting down reddit-clone Harvest runner");
    runner.stop().await;
    Ok(())
}

fn build_pool(
    database_url: &str,
    pool_size: usize,
) -> Result<autumn_harvest::worker::DbPool, RunnerMainError> {
    db::create_pool(&DatabaseConfig {
        url: Some(database_url.to_owned()),
        pool_size,
        ..DatabaseConfig::default()
    })
    .map_err(|error| RunnerMainError::Pool(error.to_string()))?
    .ok_or_else(|| RunnerMainError::Pool("database URL did not produce a pool".to_owned()))
}

fn resolve_app_database_url(config: &AutumnConfig) -> Result<&str, RunnerConfigError> {
    config
        .database
        .url
        .as_deref()
        .ok_or(RunnerConfigError::MissingAppDatabaseUrl)
}

fn resolve_harvest_database_url<'a>(
    app_config: &'a AutumnConfig,
    harvest_config: &'a HarvestRuntimeConfig,
) -> Result<&'a str, RunnerConfigError> {
    match harvest_config.mode {
        HarvestMode::Embedded => resolve_app_database_url(app_config),
        HarvestMode::Split | HarvestMode::External => harvest_config.database.url.as_deref().ok_or(
            RunnerConfigError::MissingHarvestDatabaseUrl(harvest_config.mode),
        ),
    }
}

fn validate_runner_ownership(config: &HarvestRuntimeConfig) -> Result<(), RunnerConfigError> {
    if config.worker_enabled || config.scheduler_enabled {
        Ok(())
    } else {
        Err(RunnerConfigError::NoLocalOwnership)
    }
}

fn detached_runner_state(profile: &str) -> AppState {
    AppState::detached().with_profile(profile)
}

#[cfg(test)]
mod tests {
    use autumn_web::config::AutumnConfig;
    use autumn_web_harvest::{
        HarvestDatabaseConfig, HarvestMode, HarvestOutboxConfig, HarvestRuntimeConfig,
    };

    use super::{
        detached_runner_state, resolve_app_database_url, resolve_harvest_database_url,
        validate_runner_ownership,
    };

    #[test]
    fn runner_requires_local_worker_or_scheduler_ownership() {
        let error = validate_runner_ownership(&HarvestRuntimeConfig {
            mode: HarvestMode::External,
            worker_enabled: false,
            scheduler_enabled: false,
            database: HarvestDatabaseConfig {
                url: Some("postgres://autumn:autumn@localhost:5432/reddit_harvest".to_owned()),
            },
            outbox: HarvestOutboxConfig::default(),
        })
        .expect_err("runner without local ownership should be rejected");

        assert!(error.to_string().contains("worker_enabled"));
    }

    #[test]
    fn runner_reuses_app_database_for_embedded_harvest_storage() {
        let app_config = AutumnConfig {
            database: autumn_web::config::DatabaseConfig {
                url: Some("postgres://autumn:autumn@localhost:5432/reddit".to_owned()),
                ..Default::default()
            },
            ..AutumnConfig::default()
        };
        let harvest_config = HarvestRuntimeConfig::default();

        assert_eq!(
            resolve_harvest_database_url(&app_config, &harvest_config)
                .expect("embedded mode should reuse app database"),
            "postgres://autumn:autumn@localhost:5432/reddit"
        );
    }

    #[test]
    fn detached_runner_state_carries_profile_and_pool() {
        let state = detached_runner_state("split-runner");

        assert_eq!(state.profile(), "split-runner");
    }

    #[test]
    fn runner_requires_app_database_url_for_reddit_clone_activities() {
        let error = resolve_app_database_url(&AutumnConfig::default())
            .expect_err("runner needs app db for reddit-clone activities");

        assert!(error.to_string().contains("database.url"));
    }
}
