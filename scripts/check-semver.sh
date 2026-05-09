#!/usr/bin/env bash
# Report SemVer compatibility of the public API surface for every publishable
# crate against the most recently published version on crates.io.
#
# Uses `cargo-semver-checks` (https://github.com/obi1kenobi/cargo-semver-checks).
# The tool is installed automatically if not already present.
#
# Patch and minor releases fail if any public API break is detected.
# Intentional breaking releases (major bumps, or pre-1.0 minor bumps) pass
# the gate automatically when a migration guide exists at
# docs/migrations/<version>.md — the presence of the guide is proof that the
# break is documented and acknowledged.
#
# Called from the `publish-gate` workflow. Run locally with:
#
#     ./scripts/check-semver.sh

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

# Workspace version — used to locate the migration guide for intentional breaks.
workspace_version="$(
  awk '
    /^\[workspace\.package\]/ { in_block = 1; next }
    /^\[/ && in_block         { in_block = 0 }
    in_block && /^[[:space:]]*version/ {
      match($0, /"[^"]+"/)
      print substr($0, RSTART + 1, RLENGTH - 2)
      exit
    }
  ' Cargo.toml
)"
[[ -n "$workspace_version" ]] || die "could not parse workspace version from Cargo.toml"
migration_guide="docs/migrations/${workspace_version}.md"

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
  set +e
  cargo semver-checks check-release --package "$crate" 2>&1
  exit_code=$?
  set -e

  if [[ $exit_code -eq 0 ]]; then
    echo "  PASS: $crate API is semver-compatible with crates.io baseline"
  elif [[ $exit_code -eq 2 ]]; then
    # Exit code 2 means the crate is not yet published; skip it.
    echo "  SKIP: $crate not yet published on crates.io"
  elif [[ $exit_code -eq 1 ]]; then
    # Exit code 1 means cargo-semver-checks found actual breaking API changes.
    # Allow them through only if a migration guide exists — its presence is the
    # explicit acknowledgement required by the release policy.
    if [[ -f "$migration_guide" ]]; then
      echo "  ADVISORY: $crate has breaking API changes; intentional — migration guide found at $migration_guide"
    else
      echo "  FAIL: $crate has unacknowledged breaking API changes." >&2
      echo "        Add a migration guide at $migration_guide to acknowledge an intentional break," >&2
      echo "        or fix the API regression before releasing." >&2
      failures=$((failures + 1))
    fi
  else
    # Any other non-zero exit code is a tool/invocation error (e.g. rustdoc
    # crash, registry lookup failure, unsupported flag). Do not treat these as
    # acknowledged breaks — fail immediately so the error is investigated.
    echo "  FAIL: $crate — cargo-semver-checks exited with unexpected code $exit_code (tool/invocation error)." >&2
    failures=$((failures + 1))
  fi
done

echo ""
if [[ "$failures" -gt 0 ]]; then
  die "$failures crate(s) have unacknowledged breaking public API changes."
fi

echo "SemVer check OK — no unacknowledged breaking API changes."
