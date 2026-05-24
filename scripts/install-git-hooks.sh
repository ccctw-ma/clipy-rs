#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

cd "$PROJECT_DIR"
git config core.hooksPath .githooks
chmod +x .githooks/pre-commit

echo "Git hooks installed: core.hooksPath=.githooks"
echo "Pre-commit checks: cargo fmt --check, cargo clippy -- -D warnings, cargo test"
