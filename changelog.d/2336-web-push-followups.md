### Fixed

- **web push:** the `mailto:` VAPID subject check now parses the addr-spec — no
  whitespace, exactly one `@`, both halves non-empty — instead of checking
  `@` positions. Values like `mailto:ops team@example.com` or
  `mailto:a@@example.com` now fail at boot rather than signing every delivery
  with a subject the push service will refuse. Single-label domains stay
  accepted: the shipped default is `mailto:admin@localhost`.
- **pwa:** the generated service worker's `notificationclick` handler no longer
  throws on a malformed `url` in the payload. The click falls back to the app
  root instead of dying silently after dismissing the notification.
