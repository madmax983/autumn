-- #3081: drop the provider subscription id kept on the invoice.
ALTER TABLE billing_invoices DROP COLUMN provider_subscription_id;
