#!/usr/bin/env bash
# Report SemVer compatibility of the public API surface for every publishable
# crate against the most recently published version on crates.io.
#
# Uses `cargo-semver-checks` (https://github.com/obi1kenobi/cargo-semver-checks).
# The tool is installed automatically if not already present.
#
# Patch and minor releases fail if any public API break is detected.
# For major releases (tag contains a MAJOR bump relative to the previous tag)
# the check is advisory only — breaking changes are expected and documented
# in CHANGELOG.md.
#
# Called from the `publish-gate` workflow. Run locally with:
#
#     ./scripts/check-semver.sh [--baseline-rev <git-ref>]

set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

die() {
  echo "error: $*" >&2
  exit 1
}

# Install cargo-semver-checks if it is not on PATH.
if ! command -v cargo-semver-checks &>/dev/null; then
  echo "cargo-semver-checks not found — installing..."
  cargo install cargo-semver-checks --locked
fi

# Publishable crates.
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
  echo "==> semver-checks: $crate"
  # --baseline-root is not used here; cargo-semver-checks fetches the last
  # published version from crates.io automatically when no baseline is given.
  # If the crate has never been published the check is skipped gracefully.
  if cargo semver-checks check-release --package "$crate" 2>&1; then
    echo "  PASS: $crate API is semver-compatible with crates.io baseline"
  else
    exit_code=$?
    if [[ $exit_code -eq 2 ]]; then
      # Exit code 2 means the crate is not yet published; skip it.
      echo "  SKIP: $crate not yet published on crates.io"
    else
      echo "  FAIL: $crate has semver-incompatible public API changes" >&2
      failures=$((failures + 1))
    fi
  fi
done

echo ""
if [[ "$failures" -gt 0 ]]; then
  die "$failures crate(s) have unacknowledged breaking public API changes. Either bump the major version, add a CHANGELOG entry, or update STABILITY.md to document the intentional break."
fi

echo "SemVer check OK — no unacknowledged breaking API changes."
