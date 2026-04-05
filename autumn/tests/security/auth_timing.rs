use std::time::Instant;

use autumn_web::auth::{hash_password, verify_password};

#[tokio::test]
async fn test_verify_password_timing() {
    let password = "test_password";
    let valid_hash = hash_password(password).await.unwrap();

    let mut valid_durations = vec![];
    for _ in 0..3 {
        let start = Instant::now();
        let _ = verify_password(password, &valid_hash).await;
        valid_durations.push(start.elapsed());
    }

    let mut invalid_durations = vec![];
    for _ in 0..3 {
        let start = Instant::now();
        let _ = verify_password(password, "invalid_hash_format").await;
        invalid_durations.push(start.elapsed());
    }

    // Use the minimum duration of the samples to filter out scheduling noise from the CI environment.
    let min_valid = valid_durations.iter().min().unwrap();
    let min_invalid = invalid_durations.iter().min().unwrap();

    println!("Valid hash durations: {valid_durations:?}");
    println!("Invalid hash durations: {invalid_durations:?}");
    println!("Min valid: {min_valid:?}, Min invalid: {min_invalid:?}");

    // The invalid duration should be roughly similar to the valid duration.
    // If it's less than 20% of the valid duration, it's definitely a fast-fail.
    // We use a forgiving ratio (1/5) because bcrypt operations can vary significantly,
    // and we just want to prove it's not returning instantly (< 1ms vs ~1s).
    assert!(
        *min_invalid > *min_valid / 5,
        "VULNERABILITY: verify_password failed too quickly on invalid hash format! Valid min: {min_valid:?}, Invalid min: {min_invalid:?}",
    );
}
