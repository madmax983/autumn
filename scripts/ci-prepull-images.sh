#!/usr/bin/env bash
# Pre-pull container images once, serially, with retries — for CI jobs that
# spin up many testcontainers (issue #3112).
#
# Why this exists: nothing in CI pre-pulled the images, so on a runner whose
# cache had been pruned, every one of the 100+ `Postgres::default()` call sites
# (`postgres:11-alpine`) pulled the same image in parallel at the same moment,
# and one of those parallel pulls could die mid-stream
# (`PullImage { ... "bytes remaining on stream" }`), failing a random unrelated
# test before its body ran. A serial pre-pull with retries moves the flake into
# one step that retries and fails loudly with the image named, instead of
# scattering it across test bodies.
#
# Usage: scripts/ci-prepull-images.sh <image>...   (e.g. postgres:11-alpine)
#
# Exit codes: 0 when every image pulled; 1 when an image still fails after all
# attempts (names the image); 2 on usage errors (no images given).
#
# Test hook: CI_PREPULL_RETRY_DELAYS overrides the growing inter-attempt delays
# (seconds, space-separated); e.g. CI_PREPULL_RETRY_DELAYS="0 0 0" for tests.

set -euo pipefail

# Pull attempts per image: 1 initial + 3 retries.
MAX_ATTEMPTS=4
# Growing delay (seconds) between attempts: 15s before retry 2, 30s before
# retry 3, 60s before retry 4.
RETRY_DELAYS_STR="${CI_PREPULL_RETRY_DELAYS:-15 30 60}"

if [ "$#" -eq 0 ]; then
    echo "usage: $0 <image>..." >&2
    exit 2
fi

failed=0
for image in "$@"; do
    attempt=1
    while true; do
        echo "ci-prepull-images: pulling ${image} (attempt ${attempt}/${MAX_ATTEMPTS})"
        if docker pull "${image}"; then
            echo "ci-prepull-images: ${image} ready"
            break
        fi
        if [ "${attempt}" -ge "${MAX_ATTEMPTS}" ]; then
            echo "ci-prepull-images: ERROR: failed to pull ${image} after ${MAX_ATTEMPTS} attempts" >&2
            failed=1
            break
        fi
        delay="$(echo "${RETRY_DELAYS_STR}" | cut -d' ' -f"${attempt}")"
        echo "ci-prepull-images: pull of ${image} failed; retrying in ${delay}s (attempt $((attempt + 1))/${MAX_ATTEMPTS})"
        sleep "${delay}"
        attempt=$((attempt + 1))
    done
done

if [ "${failed}" -ne 0 ]; then
    echo "ci-prepull-images: ERROR: one or more images failed to pull" >&2
    exit 1
fi
