#!/usr/bin/env bash
set -euo pipefail
if ! command -v cargo-llvm-cov &> /dev/null; then
    cargo install cargo-llvm-cov
fi
cargo llvm-cov --workspace --html --output-dir target/coverage
echo "Coverage report: target/coverage/html/index.html"
echo ""
cargo llvm-cov --workspace --summary-only
