### Fixed

- **scaffold:** a `Bytea` column's CSV export/import round trip is now
  byte-identical (issue #2330). The export rendered the column with
  `String::from_utf8_lossy`, destroying every byte that is not valid UTF-8 —
  importing the app's own export silently replaced a binary column with
  mojibake, and the column was excluded from the import entirely (a
  non-nullable `Bytea` refused `--import` outright). The export now writes
  base64 (standard alphabet, with padding), still behind the `csv_text_cell`
  formula guard, and the import decodes it back through the same `into_new`
  the browser path uses, so the column is an ordinary settable import column.
  The `{Pascal}Form` representation is the same base64 text — the edit form
  shows and accepts it — which also fixes the pre-existing browser
  edit-and-save mojibake for binary columns. The index and show views keep
  their lossy rendering. Scaffolds with a `Bytea` column gain a `base64`
  dependency.
