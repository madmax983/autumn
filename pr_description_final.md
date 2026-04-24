🎯 **What:**
Added unit tests for the `hash_password` and `verify_password` functions in `autumn/src/auth.rs`.

📊 **Coverage:**
The new `test_hash_password` test explicitly covers the expected behaviors:
1. It hashes a known string and checks that the output string represents a valid bcrypt hash starting with `$2b$`.
2. It calls `verify_password` using the known string and confirms that verification succeeds.
3. It calls `verify_password` using an incorrect string and confirms that verification fails.

✨ **Result:**
The `hash_password` API is now directly covered by the test suite, allowing us to refactor with confidence if the underlying hashing algorithm changes.
