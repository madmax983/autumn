use autumn_web::prelude::*;
use autumn_web::test::TestApp;
use autumn_web::time::TickingClock;
use chrono::{TimeZone, Utc};
use std::time::Duration;

#[get("/api/info", api_version = "v1")]
async fn api_info_v1() -> &'static str {
    "v1 info"
}

#[get("/api/info-opt", api_version = "v1", sunset_opt_out = true)]
async fn api_info_v1_opt() -> &'static str {
    "v1 info opt"
}

#[get(
    "/api/sunset-only-opt",
    api_version = "v_sunset_only",
    sunset_opt_out = true
)]
async fn api_sunset_only_opt() -> &'static str {
    "sunset only opt"
}

#[get("/api/plain")]
async fn api_plain() -> &'static str {
    "plain info"
}

#[tokio::test]
async fn test_api_versioning_integration() {
    let start_time = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
    let deprecated_at = Utc.with_ymd_and_hms(2026, 6, 2, 0, 0, 0).unwrap();
    let sunset_at = Utc.with_ymd_and_hms(2026, 6, 3, 0, 0, 0).unwrap();

    let clock = TickingClock::starting_at(start_time);

    let client = TestApp::new()
        .routes(routes![
            api_info_v1,
            api_info_v1_opt,
            api_plain,
            api_sunset_only_opt
        ])
        .api_version(autumn_web::app::ApiVersion {
            version: "v1".to_string(),
            deprecated_at: Some(deprecated_at),
            sunset_at: Some(sunset_at),
        })
        .api_version(autumn_web::app::ApiVersion {
            version: "v_sunset_only".to_string(),
            deprecated_at: None,
            sunset_at: Some(sunset_at),
        })
        .with_clock(clock.clone())
        .build();

    // 1. Untagged route behaves as normal
    let resp = client.get("/api/plain").send().await;
    resp.assert_ok();
    assert!(resp.header("Deprecation").is_none());
    assert!(resp.header("Sunset").is_none());

    // 2. Tagged route behaves normally before deprecation
    let resp = client.get("/api/info").send().await;
    resp.assert_ok();
    assert!(resp.header("Deprecation").is_none());
    assert!(resp.header("Sunset").is_none());

    // 3. Move past deprecation: Deprecation: structured date, Sunset: date headers are emitted
    client.advance_clock(Duration::from_secs(24 * 3600 + 10)); // past deprecation (June 2)
    let resp = client.get("/api/info").send().await;
    resp.assert_ok();
    let expected_deprecation_header = format!("@{}", deprecated_at.timestamp());
    assert_eq!(
        resp.header("Deprecation").unwrap(),
        expected_deprecation_header
    );
    let expected_sunset_header = sunset_at.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
    assert_eq!(resp.header("Sunset").unwrap(), expected_sunset_header);

    // 4. Move past sunset: should return 410 Gone with Sunset header
    client.advance_clock(Duration::from_secs(24 * 3600)); // past sunset (June 3)
    let resp = client.get("/api/info").send().await;
    resp.assert_status(410);
    assert_eq!(resp.header("Sunset").unwrap(), expected_sunset_header);
    assert_eq!(
        resp.header("Deprecation").unwrap(),
        expected_deprecation_header
    );

    let body = resp.text();
    assert!(body.contains("autumn.gone"));

    // 5. Opt-out route past sunset bypasses 410 Gone but still emits headers
    let resp = client.get("/api/info-opt").send().await;
    resp.assert_ok();
    assert_eq!(
        resp.header("Deprecation").unwrap(),
        expected_deprecation_header
    );
    assert_eq!(resp.header("Sunset").unwrap(), expected_sunset_header);

    // 6. Opt-out route past sunset (with no deprecation_at configured) still emits Deprecation header
    let resp = client.get("/api/sunset-only-opt").send().await;
    resp.assert_ok();
    let expected_sunset_deprecation_header = format!("@{}", sunset_at.timestamp());
    assert_eq!(
        resp.header("Deprecation").unwrap(),
        expected_sunset_deprecation_header
    );
    assert_eq!(resp.header("Sunset").unwrap(), expected_sunset_header);
}
