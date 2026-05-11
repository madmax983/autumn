# Signed Webhooks Example

This example shows a Stripe-style signed webhook endpoint using Autumn's
`SignedWebhook` extractor.

## Prerequisites

- Rust 1.88.0+

## Quick start

Run the app:

```bash
cargo run -p signed-webhooks-example
```

Run the fixture tests:

```bash
cargo test -p signed-webhooks-example
```

The tests cover valid, tampered-body, stale-timestamp, bad-signature, and
duplicate-delivery webhook requests.
