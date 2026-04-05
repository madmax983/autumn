use autumn_web::auth::{hash_password, verify_password};
use std::time::Instant;

#[tokio::test]
async fn test_verify_password_timing() {
    let password = "test_password";
    let valid_hash = hash_password(password).await.unwrap();

    // Time verification with a valid hash
    let start_valid = Instant::now();
    let _ = verify_password(password, &valid_hash).await;
    let valid_duration = start_valid.elapsed();

    // Time verification with an invalid hash format (e.g., dummy hash)
    let start_invalid = Instant::now();
    let _ = verify_password(password, "invalid_hash_format").await;
    let invalid_duration = start_invalid.elapsed();

    // The invalid duration should be roughly similar to the valid duration.
    // If it's less than 10% of the valid duration, it's definitely a fast-fail.
    println!("Valid hash duration: {valid_duration:?}");
    println!("Invalid hash duration: {invalid_duration:?}");

    assert!(
        invalid_duration > valid_duration / 2,
        "VULNERABILITY: verify_password failed instantly on invalid hash format! Valid: {valid_duration:?}, Invalid: {invalid_duration:?}",
    );
}
