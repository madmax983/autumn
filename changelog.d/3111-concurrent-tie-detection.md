### Fixed

- **billing:** same-second subscription status ties are now detected atomically
  inside the store's guarded write, so two concurrent webhooks can no longer
  both miss the tie and fall back to rank ordering (issue #3111).
  `upsert_subscription` reports `Write::Tie` when the incoming and stored
  snapshots share an instant with different non-terminal statuses; the
  reconciler asks the provider for the subscription's current state and
  re-submits it as an authoritative write at the tied instant. When the
  provider cannot be asked, the write is re-submitted with
  `SubscriptionUpsert::with_tie_ranked()`, which restores the previous rank
  ordering. The memory and DB stores already report `Write::Tie`; a custom
  `BillingStore` should surface it too (it falls out of `guard_subscription`'s
  new `Guard::Tie` decision), and callers that match on `Write` should handle
  the new variant.
