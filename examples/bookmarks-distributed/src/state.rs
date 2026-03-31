use crate::config::DistributedConfig;
use crate::db::DualPools;
use std::sync::Arc;
use std::sync::OnceLock;

pub struct DistributedState {
    pub config: DistributedConfig,
    pub pools: DualPools,
}

impl DistributedState {
    #[must_use]
    pub fn new(config: DistributedConfig, pools: DualPools) -> Self {
        Self { config, pools }
    }

    pub fn install_global(self: Arc<Self>) -> Result<Arc<Self>, DistributedStateInstallError> {
        Self::global_slot()
            .set(self.clone())
            .map_err(|_| DistributedStateInstallError::AlreadyInstalled)?;
        Ok(self)
    }

    #[must_use]
    pub fn global() -> Option<Arc<Self>> {
        Self::global_slot().get().cloned()
    }

    fn global_slot() -> &'static OnceLock<Arc<Self>> {
        static GLOBAL: OnceLock<Arc<DistributedState>> = OnceLock::new();

        &GLOBAL
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistributedStateInstallError {
    AlreadyInstalled,
}

impl std::fmt::Display for DistributedStateInstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyInstalled => f.write_str("distributed state is already installed"),
        }
    }
}

impl std::error::Error for DistributedStateInstallError {}

#[cfg(test)]
mod tests {
    use super::DistributedState;
    use crate::config::DistributedConfig;
    use crate::db::create_dual_pools;
    use std::sync::Arc;

    #[test]
    fn state_keeps_config_and_dual_pools_together() {
        let config = DistributedConfig::from_urls(
            "postgres://autumn:autumn@localhost:5432/bookmarks_distributed_primary",
            "postgres://autumn:autumn@localhost:5432/bookmarks_distributed_replica",
        )
        .with_pool_sizes(5, 5);
        let pools = create_dual_pools(&config).expect("dual pools should build");

        let state = DistributedState::new(config.clone(), pools);

        assert_eq!(
            state.config.database.primary_url.as_deref(),
            Some("postgres://autumn:autumn@localhost:5432/bookmarks_distributed_primary")
        );
        assert_eq!(state.pools.primary_pool_size(), 5);
        assert_eq!(state.pools.replica_pool_size(), 5);
    }

    #[test]
    fn install_global_is_single_assignment() {
        let config = DistributedConfig::from_urls(
            "postgres://autumn:autumn@localhost:5432/bookmarks_distributed_primary",
            "postgres://autumn:autumn@localhost:5432/bookmarks_distributed_replica",
        )
        .with_pool_sizes(2, 2);
        let pools = create_dual_pools(&config).expect("dual pools should build");
        let state = Arc::new(DistributedState::new(config, pools));
        let original = state.clone();

        let installed = state
            .clone()
            .install_global()
            .expect("first install should work");
        let shared = DistributedState::global().expect("global state should be installed");
        let second_install = state.install_global();

        assert!(Arc::ptr_eq(&installed, &original));
        assert!(Arc::ptr_eq(&shared, &original));
        assert!(matches!(
            second_install,
            Err(super::DistributedStateInstallError::AlreadyInstalled)
        ));
    }
}
