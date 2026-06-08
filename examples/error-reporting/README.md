# error-reporting

Wiring a custom [`ErrorReporter`](https://docs.rs/autumn-web/latest/autumn_web/reporting/trait.ErrorReporter.html)
so unhandled panics and 5xx responses get shipped somewhere you'll see them
(Sentry, Slack, a custom sink) instead of scrolling past in the logs.

## Prerequisites

- Rust 1.88.0+

No database or external services required.

## Quick start

```sh
cargo run -p error-reporting
```

Then, in another shell:

```sh
curl -i localhost:3000/boom   # handler panics -> clean 500, one report
curl -i localhost:3000/fail   # returns a 5xx  -> clean 500, one report
curl -i localhost:3000/ok     # 200            -> not reported
```

Watch the server logs for the `[report]` lines printed by the custom
`ConsoleReporter`. The client always gets a clean RFC 7807 Problem Details
response — the panic payload never reaches the wire. Set `RUST_BACKTRACE=1` to
capture panic backtraces in the event.

## See also

The [error-reporting guide](../../docs/guide/error-reporting.md) for the full
story: the `ErrorEvent` shape, chaining multiple reporters, sampling, and how
the panic-catch layer fits the middleware stack.
