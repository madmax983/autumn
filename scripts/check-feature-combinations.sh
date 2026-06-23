#!/usr/bin/env bash
# Check that each individual autumn-web Cargo feature compiles in isolation, and
# that a curated set of representative real-world feature combinations compile,
# without building the full 2^N powerset (which is cost-prohibitive at ~35 flags).
#
# Bounded strategy:
#   Phase A — each-feature sweep via `cargo hack --each-feature --no-default-features`
#             This builds once with NO features, then once per individual feature.
#             Linear in the number of features; ~25 builds after exclusions.
#   Phase B — curated real-world combinations (5 fixed combos that mirror documented
#             user journeys: db-only API, minimal web, mail, storage+db, telemetry).
#             Each is a discrete `cargo check` so a failure names the exact
#             --features string.
#
# Excluded from Phase A (documented as unsupported-in-CI in STABILITY.md):
#   managed-pg          — downloads Postgres binaries on first run (CI build cost)
#   managed-pg-bundled  — embeds Postgres binaries into the binary (~150 MB, huge)
#   system-tests        — pulls chromiumoxide (headless Chromium)
#   test-support        — dev-only; pulls testcontainers
#
# telemetry-otlp is explicitly gated in Phase B. It requires protoc; install with:
#   sudo apt-get install -y protobuf-compiler
# or use the workflow which installs it automatically.
#
# Called from the `feature-combinations` workflow. Run locally with:
#
#     cargo install cargo-hack            # one-time setup (if not already installed)
#     ./scripts/check-feature-combinations.sh

set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

die() {
  echo "error: $*" >&2
  exit 1
}

ok()   { echo "ok:    $*"; }
warn() { echo "warn:  $*" >&2; }

# ── Preflight: cargo-hack must be available ────────────────────────────────────
if ! cargo hack --version &>/dev/null; then
  die "cargo-hack not found. Install with: cargo install cargo-hack
  In CI, use: taiki-e/install-action@cargo-hack"
fi

echo "Feature-combination compile gate — autumn-web"
echo "cargo-hack $(cargo hack --version)"
echo ""

# ── Phase A: each-feature sweep ───────────────────────────────────────────────
# Includes: --no-default-features alone (zero-feature build) + each feature alone.
# Excluded: system-dep / build-cost heavy features (see header comment).

echo "==> Phase A: each-feature sweep (no features + each feature alone)"
echo "    Note: --each-feature implies --no-default-features for each sub-build"
echo "    Excluded: managed-pg,managed-pg-bundled,system-tests,test-support"
echo ""

if ! cargo hack check \
  -p autumn-web \
  --no-dev-deps \
  --each-feature \
  --exclude-features managed-pg-bundled,managed-pg,system-tests,test-support \
  2>&1; then
  echo ""
  echo "::error::Phase A failed — a feature did not compile in isolation."
  echo "         See the cargo-hack output above for the exact --features string."
  exit 1
fi

echo ""
ok "Phase A passed — every gated feature compiles in isolation."
echo ""

# ── Phase B: curated real-world combinations ───────────────────────────────────
# Each line is an independently documented user journey.
# Failures print the exact --features string so the break is immediately actionable.

CURATED_COMBOS=(
  "db"
  "mail"
  "storage,db"
  "maud,htmx"
  "telemetry-otlp"
)

echo "==> Phase B: curated real-world feature combinations"
echo ""

phase_b_failures=0

for combo in "${CURATED_COMBOS[@]}"; do
  echo "    checking: --no-default-features --features $combo"
  if cargo check \
    -p autumn-web \
    --no-default-features \
    --features "$combo" \
    2>&1; then
    ok "  combo ok: --features $combo"
  else
    echo "::error::feature combo failed: --no-default-features --features $combo"
    phase_b_failures=$((phase_b_failures + 1))
  fi
  echo ""
done

if [[ "$phase_b_failures" -gt 0 ]]; then
  die "$phase_b_failures curated combo(s) failed to compile. See ::error:: lines above."
fi

ok "Phase B passed — all curated real-world combinations compile."
echo ""

# ── Summary ────────────────────────────────────────────────────────────────────
# Phase A covers: 1 (no-feature build) + number of non-excluded features.
# Phase B covers: fixed curated combos.
# Total is printed for the auditable one-line record required by issue #982.

EXCLUDED_FEATURES=(managed-pg-bundled managed-pg system-tests test-support)
# Count features defined in autumn/Cargo.toml, excluding the ones above.
# We parse the [features] section by looking for "^<name> =" lines.
ALL_FEATURES=$(sed -n '/^\[features\]/,/^\[/p' autumn/Cargo.toml \
  | grep -E '^[a-z][a-z0-9_-]+ *=' \
  | sed 's/ *=.*//' \
  | grep -v '^default$')
EXCLUDED_PATTERN="$(IFS='|'; echo "${EXCLUDED_FEATURES[*]}")"
SWEPT_FEATURES=$(echo "$ALL_FEATURES" | grep -Ev "^($EXCLUDED_PATTERN)$" | wc -l | tr -d ' ')
# +1 for the no-feature build; Phase B adds the curated combos
TOTAL_COMBOS=$(( SWEPT_FEATURES + 1 + ${#CURATED_COMBOS[@]} ))

echo "Gated $TOTAL_COMBOS feature combinations (each-feature sweep: $((SWEPT_FEATURES + 1)) builds; curated real-world combos: ${#CURATED_COMBOS[@]})."
