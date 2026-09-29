### Fixed

- **`autumn upgrade`:** a preview no longer renders `build.rs`'s scaffold
  diff against the pre-codemod bytes still on disk (issue #2343). When this
  run's app-code migrations rewrite `build.rs`, the preview now diffs it
  against the codemods' planned contents — the bytes `--apply` will write —
  so the preview shows the same conflict diff the apply then produces.
