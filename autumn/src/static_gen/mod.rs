mod middleware;
mod types;

pub use middleware::StaticFileLayer;
pub use types::{ManifestEntry, StaticManifest, StaticRouteMeta, url_to_file_path};
