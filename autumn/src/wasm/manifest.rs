use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::AutumnResult;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WasmIslandManifestEntry {
    pub mount_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WasmManifest {
    pub entry_js: Option<String>,
    pub entry_wasm: Option<String>,
    #[serde(default)]
    pub islands: BTreeMap<String, WasmIslandManifestEntry>,
}

impl WasmManifest {
    /// Load a WASM manifest from disk.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or if the JSON payload
    /// cannot be deserialized into [`WasmManifest`].
    pub fn load(path: &Path) -> AutumnResult<Self> {
        let bytes = std::fs::read(path)?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}
