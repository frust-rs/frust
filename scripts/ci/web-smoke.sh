#!/usr/bin/env bash
# Browser-tier boot smoke. Builds examples/web-gallery for wasm32, stages the
# same dist `frust build web` stages, serves it on 127.0.0.1 and boots it in
# headless Chrome, then asserts on the console log. `cargo check` cannot see a
# wasm32 runtime panic (std::time::Instant::now() panics there); this can.
#
# Usage: scripts/ci/web-smoke.sh [--dist <dir>] [--out <dir>] [--no-browser] [--self-test]
#   (no flags)    build, stage into a mktemp dir, serve, boot both arms, assert
#   --dist <dir>  reuse a prebuilt tree (skips the build), e.g. the website's
#                 staged static/preview/live/; the tree is only read, never written
#   --out <dir>   where logs and screenshots go (default: a fresh mktemp dir;
#                 it is always kept, and its path is printed at the end so CI
#                 can upload it). Must not be inside the --dist tree.
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
# ASSERT, per URL: FAIL on `panicked at`, `RuntimeError` or a `[frust-web]`
# console line carrying an Error (what frust_web.js's reportFailure prints:
# `console.error("[frust-web]", err)` -> `[frust-web] TypeError: ...`); REQUIRE
# `[frust-web] wasm module initialized`. With FRUST_WEB_SMOKE_REQUIRE_SURFACE=1
# also REQUIRE `frust: render surface online` and FAIL on
# `surface bring-up attempt 8/8 failed`; without it a `no compatible GPU
# adapter` failure is only a WARN (rigs with no SwiftShader adapter).
#
# ADAPTER LADDER: a headless Chrome exposes a GPU adapter only with the right
# flags, and the right flags differ per Chrome build (GitHub's runner image,
# Debian's chromium, a laptop). Each arm is booted with the flag sets in
# GPU_FLAG_SETS in order until one logs `render surface online`; the winning
# set is printed, every attempt's log is kept, and the last attempt's log is
# the one asserted. FRUST_WEB_SMOKE_GPU_FLAGS="<flags>" pins a single set.
#
# Environment: CHROME (binary; default first of google-chrome,
# google-chrome-stable, chromium, chromium-browser), FRUST_WEB_SMOKE_BUDGET_MS
# (virtual time budget per boot, default 20000), FRUST_WEB_SMOKE_REQUIRE_SURFACE,
# FRUST_WEB_SMOKE_GPU_FLAGS, FRUST_WEB_SMOKE_NO_SANDBOX=1 (pass --no-sandbox;
# implied when CI is set, since hosted runners and containers lack the user
# namespaces Chrome's sandbox needs; local runs keep the sandbox).
# Each boot is wall-clock bounded by `timeout` or `gtimeout` when one is on
# PATH; otherwise Chrome runs unwrapped and --virtual-time-budget still bounds it.
# Proves boot + first surface, not pixels; the frust-testing webgl golden gate
# stays the pixel-level gate. Exit: 0 pass, 1 assertion failure, 2 usage or
# toolchain error.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PIN="0.2.128"
QUERY_BASE="case=button&theme=light&embed=1&log=info"
BUDGET="${FRUST_WEB_SMOKE_BUDGET_MS:-20000}"

DIST=""
OUT=""
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
# Matches are collected with `grep -m5` (stops reading early, no SIGPIPE from
# a downstream head) and the sed/print step is tolerated, so a huge log cannot
# turn a diagnostic pipe into a set -e / pipefail exit 141.
check_log() {
  local log="$1" strict="$2" label="$3" fail=0
  local pat
  for pat in 'panicked at' 'RuntimeError'; do
    if grep -qF -- "$pat" "$log"; then
      echo "FAIL [$label]: log contains '$pat'"
      { grep -m5 -nF -- "$pat" "$log" | sed 's/^/    /'; } || true
      fail=1
    fi
  done
  # reportFailure (frust_web.js) logs `console.error("[frust-web]", err)`, which
  # Chrome prints as `[frust-web] <ErrorName>: <message>`; the severity tag in
  # Chrome's stderr prefix is not relied on (Chromium 149 prints it as INFO).
  if grep -qE '\[frust-web\] [A-Za-z]*Error\b' "$log"; then
    echo "FAIL [$label]: [frust-web] reported an error"
    { grep -m5 -nE '\[frust-web\] [A-Za-z]*Error\b' "$log" | sed 's/^/    /'; } || true
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
  {
    # reportFailure after a JS-side failure (module import, invalid ?module=):
    # the real console shape is `[frust-web] <ErrorName>: <message>`.
    echo "${pre}[frust-web] TypeError: Failed to fetch dynamically imported module\", source: http://127.0.0.1/frust_web.js (43)"
  } >"$d/jsfail.log"

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
  expect 1 jsfail.log 0 "reportFailure error line, lenient (rejected)"
  if [[ "$rc" == 0 ]]; then echo "self-test OK"; else echo "self-test FAILED"; fi
  return "$rc"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dist) [[ $# -ge 2 ]] || die "--dist needs a directory"; DIST="$2"; shift 2 ;;
    --out) [[ $# -ge 2 ]] || die "--out needs a directory"; OUT="$2"; shift 2 ;;
    --no-browser) NO_BROWSER=1; shift ;;
    --self-test) SELF_TEST=1; shift ;;
    -h|--help)
      # Print the leading comment block: from line 2 to the last line before the
      # first non-comment line (derived, so the header can change freely).
      end="$(awk 'NR>1 && !/^#/{print NR-1; exit}' "${BASH_SOURCE[0]}")"
      sed -n "2,${end}p" "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
      exit 0 ;;
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
[[ -d "$DIST" ]] || die "dist directory '$DIST' does not exist"
DIST="$(cd "$DIST" && pwd)"
for f in index.html frust_web.js pkg/app.js pkg/app_bg.wasm; do
  [[ -f "$DIST/$f" ]] || die "dist $DIST is missing $f"
done
echo "web-smoke: dist staged at $DIST"

# Artefacts never go into the dist tree: the website recipe ships every file
# under static/preview/live/, so a supplied --dist must stay pristine.
command -v python3 >/dev/null || die "python3 not found"
if [[ -n "$OUT" ]]; then
  # Resolve without creating anything: a rejected --out must leave no trace.
  OUT="$(python3 -c 'import os,sys; print(os.path.abspath(sys.argv[1]))' "$OUT")"
  case "$OUT/" in "$DIST/"*) die "--out $OUT must not be inside the dist tree $DIST" ;; esac
  mkdir -p "$OUT"
fi

PORT="$(python3 -c 'import socket
s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')"
python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$DIST" >/dev/null 2>&1 &
SERVER_PID=$!
# Readiness: our server must still be alive AND what answers on the port must
# be our tree (the fetched index.html equals the staged file byte for byte), so
# a stranger that grabbed the port in the bind-then-serve gap is not trusted.
ready=0
for _ in $(seq 1 50); do
  kill -0 "$SERVER_PID" 2>/dev/null || die "static server exited early (port 127.0.0.1:$PORT taken?)"
  if python3 -c "import urllib.request,sys
body=urllib.request.urlopen('http://127.0.0.1:$PORT/index.html', timeout=1).read()
sys.exit(0 if body == open(sys.argv[1],'rb').read() else 1)" "$DIST/index.html" 2>/dev/null; then
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

if [[ -z "$OUT" ]]; then
  OUT="$(mktemp -d)"
fi

# Per-boot wall-clock bound: timeout (GNU/Linux) or gtimeout (coreutils on
# macOS) when present; stock macOS has neither, so run unwrapped and let
# --virtual-time-budget bound the boot.
TIMEOUT_CMD=()
if command -v timeout >/dev/null; then
  TIMEOUT_CMD=(timeout 120)
elif command -v gtimeout >/dev/null; then
  TIMEOUT_CMD=(gtimeout 120)
else
  echo "web-smoke: no timeout/gtimeout on PATH; running Chrome unwrapped (--virtual-time-budget still bounds each boot)"
fi

# Chrome's sandbox needs user namespaces that hosted runners and containers
# lack; local runs keep it unless opted out.
SANDBOX_FLAGS=()
if [[ -n "${CI:-}" || "${FRUST_WEB_SMOKE_NO_SANDBOX:-0}" == 1 ]]; then
  SANDBOX_FLAGS=(--no-sandbox)
fi

# Adapter flag ladder (see the header). Order: ANGLE-on-SwiftShader WebGL (the
# documented unsafe fallback), plain SwiftShader GLES, SwiftShader Vulkan for
# WebGPU, WebGPU with no backend hint, and finally no GPU flags at all (a host
# with a real GPU, or a Chrome that no longer honours the SwiftShader flags).
GPU_FLAG_SETS=(
  "--use-gl=angle --use-angle=swiftshader-webgl --enable-unsafe-swiftshader"
  "--use-gl=angle --use-angle=swiftshader --enable-unsafe-swiftshader"
  "--enable-unsafe-webgpu --enable-features=Vulkan --use-vulkan=swiftshader --use-webgpu-adapter=swiftshader --use-angle=swiftshader --enable-unsafe-swiftshader"
  "--enable-unsafe-webgpu"
  ""
)
if [[ -n "${FRUST_WEB_SMOKE_GPU_FLAGS+x}" ]]; then
  GPU_FLAG_SETS=("$FRUST_WEB_SMOKE_GPU_FLAGS")
fi

echo "web-smoke: chrome: $("$CHROME" --version 2>/dev/null || echo "$CHROME (version unknown)")"

# boot <arm> <url> <log> <shot> <gpu flags...>: one headless boot, log asserted later.
boot() {
  local arm="$1" url="$2" log="$3" shot="$4"; shift 4
  local rc=0
  # shellcheck disable=SC2068 # the flag set is a deliberate word-split list
  ${TIMEOUT_CMD[@]+"${TIMEOUT_CMD[@]}"} "$CHROME" --headless=new ${SANDBOX_FLAGS[@]+"${SANDBOX_FLAGS[@]}"} \
    --disable-dev-shm-usage --ignore-gpu-blocklist $@ \
    --enable-logging=stderr --v=0 \
    --virtual-time-budget="$BUDGET" --window-size=400,300 \
    --screenshot="$shot" "$url" >/dev/null 2>"$log" || rc=$?
  if [[ "$rc" != 0 ]]; then echo "web-smoke: chrome exited $rc for [$arm] (log still asserted)"; fi
}

overall=0
for arm in default webgl; do
  q="$QUERY_BASE"
  if [[ "$arm" == webgl ]]; then q="$q&arm=webgl"; fi
  url="http://127.0.0.1:$PORT/index.html?$q"
  log="$OUT/smoke-$arm.log"
  shot="$OUT/smoke-$arm.png"
  n=0
  winner=""
  for flags in "${GPU_FLAG_SETS[@]}"; do
    n=$((n + 1))
    attempt_log="$OUT/smoke-$arm-$n.log"
    attempt_shot="$OUT/smoke-$arm-$n.png"
    echo "web-smoke: booting [$arm] attempt $n with adapter flags: ${flags:-<none>}"
    # shellcheck disable=SC2086 # the flag set is a deliberate word-split list
    boot "$arm" "$url" "$attempt_log" "$attempt_shot" $flags
    cp -f "$attempt_log" "$log"; cp -f "$attempt_shot" "$shot" 2>/dev/null || true
    if grep -qF 'frust: render surface online' "$attempt_log"; then
      winner="${flags:-<none>}"
      break
    fi
    # Only the adapter-less outcome is worth retrying with other flags; a
    # panic or a missing init line is the same under every flag set.
    if ! grep -qF 'no compatible GPU adapter' "$attempt_log"; then break; fi
  done
  if [[ -n "$winner" ]]; then
    echo "web-smoke: [$arm] render surface online with adapter flags: $winner (attempt $n)"
  else
    echo "web-smoke: [$arm] no attempt brought a render surface online ($n flag set(s) tried)"
  fi
  echo "web-smoke: screenshot [$arm]: $shot"
  echo "web-smoke: log [$arm]: $log"
  check_log "$log" "$STRICT" "$arm" || overall=1
done
echo "web-smoke: artifacts (logs, screenshots) in $OUT"
exit "$overall"
