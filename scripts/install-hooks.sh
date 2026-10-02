#!/usr/bin/env bash
# Enable the tracked git hooks (.githooks/commit-msg, .githooks/pre-push) for this clone.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
git config core.hooksPath .githooks
echo "hooks enabled: core.hooksPath = .githooks"
