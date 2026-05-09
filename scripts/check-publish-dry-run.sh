#!/usr/bin/env bash
# Verify that every publishable crate's package archive can be assembled,
# without uploading to crates.io.
#
# Uses `cargo package --no-verify` to assemble the .crate archive without
# running cargo's build verification step. This catches missing files referenced
# from Cargo.toml, broken include/exclude patterns, manifest parse errors, and
# workspace-path leakage before release.
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
  echo "==> cargo package -p $crate --no-verify --allow-dirty"
  # --no-verify assembles the .crate archive without running cargo's build check.
  # --allow-dirty lets this run on a working tree with uncommitted changes.
  if cargo package -p "$crate" --no-verify --allow-dirty 2>&1; then
    echo "  PASS: $crate package archive can be assembled"
  else
    echo "  FAIL: $crate package archive could not be assembled" >&2
    failures=$((failures + 1))
  fi
done

echo ""
if [[ "$failures" -gt 0 ]]; then
  die "$failures crate(s) failed package archive assembly."
fi

echo "Package dry-run OK: all publishable crate archives can be assembled."
