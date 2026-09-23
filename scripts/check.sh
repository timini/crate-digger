#!/usr/bin/env bash
# Run every check CI runs, stopping at the first failure.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --quiet
(cd app && pnpm check && pnpm test)
echo "all checks passed"
