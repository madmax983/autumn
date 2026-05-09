#!/usr/bin/env bash
# Run `cargo package` for every publishable crate to verify it can be
# assembled into a .crate archive without network access or actual upload.
#
# This catches: missing files referenced from Cargo.toml, broken include/
# exclude patterns, workspace-path-dep leakage, and manifest parse errors
# that would cause a `cargo publish` to fail after a release tag is cut.
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
# autumn-macros has no Autumn deps so it publishes first.
# autumn-web depends on autumn-macros.
# plugins depend on autumn-web.
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
  echo "==> cargo package -p $crate"
  # --no-verify skips running tests inside the packaged crate (we run them
  # separately in CI).  --allow-dirty lets this run on a working tree with
  # uncommitted changes during local pre-release checks.
  if cargo package -p "$crate" --no-verify --allow-dirty 2>&1; then
    echo "  PASS: $crate packaged successfully"
  else
    echo "  FAIL: $crate could not be packaged" >&2
    failures=$((failures + 1))
  fi
done

echo ""
if [[ "$failures" -gt 0 ]]; then
  die "$failures crate(s) failed dry-run packaging."
fi

echo "Package dry-run OK — all publishable crates can be assembled."
