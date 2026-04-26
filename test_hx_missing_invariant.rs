fn main() {
    println!("I need to rewrite `autumn/tests/security/fallback_middleware_bypass.rs` to use the actual router builder.");
    println!("Wait, earlier I tried to use `autumn_web::app().with_config_loader()` but `AppBuilder` didn't have `with_config_loader`, only `config_loader` or `config`? No, wait! `AppBuilder::config` takes an `AutumnConfig`.");
    println!("Let's look at `autumn/src/app.rs`.");
}
