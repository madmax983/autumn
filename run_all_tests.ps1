$ErrorActionPreference = "Stop"

rustup target add wasm32-unknown-unknown
cargo test -p autumn-web
cargo test -p autumn-wasm --target wasm32-unknown-unknown --no-run
