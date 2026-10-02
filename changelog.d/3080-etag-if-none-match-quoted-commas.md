### Fixed

- **`fresh_when` / `EtagLayer`:** `If-None-Match` is now split on commas
  *outside* quoted strings instead of on every comma. A server-issued `ETag`
  containing a comma (e.g. `"a,b"`) now matches its own echo (before: it never
  304'd), and a tag like `"a"` no longer falsely matches `"a,b"` (before: a
  stale 304 could be served for a changed resource). Per RFC 9110 §8.8.3 /
  §13.1.2. (issue #3080)
