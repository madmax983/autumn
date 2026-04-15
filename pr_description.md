🎯 **What:** Fixed timing leak in verify_password dummy hash approach.

⚠️ **Risk:** The previous approach allowed timing attacks. When parsing failed instantly, it was falling back to calculating a dummy hash, and then moving into a tokio spawn_blocking to fail instantly and compute the dummy hash. The instant fail leaked information that could be measured, which an attacker could use to enumerate valid user accounts by measuring the differences in time.

🛡️ **Solution:** The fix parses the format of the hash *outside* the blocking task. It validates the hash length (60 characters) and that it starts with `$`. If valid, we pass the user's hash into `spawn_blocking`. If invalid, we pass a known-valid dummy hash. This ensures exactly one `bcrypt::verify` computation occurs within the blocking thread regardless of whether the user exists or not, standardizing the delay and closing the timing leak.
