-- #3081: keep the provider subscription id on the invoice when the failure
-- arrives before the subscription is mirrored, so the mirror step can
-- back-fill the local link later. `ADD COLUMN` is portable across Postgres
-- and SQLite.
ALTER TABLE billing_invoices ADD COLUMN provider_subscription_id TEXT;
