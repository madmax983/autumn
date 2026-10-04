### Fixed

- `autumn-billing`: the plan gate (`Billing::is_entitled` / `require`, and
  the `Entitled<R>` extractor) now evaluates the rule against every
  subscription view of the customer instead of testing only the newest one.
  A customer with two entitled subscriptions on different plans was denied a
  rule that only an older subscription satisfied. `require` now returns the
  best-ranked subscription that satisfies the rule; `current_subscription`
  keeps its presentation-pick semantics for the `GET /subscription` display.
