1. **Fix missing documentation for `ConfigError` and `ConfigEntry` fields in `autumn/src/runtime_config.rs`**.
   - Add comments indicating the purpose of `ValidationFailed` fields `key` and `reason`.
   - Add comments to `ConfigEntry` fields `name`, `value_type`, `current`, `default`, `is_overridden`, and `description`.
2. **Fix missing documentation for `FieldError` fields in `autumn/src/data/csv.rs`**.
   - Add comments indicating the purpose of `FieldError` fields `column` and `message`.
3. **Fix missing documentation for `WEBHOOK_REPLAY_KEY` in `autumn/src/webhook.rs`**.
   - Add comments explaining its purpose.
4. **Fix missing documentation for `WebhookSubscriptionStatus` variants and fields in `autumn/src/webhook_outbound.rs`**.
   - Add comments for `Active`, `Disabled`, and `Failed` variants.
   - Add comments for `WebhookSubscription` and `WebhookDeliveryLog` fields.
5. **Fix missing documentation for interceptor methods in `autumn/src/test.rs`**.
   - Add comments for `with_mail_interceptor`, `with_job_interceptor`, `with_db_interceptor`, `with_channels_interceptor`, and `with_http_interceptor`.
6. **Fix unresolved intra-doc links in `autumn/src/reporting.rs`**.
   - Update unresolved links to `ErrorEvent`, `ErrorReporter`, `LogReporter`, and `ReportingLayer` using absolute paths (e.g. `crate::reporting::ErrorEvent`).
7. **Fix unresolved intra-doc link in `autumn/src/system_test.rs`**.
   - Change `AppState::for_test()` to `crate::AppState::for_test()`.
8. **Fix ambiguous intra-doc link in `autumn/src/lib.rs`**.
   - Use `macro@inbound_mail` instead of just `inbound_mail`.
9. **Complete pre-commit steps to ensure proper testing, verification, review, and reflection are done**.
10. **Submit PR with Bard styling**.
