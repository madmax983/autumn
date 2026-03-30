use serde::{Deserialize, Serialize};

/// Metadata for a browser island registered with the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IslandMeta {
    pub name: &'static str,
    pub mount_id: &'static str,
    pub props_type: &'static str,
}

/// Metadata for a generated server action endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionMeta {
    pub name: &'static str,
    pub path: &'static str,
}
