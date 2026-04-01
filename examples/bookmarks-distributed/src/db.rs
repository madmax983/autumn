use diesel_async::AsyncPgConnection;
use diesel_async::pooled_connection::AsyncDieselConnectionManager;
use diesel_async::pooled_connection::deadpool::Pool;
use std::error::Error;
use std::fmt;

use crate::config::{DistributedConfig, MissingDistributedDatabaseUrls};

pub type PoolError = diesel_async::pooled_connection::deadpool::BuildError;

#[derive(Debug)]
pub enum DualPoolError {
    MissingUrls(MissingDistributedDatabaseUrls),
    Build(PoolError),
}

impl fmt::Display for DualPoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingUrls(error) => error.fmt(f),
            Self::Build(error) => error.fmt(f),
        }
    }
}

impl Error for DualPoolError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::MissingUrls(error) => Some(error),
            Self::Build(error) => Some(error),
        }
    }
}

impl From<MissingDistributedDatabaseUrls> for DualPoolError {
    fn from(error: MissingDistributedDatabaseUrls) -> Self {
        Self::MissingUrls(error)
    }
}

impl From<PoolError> for DualPoolError {
    fn from(error: PoolError) -> Self {
        Self::Build(error)
    }
}

pub struct DualPools {
    primary: Pool<AsyncPgConnection>,
    replica: Pool<AsyncPgConnection>,
}

impl DualPools {
    #[must_use]
    pub fn primary(&self) -> &Pool<AsyncPgConnection> {
        &self.primary
    }

    #[must_use]
    pub fn replica(&self) -> &Pool<AsyncPgConnection> {
        &self.replica
    }

    #[must_use]
    pub fn primary_pool_size(&self) -> usize {
        self.primary.status().max_size
    }

    #[must_use]
    pub fn replica_pool_size(&self) -> usize {
        self.replica.status().max_size
    }
}

fn build_pool(url: &str, size: usize) -> Result<Pool<AsyncPgConnection>, PoolError> {
    let manager = AsyncDieselConnectionManager::<AsyncPgConnection>::new(url);
    Pool::builder(manager).max_size(size).build()
}

pub fn create_dual_pools(config: &DistributedConfig) -> Result<DualPools, DualPoolError> {
    let (primary_url, replica_url) = config.database.urls()?;

    Ok(DualPools {
        primary: build_pool(primary_url, config.database.primary_pool_size)?,
        replica: build_pool(replica_url, config.database.replica_pool_size)?,
    })
}

#[cfg(test)]
mod tests {
    use super::{DualPools, create_dual_pools};
    use crate::config::DistributedConfig;

    #[test]
    fn builds_primary_and_replica_pools() {
        let config = DistributedConfig::from_urls(
            "postgres://autumn:autumn@localhost:5432/bookmarks_distributed_primary",
            "postgres://autumn:autumn@localhost:5432/bookmarks_distributed_replica",
        )
        .with_pool_sizes(4, 2);

        let pools: DualPools = create_dual_pools(&config).expect("dual pools should build");

        assert_eq!(pools.primary().status().max_size, 4);
        assert_eq!(pools.replica().status().max_size, 2);
        assert_eq!(pools.primary_pool_size(), 4);
        assert_eq!(pools.replica_pool_size(), 2);
    }
}
