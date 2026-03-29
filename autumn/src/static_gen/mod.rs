pub mod build;
mod middleware;
mod types;

pub use build::{BuildError, render_static_routes};
pub use middleware::StaticFileLayer;
pub use types::{
    ManifestEntry, ParamsFn, StaticManifest, StaticParams, StaticRouteMeta, url_to_file_path,
};
