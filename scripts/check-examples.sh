#!/usr/bin/env bash
# Example catalog drift gate.
#
# Checks that the examples catalog (EXAMPLES.md) is coherent with:
#   1. Every directory under examples/ is cataloged.
#   2. Every workspace examples/* member is cataloged as a supported example.
#   3. Every example in the README.md Examples table appears in the catalog.
#   4. Each supported example has a README.md with the required quickstart sections.
#
# The catalog format uses machine-readable HTML comment markers:
#
#   <!-- catalog:example name=<dir> tier=supported -->
#   <!-- catalog:example name=<dir> tier=excluded -->
#   <!-- catalog:example name=<dir> tier=experimental -->
#
# Output is grouped by support tier so contributors can immediately see which
# failures block release and which are intentionally manual.
#
# Called from the `publish-gate` workflow. Run locally with:
#
#     ./scripts/check-examples.sh

set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

CATALOG="EXAMPLES.md"
EXAMPLES_DIR="examples"
WORKSPACE_MANIFEST="Cargo.toml"
README="README.md"

# Required sections in every supported example README (case-insensitive grep).
REQUIRED_README_SECTIONS=(
  "## Prerequisites"
  "## Quick"
)

die() {
  echo "error: $*" >&2
  exit 1
}

ok() {
  echo "ok:    $*"
}

warn() {
  echo "warn:  $*" >&2
}

fail() {
  echo "FAIL:  $*" >&2
  failures=$((failures + 1))
}

failures=0

# ---------------------------------------------------------------------------
# 0. Catalog must exist
# ---------------------------------------------------------------------------
echo "==> Checking catalog file exists: $CATALOG"
if [[ ! -f "$CATALOG" ]]; then
  fail "catalog file '$CATALOG' not found — create it to pass this gate"
  echo ""
  echo "Example drift gate: $failures failure(s) found."
  die "$failures failure(s) — see output above. Create EXAMPLES.md to begin."
fi
ok "catalog file found"
echo ""

# ---------------------------------------------------------------------------
# Helper: extract cataloged example names by tier from EXAMPLES.md
# Each marker line has the form:
#   <!-- catalog:example name=<dir> tier=<tier> -->
# ---------------------------------------------------------------------------
catalog_names_by_tier() {
  local tier="$1"
  grep -E "<!-- catalog:example name=[^ ]+ tier=${tier}" "$CATALOG" \
    | grep -oE 'name=[^ >]+' \
    | sed 's/name=//' \
    || true
}

all_catalog_names() {
  grep -E "<!-- catalog:example name=" "$CATALOG" \
    | grep -oE 'name=[^ >]+' \
    | sed 's/name=//' \
    || true
}

# ---------------------------------------------------------------------------
# 1. Every examples/ directory must be cataloged
# ---------------------------------------------------------------------------
echo "==> Checking every examples/ directory is cataloged"

mapfile -t example_dirs < <(find "$EXAMPLES_DIR" -mindepth 1 -maxdepth 1 -type d -printf '%f\n' | sort)
mapfile -t catalog_all < <(all_catalog_names | sort)

for dir in "${example_dirs[@]}"; do
  if printf '%s\n' "${catalog_all[@]}" | grep -qx "$dir"; then
    ok "  examples/$dir is cataloged"
  else
    fail "  examples/$dir has no catalog entry in $CATALOG"
  fi
done
echo ""

# ---------------------------------------------------------------------------
# 2. Every workspace examples/* member must be cataloged as supported
# ---------------------------------------------------------------------------
echo "==> Checking workspace examples/* members are cataloged as supported"

mapfile -t workspace_examples < <(
  grep -E '"examples/' "$WORKSPACE_MANIFEST" \
    | grep -oE 'examples/[^"]+' \
    | sed 's|examples/||' \
    | sort
)
mapfile -t catalog_supported < <(catalog_names_by_tier "supported" | sort)

for member in "${workspace_examples[@]}"; do
  if printf '%s\n' "${catalog_supported[@]}" | grep -qx "$member"; then
    ok "  workspace member examples/$member is cataloged as supported"
  else
    fail "  workspace member examples/$member is NOT cataloged as supported in $CATALOG"
  fi
done
echo ""

# ---------------------------------------------------------------------------
# 3. Every README.md Examples table entry must appear in the catalog
# ---------------------------------------------------------------------------
echo "==> Checking README.md Examples table entries appear in catalog"

# Match markdown table cells containing a link like [`examples/foo`](examples/foo)
mapfile -t readme_examples < <(
  grep -oE '\[`examples/[^`]+`\]' "$README" \
    | grep -oE 'examples/[^`]+' \
    | sed 's|examples/||' \
    | sort
)

for ex in "${readme_examples[@]}"; do
  if printf '%s\n' "${catalog_all[@]}" | grep -qx "$ex"; then
    ok "  README example '$ex' found in catalog"
  else
    fail "  README example '$ex' is NOT in the catalog"
  fi
done
echo ""

# ---------------------------------------------------------------------------
# 3b. Every examples/* path referenced in docs/guide/ must appear in the catalog
# ---------------------------------------------------------------------------
echo "==> Checking docs/guide/ example references appear in catalog"

DOCS_GUIDE="docs/guide"
if [[ -d "$DOCS_GUIDE" ]]; then
  mapfile -t docs_examples < <(
    grep -rhoE 'examples/[a-z_-]+' "$DOCS_GUIDE" \
      | sed 's|examples/||' \
      | sort -u
  )
  for ex in "${docs_examples[@]}"; do
    # Skip references that are just path fragments matching no real directory.
    [[ -d "$EXAMPLES_DIR/$ex" ]] || continue
    if printf '%s\n' "${catalog_all[@]}" | grep -qx "$ex"; then
      ok "  docs/guide/ reference '$ex' found in catalog"
    else
      fail "  docs/guide/ reference 'examples/$ex' is NOT in the catalog"
    fi
  done
else
  warn "docs/guide/ directory not found — skipping guide reference check"
fi
echo ""

# ---------------------------------------------------------------------------
# 4. Every supported example must have a README.md with required sections
# ---------------------------------------------------------------------------
echo "==> Checking supported examples have README.md with required sections"

for ex in "${catalog_supported[@]}"; do
  readme="$EXAMPLES_DIR/$ex/README.md"
  if [[ ! -f "$readme" ]]; then
    fail "  examples/$ex has no README.md"
    continue
  fi

  for section in "${REQUIRED_README_SECTIONS[@]}"; do
    if grep -qi "$section" "$readme"; then
      ok "  examples/$ex: section '$section' present"
    else
      fail "  examples/$ex: README.md missing section '$section'"
    fi
  done
done
echo ""

# ---------------------------------------------------------------------------
# 5. Summary grouped by tier
#
# Failures in SUPPORTED examples block release.
# EXPERIMENTAL and EXCLUDED examples are informational only; they do not
# participate in normal workspace validation.
# ---------------------------------------------------------------------------
echo "==> Catalog summary by support tier"

_count_supported=0
echo ""
echo "  SUPPORTED  (failures here block release):"
for ex in $(catalog_names_by_tier "supported" | sort); do
  echo "    examples/$ex"
  _count_supported=$((_count_supported + 1))
done
echo "  ($( [[ $_count_supported -eq 0 ]] && echo "none" || echo "$_count_supported example(s)" ))"

echo ""
echo "  EXPERIMENTAL  (informational; not release-blocking):"
exp_list="$(catalog_names_by_tier "experimental" | sort)"
if [[ -z "$exp_list" ]]; then
  echo "    (none)"
else
  for ex in $exp_list; do
    echo "    examples/$ex"
  done
fi

echo ""
echo "  EXCLUDED  (intentionally out of adoption path):"
excl_list="$(catalog_names_by_tier "excluded" | sort)"
if [[ -z "$excl_list" ]]; then
  echo "    (none)"
else
  for ex in $excl_list; do
    echo "    examples/$ex"
  done
fi

echo ""
if [[ "$failures" -gt 0 ]]; then
  echo "Tip: run 'cat EXAMPLES.md' to see the catalog and 'cat scripts/check-examples.sh'" \
       "for the marker format." >&2
  die "$failures failure(s) found — fix catalog, README.md table, or workspace membership before publishing."
fi

echo "Example catalog drift gate: all checks passed."
