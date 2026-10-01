### Fixed

- **etag:** `ETag::matches_if_none_match` splits `If-None-Match` on commas
  outside quoted entity-tags (RFC 9110 §13.1.2), instead of a bare
  `split(',')` that treated a comma inside a quoted tag as a list separator
  (issue #3080). A hand-built tag like `ETag::strong("a,b")` now matches its
  own echoed header (previously a never-304), and a different tag is no
  longer a false prefix match (previously a stale-body 304). Unquoted
  candidates stay accepted leniently, as before.
