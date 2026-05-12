#!/usr/bin/env bash
# Benchmark harness structure gate.
#
# Verifies that the framework runtime benchmark suite under
# benchmarks/runtime/ is structurally complete:
#
#   - Shared infrastructure files (schema, seed, docker-compose) exist.
#   - Every expected framework directory is present with a Dockerfile.
#   - Load-test scripts are checked in and the runner script is executable.
#   - README.md exists with required methodology sections.
#   - Each framework app has migrations and a documented version file.
#
# Exit status 0 = all checks passed.
# Exit status 1 = one or more failures found.
#
# Run locally:
#   ./scripts/check-benchmarks.sh

set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

BENCH="benchmarks/runtime"
FRAMEWORKS=(autumn spring-boot rails django phoenix loco)
REQUIRED_K6_SCRIPTS=(json-crud.js html-page.js validation-fail.js auth-protected.js)
REQUIRED_README_SECTIONS=(
  "## Methodology"
  "## Framework Versions"
  "## Infrastructure"
  "## Running the Benchmark"
  "## Metrics"
  "## Tracks"
)

failures=0

ok()   { echo "ok:    $*"; }
fail() { echo "FAIL:  $*" >&2; failures=$((failures + 1)); }
warn() { echo "warn:  $*"; }

# ---------------------------------------------------------------------------
# 1. Top-level benchmark directory
# ---------------------------------------------------------------------------
echo "==> Checking top-level benchmark directory: $BENCH"
if [[ -d "$BENCH" ]]; then
  ok "$BENCH exists"
else
  fail "$BENCH directory not found — create it to begin"
fi
echo ""

# ---------------------------------------------------------------------------
# 2. Shared infrastructure files
# ---------------------------------------------------------------------------
echo "==> Checking shared infrastructure files"

check_file() {
  local path="$1"
  if [[ -f "$root/$path" ]]; then
    ok "$path"
  else
    fail "$path not found"
  fi
}

check_file "$BENCH/docker-compose.yml"
check_file "$BENCH/schema/init.sql"
check_file "$BENCH/seed/seed.sql"
echo ""

# ---------------------------------------------------------------------------
# 3. Framework directories: Dockerfile + migrations + version file
# ---------------------------------------------------------------------------
echo "==> Checking framework directories"

for fw in "${FRAMEWORKS[@]}"; do
  dir="$BENCH/$fw"
  if [[ ! -d "$root/$dir" ]]; then
    fail "$dir/ directory not found"
    continue
  fi
  ok "$dir/ exists"

  if [[ -f "$root/$dir/Dockerfile" ]]; then
    ok "$dir/Dockerfile exists"
  else
    fail "$dir/Dockerfile not found"
  fi

  # Migrations: accept any conventional migrations path across the supported frameworks.
  # Spring Boot/Flyway: src/main/resources/db/migration/
  # Django: <app>/migrations/
  # Rails: db/migrate/
  # Phoenix: priv/repo/migrations/
  # Rust (Autumn/Loco): migrations/
  if [[ -d "$root/$dir/migrations" \
     || -d "$root/$dir/db/migrate" \
     || -d "$root/$dir/priv/repo/migrations" \
     || -d "$root/$dir/src/main/resources/db/migration" \
     || $(find "$root/$dir" -maxdepth 3 -type d -name "migrations" 2>/dev/null | wc -l) -gt 0 ]]; then
    ok "$dir has a migrations directory"
  else
    fail "$dir has no migrations directory (checked migrations/, db/migrate/, priv/repo/migrations/, src/main/resources/db/migration/, or any nested migrations/)"
  fi

  # Version marker: a VERSIONS file or .tool-versions or mix.exs or Cargo.toml or pom.xml etc.
  has_version=false
  for vf in VERSIONS .tool-versions mix.exs Cargo.toml pom.xml Gemfile build.gradle; do
    if [[ -f "$root/$dir/$vf" ]]; then
      has_version=true
      ok "$dir/$vf found (version marker)"
      break
    fi
  done
  if [[ "$has_version" == "false" ]]; then
    fail "$dir has no version marker file (VERSIONS, .tool-versions, Cargo.toml, pom.xml, etc.)"
  fi
done
echo ""

# ---------------------------------------------------------------------------
# 4. Load-test scripts
# ---------------------------------------------------------------------------
echo "==> Checking load-test scripts under $BENCH/load/k6/"

for script in "${REQUIRED_K6_SCRIPTS[@]}"; do
  check_file "$BENCH/load/k6/$script"
done

runner="$BENCH/load/run.sh"
check_file "$runner"

if [[ -f "$root/$runner" && -x "$root/$runner" ]]; then
  ok "$runner is executable"
else
  fail "$runner is not executable (run: chmod +x $runner)"
fi
echo ""

# ---------------------------------------------------------------------------
# 5. Load script parameterization: each k6 script must reference BASE_URL
# ---------------------------------------------------------------------------
echo "==> Checking k6 scripts are parameterized (reference BASE_URL)"

for script in "${REQUIRED_K6_SCRIPTS[@]}"; do
  path="$root/$BENCH/load/k6/$script"
  if [[ ! -f "$path" ]]; then
    continue  # already flagged above
  fi
  if grep -q "BASE_URL" "$path"; then
    ok "load/k6/$script references BASE_URL"
  else
    fail "load/k6/$script does not reference BASE_URL — scripts must be parameterized"
  fi
done
echo ""

# ---------------------------------------------------------------------------
# 6. README.md with required sections
# ---------------------------------------------------------------------------
echo "==> Checking $BENCH/README.md for required sections"

readme="$root/$BENCH/README.md"
if [[ ! -f "$readme" ]]; then
  fail "$BENCH/README.md not found"
else
  ok "$BENCH/README.md exists"
  for section in "${REQUIRED_README_SECTIONS[@]}"; do
    if grep -qi "$section" "$readme"; then
      ok "  section '$section' present"
    else
      fail "  README.md missing section '$section'"
    fi
  done
fi
echo ""

# ---------------------------------------------------------------------------
# 7. Schema equivalence hint: init.sql and autumn migration must reference
#    the same benchmark table name (posts)
# ---------------------------------------------------------------------------
echo "==> Checking schema uses canonical 'posts' table"

init_sql="$root/$BENCH/schema/init.sql"
if [[ -f "$init_sql" ]]; then
  if grep -qiE "CREATE TABLE (IF NOT EXISTS )?posts" "$init_sql"; then
    ok "schema/init.sql defines 'posts' table"
  else
    fail "schema/init.sql must define a 'posts' table (canonical benchmark resource)"
  fi
fi

autumn_migration_dir="$root/$BENCH/autumn/migrations"
if [[ -d "$autumn_migration_dir" ]]; then
  if grep -rqiE "CREATE TABLE (IF NOT EXISTS )?posts" "$autumn_migration_dir"; then
    ok "autumn/migrations defines 'posts' table"
  else
    fail "autumn/migrations must define a 'posts' table matching schema/init.sql"
  fi
fi
echo ""

# ---------------------------------------------------------------------------
# 8. Seed data sanity: seed.sql must INSERT into posts
# ---------------------------------------------------------------------------
echo "==> Checking seed data references 'posts'"

seed_sql="$root/$BENCH/seed/seed.sql"
if [[ -f "$seed_sql" ]]; then
  if grep -qi "INSERT INTO posts" "$seed_sql"; then
    ok "seed/seed.sql INSERTs into 'posts'"
  else
    fail "seed/seed.sql must INSERT into 'posts' table"
  fi
fi
echo ""

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo "========================================"
if [[ "$failures" -gt 0 ]]; then
  echo "Benchmark gate: $failures failure(s) found." >&2
  exit 1
fi
echo "Benchmark gate: all checks passed."
