### Fixed

- **`autumn upgrade`:** publishing is now compare-and-swap (issue #2342).
  Replacing a scaffold file re-verifies the destination at the moment of the
  swap, so a file that changed while the replacement was being staged is
  refused with the usual partial-apply error instead of silently reverting
  whatever landed in between. The manifest is now read, merged, and published
  with a bounded retry: `pinned` merges as a set union and each digest resolves
  against the bytes on disk, so two concurrent runs — two `--accept`s pinning
  different files, or two applies touching different files — no longer lose
  each other's work.
