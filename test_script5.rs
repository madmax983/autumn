use std::time::{SystemTime, Duration};

fn main() {
    let now = SystemTime::UNIX_EPOCH;
    let window = Duration::from_secs(60);

    // In code we do:
    let expires_at = now.checked_add(window).unwrap_or(now); // = UNIX_EPOCH + 60s

    // But since `check_and_insert` is running in real time, `SystemTime::now()` is not UNIX_EPOCH!
    let real_now = SystemTime::now();

    // The test hardcodes `SystemTime::UNIX_EPOCH` for `now`.
    // Wait, the webhook code uses `SystemTime::now()` in `check_and_insert_sync`:
    // let now = SystemTime::now(); // line 768

    // But then:
    // let expires_at = received_at.checked_add(window).unwrap_or(received_at); // line 772
    // `expires_at` is `UNIX_EPOCH + 60s`.
    // `now` is `2024-...` (a very large timestamp).

    // Then `cleanup_if_due(state, now)` uses `now` which is ~2024.
    // expires_at.duration_since(now).is_ok()
    // (UNIX_EPOCH + 60s).duration_since(2024) -> Err

    // So the legitimate key IS expired according to real time, because it was submitted at UNIX_EPOCH + 60s!
}
