use std::time::Instant;

use autumn_web::auth::verify_password;

#[tokio::test]
async fn eris_auth_timing_poc() {
    // Generate a valid bcrypt hash
    let valid_hash = autumn_web::auth::hash_password("correct_horse_battery_staple")
        .await
        .unwrap();

    // Verify time for a valid hash
    let start_valid = Instant::now();
    let res_valid = verify_password("wrong_password", &valid_hash).await;
    let _duration_valid = start_valid.elapsed();
    assert!(res_valid.is_ok());
    assert!(!res_valid.unwrap());

    // Verify time for an invalid hash format
    let invalid_hash = "not_a_bcrypt_hash_at_all";
    let start_invalid = Instant::now();
    let res_invalid = verify_password("any_password", invalid_hash).await;
    let duration_invalid = start_invalid.elapsed();
    assert!(res_invalid.is_err());

    // Both should take a non-trivial amount of time because of bcrypt hashing.
    // E.g. at least 50ms depending on CPU speed for cost=12. We check that
    // duration_invalid is at least somewhat close to duration_valid.
    // If the vulnerability exists, duration_invalid would be micro-seconds (< 1ms).
    // Let's assert it takes at least 10ms.
    assert!(
        duration_invalid.as_millis() > 10,
        "Timing attack vulnerability: verification of invalid hash was too fast ({duration_invalid:?})"
    );
}
