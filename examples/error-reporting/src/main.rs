//! Wiring a custom error reporter.
//!
//! Run it:
//!
//! ```sh
//! cargo run -p error-reporting
//! # then, in another shell:
//! curl -i localhost:3000/boom   # panics  -> clean 500, reported
//! curl -i localhost:3000/fail   # 5xx     -> clean 500, reported
//! curl -i localhost:3000/ok     # 200     -> not reported
//! ```
//!
//! Watch the server logs: every panic and 5xx prints a `[report]` line from the
//! custom reporter below, while the client always gets a clean RFC 7807
//! Problem Details 500. Set `RUST_BACKTRACE=1` to capture panic backtraces in
//! the event.

use autumn_web::prelude::*;
use autumn_web::reporting::{ErrorEvent, ErrorReporter, ReportFuture};

/// A custom reporter. In a real app this would post to Sentry/Slack/Honeycomb;
/// here it just prints. Wiring it up is the 4 lines of the `impl` below plus a
/// single `.with_error_reporter(..)` builder call.
struct ConsoleReporter;

impl ErrorReporter for ConsoleReporter {
    fn report<'a>(&'a self, event: &'a ErrorEvent) -> ReportFuture<'a> {
        Box::pin(async move {
            let kind = if event.panic.is_some() { "panic" } else { "error" };
            println!(
                "[report] {kind} {} {} {} (request_id={})",
                event.status,
                event.method.as_deref().unwrap_or("-"),
                event.route.as_deref().unwrap_or("-"),
                event.request_id.as_deref().unwrap_or("-"),
            );
        })
    }
}

#[get("/ok")]
async fn ok() -> &'static str {
    "ok"
}

#[get("/boom")]
async fn boom() -> &'static str {
    panic!("kaboom: something went very wrong in the handler");
}

#[get("/fail")]
async fn fail() -> Result<&'static str, AutumnError> {
    Err(AutumnError::internal_server_error_msg(
        "downstream service unavailable",
    ))
}

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .with_error_reporter(ConsoleReporter)
        .routes(routes![ok, boom, fail])
        .run()
        .await;
}
