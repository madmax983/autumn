#!/usr/bin/env bash
# Build-and-boot gate for the generated `autumn release init` image (issue #978).
#
# `autumn release init` scaffolds a production Dockerfile and docs/guide/deployment.md
# makes a falsifiable promise: "from a fresh `autumn new` project to a
# production-shaped container running... under 10 minutes", with `/health` wired
# as the container HEALTHCHECK. The existing tests only string-assert Dockerfile
# *contents*; nothing ever runs `docker build` on the generated image and boots
# it. This harness closes that loop: it scaffolds a fresh project, runs
# `autumn release init --force`, builds the generated image, runs the documented
# one-shot `autumn migrate` job against a throwaway Postgres, boots the web
# container, and asserts GET /health and /actuator/health both return 200 within
# a bounded startup window. It also covers the `--target docker-compose` path,
# bringing the stack up and tearing it down cleanly.
#
# Usage:
#   scripts/check-release-image-boot.sh [default|docker-compose]
#
# Environment:
#   AUTUMN_BIN             Path to a prebuilt `autumn` binary. When unset, the
#                         script builds `autumn-cli` from the current checkout so
#                         the gate verifies the scaffold produced by *this* tree.
#   PG_HOST / PG_PORT     Postgres host/port for the bare `default` target's
#                         one-shot migrate + boot (default: localhost / 5432).
#                         In CI this is a service container mapped to localhost.
#   PG_USER / PG_PASSWORD Postgres credentials (default: autumn / autumn).
#   STARTUP_BUDGET_SECS   Health-probe deadline after boot (default: 30) — the
#                         documented "≤ 30s after boot" window.
#   IMAGE_SIZE_BUDGET_MB  Runtime image size budget for the secondary guard
#                         (default: 150). Reported informationally; exceeding it
#                         warns rather than fails (optional per the spec).
set -euo pipefail

TARGET="${1:-default}"
STARTUP_BUDGET_SECS="${STARTUP_BUDGET_SECS:-30}"
IMAGE_SIZE_BUDGET_MB="${IMAGE_SIZE_BUDGET_MB:-150}"
PG_HOST="${PG_HOST:-localhost}"
PG_PORT="${PG_PORT:-5432}"
PG_USER="${PG_USER:-autumn}"
PG_PASSWORD="${PG_PASSWORD:-autumn}"

PROJECT_NAME="releasecheck"
IMAGE_TAG="autumn-release-image-boot:ci"
CONTAINER_NAME="autumn-release-image-boot"

# ── repo + workspace setup ──────────────────────────────────────────────────
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

log()  { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }
warn() { printf '\033[1;33m[warn]\033[0m %s\n' "$*"; }
fail() { printf '\033[1;31m[fail]\033[0m %s\n' "$*" >&2; }

# Resolve the `autumn` binary. Build from the current checkout when AUTUMN_BIN is
# unset so the gate exercises the scaffold emitted by the code under test.
if [[ -n "${AUTUMN_BIN:-}" ]]; then
  AUTUMN="${AUTUMN_BIN}"
else
  log "Building autumn-cli from the current checkout"
  cargo build -p autumn-cli --bin autumn --manifest-path "${REPO_ROOT}/Cargo.toml"
  AUTUMN="${REPO_ROOT}/target/debug/autumn"
fi
log "Using autumn binary: ${AUTUMN}"
"${AUTUMN}" --version || true

WORKDIR="$(mktemp -d)"
PROJECT_DIR="${WORKDIR}/${PROJECT_NAME}"

# Probe response captured by the most recent failed health check, surfaced in
# the failure summary so the breakage is diagnosable from the CI log alone.
LAST_PROBE_RESPONSE=""

DB_URL="postgres://${PG_USER}:${PG_PASSWORD}@${PG_HOST}:${PG_PORT}/${PROJECT_NAME}_prod"
SIGNING_SECRET="$(openssl rand -hex 32 2>/dev/null || echo "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")"

# ── cleanup ─────────────────────────────────────────────────────────────────
cleanup() {
  set +e
  if [[ "${TARGET}" == "docker-compose" && -d "${PROJECT_DIR}" ]]; then
    log "Tearing down docker-compose stack"
    ( cd "${PROJECT_DIR}" && docker compose down -v --remove-orphans >/dev/null 2>&1 )
  fi
  # Only remove the named container in the default target; the docker-compose
  # target never starts it, and removing it unconditionally would kill a
  # sibling job's container on a shared runner.
  if [[ "${TARGET}" == "default" ]]; then
    docker rm -f "${CONTAINER_NAME}" >/dev/null 2>&1 || true
  fi
  rm -rf "${WORKDIR}"
}
trap cleanup EXIT

# ── scaffold ────────────────────────────────────────────────────────────────
log "Scaffolding a fresh project with \`autumn new\`"
( cd "${WORKDIR}" && "${AUTUMN}" new "${PROJECT_NAME}" )

# ── health probe helper ─────────────────────────────────────────────────────
# Polls each URL until it returns HTTP 200 or the per-URL budget elapses.
# Each URL gets a fresh budget window so a slow-starting first endpoint cannot
# starve subsequent ones. A single curl invocation per tick captures body and
# status atomically (no TOCTOU). The body file is truncated before each curl
# so a failed request (no HTTP response written) never exposes stale bytes.
#
# Usage: probe_until_healthy <budget_secs> <url> [<url> ...]
#   budget_secs  — seconds each URL is given to reach 200
#   url(s)       — one or more endpoints; all must return 200
probe_until_healthy() {
  local budget_secs="$1"
  shift
  local urls=("$@")
  local probe_body_file
  probe_body_file="$(mktemp "${WORKDIR}/probe_body.XXXXXX")"

  for url in "${urls[@]}"; do
    local code=""
    local body=""
    LAST_PROBE_RESPONSE=""
    local url_deadline=$(( SECONDS + budget_secs ))
    while (( SECONDS < url_deadline )); do
      : > "${probe_body_file}"
      code="$(curl -o "${probe_body_file}" -s -m 5 -w '%{http_code}' "${url}" 2>/dev/null || echo 000)"
      body="$(cat "${probe_body_file}" 2>/dev/null || true)"
      if [[ "${code}" == "200" ]]; then
        log "HEALTHY: ${url} -> 200 (${body})"
        break
      fi
      LAST_PROBE_RESPONSE="${code} ${body}"
      sleep 1
    done
    if [[ "${code}" != "200" ]]; then
      rm -f "${probe_body_file}"
      fail "${url} did not return 200 within ${budget_secs}s (last code: ${code:-none}, body: ${body:-<empty>})"
      return 1
    fi
  done

  rm -f "${probe_body_file}"
  return 0
}

# Report the runtime image size against the secondary budget (informational).
report_image_size() {
  local image="$1"
  local bytes mb
  bytes="$(docker image inspect "${image}" --format '{{.Size}}' 2>/dev/null || echo 0)"
  mb=$(( bytes / 1024 / 1024 ))
  log "Runtime image size: ${mb} MB (budget: ${IMAGE_SIZE_BUDGET_MB} MB)"
  if (( mb > IMAGE_SIZE_BUDGET_MB )); then
    warn "image size ${mb} MB exceeds the ${IMAGE_SIZE_BUDGET_MB} MB budget — investigate runtime image bloat"
  fi
  if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
    printf '* Runtime image size: **%s MB** (budget %s MB)\n' "${mb}" "${IMAGE_SIZE_BUDGET_MB}" >> "${GITHUB_STEP_SUMMARY}"
  fi
}

# ── default (bare release init) target ──────────────────────────────────────
run_default_target() {
  log "release init (bare/default target)"
  ( cd "${PROJECT_DIR}" && "${AUTUMN}" release init --force )

  log "docker build the generated image"
  if ! ( cd "${PROJECT_DIR}" && docker build -t "${IMAGE_TAG}" . 2>&1 | tee "${WORKDIR}/build.log" ); then
    fail "docker build failed — see build log above"
    exit 1
  fi

  report_image_size "${IMAGE_TAG}"

  # AC: exercise the documented one-shot migrate path against the primary
  # *before* the web container is marked ready (deployment.md Step 4).
  log "one-shot migrate against the primary (\`autumn migrate\`)"
  if ! docker run --rm --network host \
        -e AUTUMN_DATABASE__PRIMARY_URL="${DB_URL}" \
        "${IMAGE_TAG}" autumn migrate 2>&1 | tee "${WORKDIR}/migrate.log"; then
    fail "one-shot \`autumn migrate\` failed — the rollout must stop here"
    exit 1
  fi

  log "boot the web container"
  # Remove any leftover container with the same name before starting a new one.
  docker rm -f "${CONTAINER_NAME}" >/dev/null 2>&1 || true
  # --network host lets the container reach the Postgres service on localhost
  # and bind :3000 on the runner. Minimal AUTUMN_* env: the primary URL, the
  # required production signing secret, and a trusted-host allowlist so the
  # prod profile binds and /health is reachable.
  docker run -d --name "${CONTAINER_NAME}" --network host \
    -e AUTUMN_DATABASE__PRIMARY_URL="${DB_URL}" \
    -e AUTUMN_SECURITY__SIGNING_SECRET="${SIGNING_SECRET}" \
    -e AUTUMN_SECURITY__TRUSTED_HOSTS__HOSTS="*" \
    "${IMAGE_TAG}"

  if ! probe_until_healthy "${STARTUP_BUDGET_SECS}" \
       "http://localhost:3000/health" \
       "http://localhost:3000/actuator/health"; then
    fail "container did not reach a healthy state — boot logs follow"
    docker logs "${CONTAINER_NAME}" || true
    printf '\n--- failing probe response ---\n%s\n' "${LAST_PROBE_RESPONSE}" >&2
    exit 1
  fi

  log "default target: image builds and boots, /health + /actuator/health = 200"
}

# ── docker-compose target ───────────────────────────────────────────────────
run_compose_target() {
  log "release init --target docker-compose"
  ( cd "${PROJECT_DIR}" && "${AUTUMN}" release init --force --target docker-compose )

  # The generated compose app runs in the prod profile, which requires a
  # non-empty trusted-host allowlist to bind. Inject it (and a signing secret)
  # via a smoke-only override file so the *generated* compose file stays
  # untouched — the artifact under test is unchanged.
  cat > "${PROJECT_DIR}/docker-compose.override.yml" <<'YAML'
services:
  app:
    environment:
      AUTUMN_SECURITY__TRUSTED_HOSTS__HOSTS: "*"
YAML

  export AUTUMN_SECURITY__SIGNING_SECRET="${SIGNING_SECRET}"

  log "docker compose up --build (app + one-shot migrate + Postgres)"
  if ! ( cd "${PROJECT_DIR}" && docker compose up --build -d 2>&1 | tee "${WORKDIR}/compose-build.log" ); then
    fail "docker compose up failed — see build log above"
    ( cd "${PROJECT_DIR}" && docker compose logs ) || true
    exit 1
  fi

  if ! probe_until_healthy "${STARTUP_BUDGET_SECS}" \
       "http://localhost:3000/health" \
       "http://localhost:3000/actuator/health"; then
    fail "compose stack did not reach a healthy state — compose logs follow"
    ( cd "${PROJECT_DIR}" && docker compose logs ) || true
    printf '\n--- failing probe response ---\n%s\n' "${LAST_PROBE_RESPONSE}" >&2
    exit 1
  fi

  log "compose target: stack builds, migrates, and serves /health + /actuator/health = 200"
  # Teardown is handled by the EXIT trap (docker compose down -v).
}

case "${TARGET}" in
  default)        run_default_target ;;
  docker-compose) run_compose_target ;;
  *)
    fail "unknown target '${TARGET}'; expected 'default' or 'docker-compose'"
    exit 2
    ;;
esac

log "release-image-boot gate passed for target '${TARGET}'"
