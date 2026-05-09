#!/usr/bin/env bash
# Verify that every publishable crate's manifest is valid and all referenced
# source files exist, without uploading to crates.io.
#
# Uses `cargo package --list` which enumerates the files that would be included
# in the .crate archive.  This catches: missing files referenced from Cargo.toml,
# broken include/exclude patterns, and manifest parse errors — without attempting
# registry dependency resolution.
#
# NOTE: We intentionally avoid `cargo package --no-verify` here because that
# command resolves dependencies against the published registry. Plugin crates
# (autumn-admin-plugin, autumn-storage-s3, autumn-cache-redis) depend on
# autumn-web features that may not yet be published at the time this gate runs,
# which would cause false failures. The `check-crate-metadata.sh` script already
# verifies inter-crate version pin alignment; the packaging step here focuses on
# file and manifest integrity.
#
# Called from the `publish-gate` workflow. Run locally with:
#
#     ./scripts/check-publish-dry-run.sh

set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

die() {
  echo "error: $*" >&2
  exit 1
}

# Publishable crates in dependency order.
CRATES=(
  autumn-macros
  autumn-web
  autumn-cli
  autumn-admin-plugin
  autumn-storage-s3
  autumn-cache-redis
)

failures=0

for crate in "${CRATES[@]}"; do
  echo ""
  echo "==> cargo package --list -p $crate"
  # --list enumerates the files that would be in the archive.
  # --allow-dirty lets this run on a working tree with uncommitted changes.
  if cargo package -p "$crate" --list --allow-dirty 2>&1; then
    echo "  PASS: $crate manifest and files are valid"
  else
    echo "  FAIL: $crate could not be listed for packaging" >&2
    failures=$((failures + 1))
  fi
done

echo ""
if [[ "$failures" -gt 0 ]]; then
  die "$failures crate(s) failed packaging file verification."
fi

echo "Package dry-run OK — all publishable crates have valid manifests and files."
