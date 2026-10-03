### Fixed

- A database unique-constraint violation that reaches `AutumnError` is now a
  `409 Conflict` instead of an unhandled `500` (issue #2573). A concurrent
  identical-title write to a generated JSON API — or any other handler that
  lets a `diesel::result::Error` unique violation propagate through `?` —
  now responds `409` with the client-safe message "A record with these
  values already exists." instead of a server error. The original database
  error stays reachable as `source()` for logs and error reporting, and
  [`unique_violation_field`](https://docs.rs/autumn-web/latest/autumn_web/error/fn.unique_violation_field.html)
  keeps mapping the violation to a field error through the new wrapper.
