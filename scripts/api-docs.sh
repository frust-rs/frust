#!/usr/bin/env bash
# scripts/api-docs.sh — generate self-hostable rustdoc tree for website API reference.
#
# Builds rustdoc for the root workspace (crates/*) and generates a
# target/doc/index.html redirect + crate index for nginx's /api/ location
# redirect and fallback navigation. Standalone workspaces (examples/huddle,
# examples/shadertoy, examples/glyph-catalog, examples/playground,
# examples/design-system-sample, examples/material3-demo) are out of scope;
# they are documented from their own directories if needed.
#
# The script respects the root workspace's existing rustdoc warnings (see
# docs/DEVELOPMENT.md for action-item ledger), so `cargo doc` never fails on
# warnings. Crates that cannot document on the host platform are auto-excluded
# with a comment naming the failure.
#
# Usage: scripts/api-docs.sh [--check]
#   --check   Verify target/doc/frust/index.html exists and exit non-zero
#             otherwise. Used as the API-reference build gate before publishing.
#
# Environment variables:
#   OUT       Optional output directory (default: target/doc). When set,
#             copies the generated documentation tree to $OUT via rsync
#             (or cp -r if rsync is unavailable).
#
# Outputs: prints the documentation root path and crate count.
# Exits: 0 on success (documentation generated and any copies completed);
#        1 on failure (build failed or --check found missing index).

set -euo pipefail

# --- Derive repo root from script location ------------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." >/dev/null 2>&1 && pwd)"

# --- Arg parsing and config --------------------------------------------------

CHECK_MODE=0
OUT_DIR="${OUT:-}"

while [ $# -gt 0 ]; do
  case "$1" in
    --check)
      CHECK_MODE=1
      shift
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

# --- Build rustdoc for root workspace ----------------------------------------

echo "Building rustdoc for root workspace..." >&2

cd "$REPO_ROOT"

# Run cargo doc --workspace --no-deps --lib. Do not add -D warnings: the repo
# carries known rustdoc warnings (see docs/DEVELOPMENT.md action-item ledger).
# Crates that require platform-specific compilation will be caught here; if
# any fail to document on the build host, they will be excluded with a comment
# below. For this first run, try the full workspace. Use --target-dir to
# override any global cargo config that might redirect to a shared cache.
if ! cargo doc --target-dir "$REPO_ROOT/target" --workspace --no-deps --lib 2>&1; then
  echo "warning: cargo doc failed; checking which crates to exclude..." >&2
  
  # Re-run with --exclude for known platform-specific crates that may not build
  # on Linux. frust-shell-ios, frust-shell-macos, and frust-shell-windows are
  # known to fail on Linux due to platform SDK dependencies.
  cargo doc --target-dir "$REPO_ROOT/target" --workspace --no-deps --lib \
    --exclude frust-shell-ios \
    --exclude frust-shell-macos \
    --exclude frust-shell-windows \
    2>&1 || {
      echo "error: cargo doc failed even after excluding platform-specific crates" >&2
      exit 1
    }
  
  echo "Documented root workspace (excluded frust-shell-ios, frust-shell-macos, frust-shell-windows)." >&2
else
  echo "Documented root workspace (all crates)." >&2
fi

# --- Generate target/doc/index.html redirect ---------------------------------

DOC_ROOT="${REPO_ROOT}/target/doc"

# List all documented crates (directories in target/doc/ with index.html).
CRATE_DIRS=()
if [ -d "$DOC_ROOT" ]; then
  while IFS= read -r d; do
    if [ -f "$d/index.html" ]; then
      CRATE_DIRS+=("$(basename "$d")")
    fi
  done < <(find "$DOC_ROOT" -mindepth 1 -maxdepth 1 -type d)
fi

# Count crates (excluding 'src' and other auxiliary directories).
CRATE_COUNT=0
for d in "${CRATE_DIRS[@]}"; do
  if [ "$d" != "src" ]; then
    if [ "$d" != "static.files" ]; then
      CRATE_COUNT=$((CRATE_COUNT + 1))
    fi
  fi
done

# Generate index.html: redirect to frust/index.html + fallback crate list.
cat > "$DOC_ROOT/index.html" << 'HTML'
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Frust API Reference</title>
  <meta http-equiv="refresh" content="0; url=frust/index.html">
  <style>
    body {
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
      margin: 2rem;
      line-height: 1.6;
    }
    h1 { color: #333; }
    a { color: #0066cc; text-decoration: none; }
    a:hover { text-decoration: underline; }
    ul { list-style-type: none; padding: 0; }
    li { margin: 0.5rem 0; }
    code { background: #f5f5f5; padding: 0.2rem 0.4rem; border-radius: 3px; }
  </style>
</head>
<body>
  <h1>Frust API Reference</h1>
  <p>Redirecting to <code>frust</code> root crate documentation...</p>
  <p><a href="frust/index.html">Click here if not redirected.</a></p>
  
  <h2>All Crates</h2>
  <ul>
HTML

# Append crate links (sorted, excluding auxiliary dirs).
for d in $(printf '%s\n' "${CRATE_DIRS[@]}" | grep -v '^src$' | grep -v '^static\.files$' | sort); do
  echo "    <li><a href=\"$d/index.html\">$d</a></li>" >> "$DOC_ROOT/index.html"
done

cat >> "$DOC_ROOT/index.html" << 'HTML'
  </ul>
</body>
</html>
HTML

# --- Copy to OUT if specified ------------------------------------------------

if [ -n "$OUT_DIR" ]; then
  echo "Copying documentation to $OUT_DIR..." >&2
  
  if command -v rsync >/dev/null 2>&1; then
    rsync -a --delete "$DOC_ROOT/" "$OUT_DIR/"
  else
    echo "warning: rsync not found; falling back to cp -r" >&2
    rm -rf "$OUT_DIR"
    cp -r "$DOC_ROOT" "$OUT_DIR"
  fi
fi

# --- Handle --check mode -----------------------------------------------------

if [ "$CHECK_MODE" -eq 1 ]; then
  if [ -f "$DOC_ROOT/frust/index.html" ]; then
    echo "✓ target/doc/frust/index.html exists" >&2
    exit 0
  else
    echo "✗ target/doc/frust/index.html not found" >&2
    exit 1
  fi
fi

# --- Print summary and exit --------------------------------------------------

echo "Documentation generated: $DOC_ROOT" >&2
echo "Crate count: $CRATE_COUNT" >&2

exit 0
