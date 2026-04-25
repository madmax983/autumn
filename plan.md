1.  **Refactor `cache_key` creation in `autumn/src/cache/layer.rs` and `autumn/src/middleware/metrics.rs` to optimize out `format!` heap allocations.**
    -   In `autumn/src/cache/layer.rs`'s `call` function, we currently create `let cache_key = format!("http:{}", req.uri());` on every GET request, which is a hot path for cache checking.
        -   By replacing `format!` with formatting into a stack-allocated buffer (`[u8; 256]`), we can check the cache hit condition without allocating a `String` on the heap.
        -   If there is a cache miss, we allocate a `String` to be moved into the async block.
    -   In `autumn/src/middleware/metrics.rs`'s `record` function, a stack-allocated string `key_str` is already created, but if `is_new` is true, it redundantly calls `let key = format!("{method} {route}");`, allocating again when `key_str` already contains the valid string representation.
        -   We can optimize the `is_new` block by using `key = if key_str.is_empty() { format!("{method} {route}") } else { key_str.to_owned() };` instead of running `format!` again.
2.  **Complete pre-commit steps to ensure proper testing, verification, review, and reflection are done.**
