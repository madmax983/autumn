### Fixed

- **billing:** dunning exhaustion no longer skips `Unpaid`/provider-cancel
  when `invoice.payment_failed` arrives before the subscription event
  (issue #3081). The invoice keeps the provider subscription id even while
  the local subscription is not mirrored yet, and mirroring the
  subscription back-fills the local link on the pending invoices and their
  open dunning rows. Adds the `billing_invoice_provider_subscription_id`
  mirror migration (new column on `billing_invoices`).
