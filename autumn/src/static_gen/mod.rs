pub mod build;
mod middleware;
mod types;

pub use build::{render_static_routes, BuildError};
pub use middleware::StaticFileLayer;
pub use types::{ManifestEntry, StaticManifest, StaticRouteMeta, url_to_file_path};
