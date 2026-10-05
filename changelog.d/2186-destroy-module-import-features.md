### Fixed

- **generate:** `autumn destroy scaffold` no longer strips the `storage` or `markdown` autumn-web features while hand-written code imports the module (`use autumn_web::storage;`, `use autumn_web::markdown as md;`). The feature markers carried a trailing `::` the module-import line never contains, so `destroy` removed the feature and the build broke in a file the generator did not write (issue #2186).
