#[autumn_web::main]
async fn main() {
    // Verify it compiles and the tokio runtime is set up.
    let _ = tokio::time::Instant::now();
}
