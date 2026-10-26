#!/usr/bin/env bash
# Browser-tier boot smoke. Builds examples/web-gallery for wasm32, stages the
# same dist `frust build web` stages, serves it on 127.0.0.1 and boots it in
# headless Chrome, then asserts on the console log. `cargo check` cannot see a
# wasm32 runtime panic (std::time::Instant::now() panics there); this can.
#
# Usage: scripts/ci/web-smoke.sh [--dist <dir>] [--no-browser] [--self-test]
#   (no flags)    build, stage into a mktemp dir, serve, boot both arms, assert
#   --dist <dir>  reuse a prebuilt tree (skips the build), e.g. the website's
#                 staged static/preview/live/
#   --no-browser  stop after staging (hosts without Chrome)
#   --self-test   run the assertion function over fixture logs, then exit
#
# Stages: BUILD (cargo release for wasm32 in examples/web-gallery, honouring
# CARGO_TARGET_DIR; wasm-bindgen --target web, which must be exactly 0.2.128,
# the workspace pin, see docs/DEVELOPMENT.md Version-Pin Policy; copy the
# framework host page index.html + frust_web.js from
# crates/frust-shell-web/platform/web), SERVE (python3 http.server, loopback
# only, killed on exit), BOOT (two URLs: default arm and arm=webgl) and ASSERT.
#
# ASSERT, per URL: FAIL on `panicked at`, `RuntimeError`, `[frust-web] FAIL` or
# an ERROR-level `[frust-web]` console line; REQUIRE
# `[frust-web] wasm module initialized`. With FRUST_WEB_SMOKE_REQUIRE_SURFACE=1
# also REQUIRE `frust: render surface online` and FAIL on
# `surface bring-up attempt 8/8 failed`; without it a `no compatible GPU
# adapter` failure is only a WARN (rigs with no SwiftShader adapter).
#
# Environment: CHROME (binary; default first of google-chrome,
# google-chrome-stable, chromium, chromium-browser), FRUST_WEB_SMOKE_BUDGET_MS
# (virtual time budget, default 20000), FRUST_WEB_SMOKE_REQUIRE_SURFACE.
# Proves boot + first surface, not pixels; the frust-testing webgl golden gate
# stays the pixel-level gate. Exit: 0 pass, 1 assertion failure, 2 usage or
# toolchain error.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PIN="0.2.128"
QUERY_BASE="case=button&theme=light&embed=1&log=info"
BUDGET="${FRUST_WEB_SMOKE_BUDGET_MS:-20000}"

DIST=""
NO_BROWSER=0
SELF_TEST=0
TMP_DIRS=()
SERVER_PID=""

die() { echo "web-smoke: $*" >&2; exit 2; }

cleanup() {
  if [[ -n "$SERVER_PID" ]]; then kill "$SERVER_PID" 2>/dev/null || true; fi
  local d
  for d in ${TMP_DIRS[@]+"${TMP_DIRS[@]}"}; do rm -rf "$d"; done
}
trap cleanup EXIT

# check_log <logfile> <strict 0|1> <label>: prints findings, returns 0 on pass.
check_log() {
  local log="$1" strict="$2" label="$3" fail=0
  local pat
  for pat in 'panicked at' 'RuntimeError' '[frust-web] FAIL'; do
    if grep -qF -- "$pat" "$log"; then
      echo "FAIL [$label]: log contains '$pat'"
      grep -nF -- "$pat" "$log" | head -5 | sed 's/^/    /'
      fail=1
    fi
  done
  if grep -E ':ERROR:CONSOLE.*\[frust-web\]' "$log" >/dev/null; then
    echo "FAIL [$label]: console.error-level [frust-web] line"
    grep -nE ':ERROR:CONSOLE.*\[frust-web\]' "$log" | head -5 | sed 's/^/    /'
    fail=1
  fi
  if ! grep -qF '[frust-web] wasm module initialized' "$log"; then
    echo "FAIL [$label]: missing '[frust-web] wasm module initialized'"
    fail=1
  fi
  if [[ "$strict" == 1 ]]; then
    if ! grep -qF 'frust: render surface online' "$log"; then
      echo "FAIL [$label]: missing 'frust: render surface online' (strict)"
      fail=1
    fi
    if grep -qF 'surface bring-up attempt 8/8 failed' "$log"; then
      echo "FAIL [$label]: 'surface bring-up attempt 8/8 failed' (strict)"
      fail=1
    fi
  elif grep -qF 'no compatible GPU adapter' "$log"; then
    echo "WARN [$label]: no compatible GPU adapter (lenient mode; surface not required)"
  fi
  if [[ "$fail" == 0 ]]; then echo "PASS [$label]"; fi
  return "$fail"
}

self_test() {
  local d rc=0
  d="$(mktemp -d)"; TMP_DIRS+=("$d")
  local pre='[1:1:1026/000000.000:INFO:CONSOLE(1)] "'
  {
    echo "${pre}panicked at /rustc/abc/library/std/src/sys/time/unsupported.rs:13:9:"
    echo "time not implemented on this platform"
    echo "stack:"
    echo "${pre}[frust-web] RuntimeError: unreachable\", source: http://127.0.0.1/frust_web.js (43)"
  } >"$d/panic.log"
  {
    echo "${pre}[frust-web] wasm module initialized\", source: http://127.0.0.1/frust_web.js (268)"
    echo "${pre}frust: render surface online at 400x300 after 1 attempt(s)\", source: x (1)"
  } >"$d/clean.log"
  {
    echo "${pre}[frust-web] wasm module initialized\", source: http://127.0.0.1/frust_web.js (268)"
    echo "${pre}frust: surface bring-up attempt 1/8 failed: frust-render: failed to create render surface: frust-gpu: no compatible GPU adapter: none\", source: x (1)"
  } >"$d/noadapter.log"

  expect() { # expect <want-rc> <file> <strict> <label>
    local got=0
    check_log "$d/$2" "$3" "$4" || got=$?
    if [[ "$got" != "$1" ]]; then echo "SELF-TEST MISMATCH: $4 wanted rc=$1 got $got"; rc=1; fi
  }
  expect 1 panic.log 0 "panic, lenient (rejected)"
  expect 1 panic.log 1 "panic, strict (rejected)"
  expect 0 clean.log 0 "clean, lenient (accepted)"
  expect 0 clean.log 1 "clean, strict (accepted)"
  expect 0 noadapter.log 0 "no-adapter, lenient (accepted)"
  expect 1 noadapter.log 1 "no-adapter, strict (rejected)"
  if [[ "$rc" == 0 ]]; then echo "self-test OK"; else echo "self-test FAILED"; fi
  return "$rc"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dist) [[ $# -ge 2 ]] || die "--dist needs a directory"; DIST="$2"; shift 2 ;;
    --no-browser) NO_BROWSER=1; shift ;;
    --self-test) SELF_TEST=1; shift ;;
    -h|--help) sed -n '2,36p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) die "unknown argument '$1' (see --help)" ;;
  esac
done

if [[ "$SELF_TEST" == 1 ]]; then
  self_test && exit 0 || exit 1
fi

build_and_stage() {
  command -v cargo >/dev/null || die "cargo not found"
  local ver
  ver="$(wasm-bindgen --version 2>/dev/null | awk '{print $2}')" || true
  [[ -n "$ver" ]] || die "wasm-bindgen not found; install wasm-bindgen-cli $PIN (docs/DEVELOPMENT.md Version-Pin Policy)"
  [[ "$ver" == "$PIN" ]] || die "wasm-bindgen $ver found, but the workspace pins exactly $PIN; install wasm-bindgen-cli $PIN (docs/DEVELOPMENT.md Version-Pin Policy)"
  local target="${CARGO_TARGET_DIR:-$ROOT/examples/web-gallery/target}"
  echo "web-smoke: building examples/web-gallery (wasm32, release)"
  (cd "$ROOT/examples/web-gallery" && CARGO_TARGET_DIR="$target" \
    cargo build --release --target wasm32-unknown-unknown) || die "cargo build failed"
  local wasm="$target/wasm32-unknown-unknown/release/web-gallery.wasm"
  [[ -f "$wasm" ]] || die "missing build artefact $wasm"
  mkdir -p "$DIST"
  wasm-bindgen --target web --out-dir "$DIST/pkg" --out-name app "$wasm" || die "wasm-bindgen failed"
  cp "$ROOT/crates/frust-shell-web/platform/web/index.html" \
     "$ROOT/crates/frust-shell-web/platform/web/frust_web.js" "$DIST/"
}

if [[ -z "$DIST" ]]; then
  DIST="$(mktemp -d)"; TMP_DIRS+=("$DIST")
  build_and_stage
fi
DIST="$(cd "$DIST" && pwd)"
for f in index.html frust_web.js pkg/app.js pkg/app_bg.wasm; do
  [[ -f "$DIST/$f" ]] || die "dist $DIST is missing $f"
done
echo "web-smoke: dist staged at $DIST"

command -v python3 >/dev/null || die "python3 not found"
PORT="$(python3 -c 'import socket
s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')"
python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$DIST" >/dev/null 2>&1 &
SERVER_PID=$!
ready=0
for _ in $(seq 1 50); do
  if python3 -c "import urllib.request,sys; urllib.request.urlopen('http://127.0.0.1:$PORT/index.html', timeout=1)" 2>/dev/null; then
    ready=1; break
  fi
  sleep 0.1
done
[[ "$ready" == 1 ]] || die "static server did not come up on 127.0.0.1:$PORT"
echo "web-smoke: serving on http://127.0.0.1:$PORT/"

if [[ "$NO_BROWSER" == 1 ]]; then
  echo "web-smoke: --no-browser; staged and bound OK"
  exit 0
fi

if [[ -z "${CHROME:-}" ]]; then
  for c in google-chrome google-chrome-stable chromium chromium-browser; do
    if command -v "$c" >/dev/null; then CHROME="$c"; break; fi
  done
fi
[[ -n "${CHROME:-}" ]] || die "no Chrome found (set CHROME, or use --no-browser)"
STRICT="${FRUST_WEB_SMOKE_REQUIRE_SURFACE:-0}"

overall=0
for arm in default webgl; do
  q="$QUERY_BASE"
  if [[ "$arm" == webgl ]]; then q="$q&arm=webgl"; fi
  url="http://127.0.0.1:$PORT/index.html?$q"
  log="$DIST/smoke-$arm.log"
  shot="$DIST/smoke-$arm.png"
  echo "web-smoke: booting [$arm] $url"
  rc=0
  timeout 120 "$CHROME" --headless=new --no-sandbox --disable-dev-shm-usage \
    --use-gl=angle --use-angle=swiftshader-webgl --enable-unsafe-swiftshader \
    --ignore-gpu-blocklist --enable-logging=stderr --v=0 \
    --virtual-time-budget="$BUDGET" --window-size=400,300 \
    --screenshot="$shot" "$url" >/dev/null 2>"$log" || rc=$?
  if [[ "$rc" != 0 ]]; then echo "web-smoke: chrome exited $rc for [$arm] (log still asserted)"; fi
  echo "web-smoke: screenshot [$arm]: $shot"
  echo "web-smoke: log [$arm]: $log"
  check_log "$log" "$STRICT" "$arm" || overall=1
done
exit "$overall"
