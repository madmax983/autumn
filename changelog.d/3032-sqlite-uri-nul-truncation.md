### Fixed

- **db: SQLite URI query parsing now stops at a percent-decoded NUL.**
  `sqlite_uri_has_query_pair` (the shared-cache boot warning and the
  `statement_timeout` rejection wording) already mirrored SQLite's URI
  parsing — `file:`-only queries, fragment dropping, percent-decoding, and
  last-value-wins — but not SQLite's rule that interpretation stops at the
  first decoded `%00`. `file::memory:?cache=shared&cache=private%00` was
  read as private-cache (the later pair won) where SQLite treats it as
  shared-cache. The pair carrying the NUL and every pair after it are now
  ignored.
