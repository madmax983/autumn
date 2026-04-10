use std::collections::HashMap;

use redis::AsyncCommands;
use redis::aio::{ConnectionManager, ConnectionManagerConfig};

use crate::session::{SessionBackendConfigError, SessionConfig, SessionStore, SessionStoreError};

#[derive(Clone, Debug)]
pub struct RedisStore {
    connection: ConnectionManager,
    key_prefix: String,
    ttl_secs: u64,
}

impl RedisStore {
    pub(crate) fn from_config(config: &SessionConfig) -> Result<Self, SessionBackendConfigError> {
        let url = config
            .redis
            .url
            .clone()
            .filter(|url| !url.trim().is_empty())
            .ok_or(SessionBackendConfigError::MissingRedisUrl)?;
        let client = redis::Client::open(url)
            .map_err(|error| SessionBackendConfigError::InvalidRedisUrl(error.to_string()))?;
        let connection =
            ConnectionManager::new_lazy_with_config(client, ConnectionManagerConfig::new())
                .map_err(|error| SessionBackendConfigError::InvalidRedisUrl(error.to_string()))?;

        Ok(Self {
            connection,
            key_prefix: config.redis.key_prefix.clone(),
            ttl_secs: config.max_age_secs,
        })
    }

    fn key_for(&self, id: &str) -> String {
        format!("{}:{id}", self.key_prefix)
    }
}

impl SessionStore for RedisStore {
    async fn load(&self, id: &str) -> Result<Option<HashMap<String, String>>, SessionStoreError> {
        let mut connection = self.connection.clone();
        let key = self.key_for(id);
        match connection.get::<_, Option<String>>(&key).await {
            Ok(Some(payload)) => match serde_json::from_str::<HashMap<String, String>>(&payload) {
                Ok(session) => Ok(Some(session)),
                Err(error) => Err(SessionStoreError::backend(
                    "deserialize session payload",
                    format!("{key}: {error}"),
                )),
            },
            Ok(None) => Ok(None),
            Err(error) => Err(SessionStoreError::backend(
                "load session",
                format!("{key}: {error}"),
            )),
        }
    }

    async fn save(&self, id: &str, data: HashMap<String, String>) -> Result<(), SessionStoreError> {
        let mut connection = self.connection.clone();
        let key = self.key_for(id);
        match serde_json::to_string(&data) {
            Ok(payload) => {
                connection
                    .set_ex::<_, _, ()>(&key, payload, self.ttl_secs)
                    .await
                    .map_err(|error| {
                        SessionStoreError::backend("save session", format!("{key}: {error}"))
                    })?;
                Ok(())
            }
            Err(error) => Err(SessionStoreError::backend(
                "serialize session payload",
                format!("{key}: {error}"),
            )),
        }
    }

    async fn destroy(&self, id: &str) -> Result<(), SessionStoreError> {
        let mut connection = self.connection.clone();
        let key = self.key_for(id);
        connection.del::<_, ()>(&key).await.map_err(|error| {
            SessionStoreError::backend("destroy session", format!("{key}: {error}"))
        })
    }
}
