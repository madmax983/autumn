# outbound-http example

Demonstrates `autumn_web::http::Client` — a traced outbound HTTP client with
automatic retries and a mock harness for integration tests.

The example implements a `POST /charges` handler that calls a Stripe-style
charge endpoint and a `GET /status` liveness check.  Four integration tests
cover mocked calls, call-count assertions, the `NoMock` guard, and routing.

## Prerequisites

- Rust 1.88.0+
- (Optional) A Stripe sandbox key in `STRIPE_SECRET_KEY` for real calls

## Quick start

Run the server:

```sh
cargo run -p outbound-http
```

Make a charge (set `STRIPE_SECRET_KEY` first or omit for the sandbox default):

```sh
curl -X POST http://localhost:3000/charges \
     -H 'content-type: application/json' \
     -d '{"amount": 1000, "currency": "usd"}'
```

Check liveness:

```sh
curl http://localhost:3000/status
```

## Running the tests

```sh
cargo test -p outbound-http
```

All four tests use `TestApp::http_mock` — no real network required.
