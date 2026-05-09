#!/usr/bin/env bash
# Build public documentation for every publishable crate in the feature posture
# that docs.rs will use, and fail on any rustdoc warning or broken intra-doc link.
#
# docs.rs builds each crate in isolation (not as a workspace) with the feature
# set declared in [package.metadata.docs.rs].  We approximate that here by
# building the full workspace with --all-features and -D warnings so that any
# docs.rs-visible problem surfaces in CI before the crate reaches the registry.
#
# Called from the `publish-gate` workflow. Run locally with:
#
#     ./scripts/check-docs.sh

set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

# Turn all rustdoc warnings into errors and enable the broken-intra-doc-links lint.
export RUSTDOCFLAGS="-D warnings -D rustdoc::broken_intra_doc_links -D rustdoc::private_intra_doc_links"

echo "Building workspace documentation (all features, no deps)..."
echo "RUSTDOCFLAGS=$RUSTDOCFLAGS"
echo ""

# --no-deps keeps build time short; docs.rs also builds only the target crate.
cargo doc --workspace --all-features --no-deps 2>&1

echo ""
echo "Documentation build OK — no rustdoc warnings or broken intra-doc links."
