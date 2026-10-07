#!/usr/bin/env bash
# Hotpatch feature-on gate. Compiles and tests the hotpatch seam end-to-end,
# which feature-off CI (cargo test --workspace, cargo clippy --workspace)
# cannot validate since the feature is opt-in and never default.
#
# Runs, in order:
#  - cargo test -p frust-hotpatch
#  - cargo clippy -p frust-hotpatch --all-targets -- -D warnings
#  - For each of frust-core, frust-shell-desktop, and frust (frust-ui), a test
#    and clippy run with --features hotpatch
#
# Uses --locked like the main CI; frust-shell-desktop needs the same Linux
# build packages the clippy and test jobs install.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

echo "=== frust-hotpatch tests ==="
cargo test -p frust-hotpatch --locked

echo "=== frust-hotpatch clippy ==="
cargo clippy -p frust-hotpatch --all-targets --locked -- -D warnings

echo "=== frust-core tests (hotpatch feature) ==="
cargo test --manifest-path crates/frust-core/Cargo.toml --features hotpatch --locked

echo "=== frust-core clippy (hotpatch feature) ==="
cargo clippy --manifest-path crates/frust-core/Cargo.toml --features hotpatch --all-targets --locked -- -D warnings

echo "=== frust-shell-desktop tests (hotpatch feature) ==="
cargo test --manifest-path crates/frust-shell-desktop/Cargo.toml --features hotpatch --locked

echo "=== frust-shell-desktop clippy (hotpatch feature) ==="
cargo clippy --manifest-path crates/frust-shell-desktop/Cargo.toml --features hotpatch --all-targets --locked -- -D warnings

echo "=== frust-ui (frust) tests (hotpatch feature) ==="
cargo test --manifest-path crates/frust/Cargo.toml --features hotpatch --locked

echo "=== frust-ui (frust) clippy (hotpatch feature) ==="
cargo clippy --manifest-path crates/frust/Cargo.toml --features hotpatch --all-targets --locked -- -D warnings

echo "=== All hotpatch feature gates passed ==="
