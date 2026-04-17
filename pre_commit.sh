cargo fmt --all
cargo clippy -p autumn-web --all-targets --all-features -- -D warnings
cargo test -p autumn-web --all-features
