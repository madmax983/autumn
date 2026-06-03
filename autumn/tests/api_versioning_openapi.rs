//! OpenAPI versioning integration tests.
//!
//! Verifies that OpenAPI spec generation correctly tags operations with their
//! API version names and sets the `deprecated: true` property when a route
//! is associated with a version whose deprecation date is in the past.

#![cfg(feature = "openapi")]

use autumn_web::openapi::OpenApiConfig;
use autumn_web::prelude::*;
use autumn_web::app::ApiVersion;
use chrono::{TimeZone, Utc};

#[get("/v1/items", api_version = "v1")]
async fn get_v1_items() -> &'static str {
    "v1"
}

#[get("/v2/items", api_version = "v2")]
async fn get_v2_items() -> &'static str {
    "v2"
}

#[test]
fn test_openapi_spec_version_tagging_and_deprecation() {
    let route_v1 = __autumn_route_info_get_v1_items();
    let route_v2 = __autumn_route_info_get_v2_items();

    // v1 is deprecated (2020), v2 is active (2035)
    let v1 = ApiVersion {
        version: "v1".to_string(),
        deprecated_at: Some(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap()),
        sunset_at: Some(Utc.with_ymd_and_hms(2040, 1, 1, 0, 0, 0).unwrap()),
    };
    let v2 = ApiVersion {
        version: "v2".to_string(),
        deprecated_at: Some(Utc.with_ymd_and_hms(2035, 1, 1, 0, 0, 0).unwrap()),
        sunset_at: Some(Utc.with_ymd_and_hms(2040, 1, 1, 0, 0, 0).unwrap()),
    };

    let mut config = OpenApiConfig::new("Versioning Test API", "1.0.0");
    config.api_versions = vec![v1, v2];

    let docs = vec![&route_v1.api_doc, &route_v2.api_doc];
    let spec = autumn_web::openapi::generate_spec(&config, &docs);

    // v1 route path should exist
    assert!(spec.paths.contains_key("/v1/items"));
    let op_v1 = spec.paths["/v1/items"].get.as_ref().unwrap();
    // v1 tag should be appended
    assert!(op_v1.tags.contains(&"v1".to_string()));
    // v1 should be marked deprecated: true
    assert_eq!(op_v1.deprecated, Some(true));

    // v2 route path should exist
    assert!(spec.paths.contains_key("/v2/items"));
    let op_v2 = spec.paths["/v2/items"].get.as_ref().unwrap();
    // v2 tag should be appended
    assert!(op_v2.tags.contains(&"v2".to_string()));
    // v2 should NOT be marked deprecated (None)
    assert_eq!(op_v2.deprecated, None);
}
