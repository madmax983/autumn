use autumn_web::auth::{hash_password, verify_password};
use std::time::{Duration, Instant};

#[tokio::test]
async fn eris_timing_attack() {
    let password = "my_secure_password";
    let valid_hash = hash_password(password).await.unwrap();

    let start_valid = Instant::now();
    let _ = verify_password(password, &valid_hash).await;
    let time_valid = start_valid.elapsed();

    let start_invalid = Instant::now();
    let _ = verify_password(password, "not_a_valid_hash_at_all_which_is_bad").await;
    let time_invalid = start_invalid.elapsed();

    println!("Time with valid hash: {time_valid:?}");
    println!("Time with invalid hash: {time_invalid:?}");

    // If time_invalid is less than a small threshold (e.g. 5ms), it returned instantly
    assert!(
        time_invalid > Duration::from_millis(10),
        "[ERIS-VULN] verify_password returns instantly on invalid hash, exposing a timing attack! Invalid: {time_invalid:?}, Valid: {time_valid:?}",
    );
}
