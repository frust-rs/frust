#!/usr/bin/env bash
# scripts/testing/android-smoke.sh — end-to-end Android device smoke check.
#
# Scaffolds a brand-new template app AND builds the checked-in `playground`
# example, installs both on one attached Android target, launches each, and
# asserts on real logcat output — the shape of check that would have caught
# both Android-only defects that shipped unseen without it: a `frust_paths`
# regression on the Android arm, and an insets/legacy-theme bug that only
# shows up once a real device reports non-zero system-bar insets. Neither
# defect is visible to a desktop-only or headless test.
#
# LEG "template" proves the theme/insets fix from a FRESH scaffold (nothing
# baked into a checked-in project could paper over a regression in the
# scaffold templates themselves):
#   1. `frust create` a throwaway app under a scratch directory outside this
#      repository, path-dependency-pinned onto this checkout via the hidden
#      `--frust-path` flag.
#   2. `frust build apk --debug` from inside the scaffold.
#   3. Install, force-stop, launch, and assert on logcat:
#        - a `frust-insets` line whose `view_padding` block has t>0 AND b>0
#          (proves the shell is reporting real system-bar/gesture insets,
#          not the legacy zeroed-out layout);
#        - the legacy-theme warning ("Frust: AndroidManifest.xml still names
#          a legacy theme") is ABSENT (proves the generated manifest already
#          carries the corrected theme).
#      An optional, best-effort visual probe screencaps the launched app and
#      samples the top/bottom bands for non-black pixels; it is skipped (not
#      failed) when `python3`+Pillow aren't available, since it is a
#      belt-and-suspenders check on top of the logcat assertions above, never
#      the thing this script's exit code rests on.
#
# LEG "playground" proves `frust-database` on-device plus the same insets
# check, against `examples/playground` (already installed/committed, not
# re-scaffolded):
#   1. `frust build apk --debug` from `examples/playground`.
#   2. Install, launch via the `frustplay://section/db` deep link (routes
#      straight to the DB section and auto-runs the smoke query — no taps
#      needed), and assert on logcat:
#        - `frust-database smoke: ok` is present (the query round-tripped);
#        - `frust-database smoke: ... error` is ABSENT (fails fast rather
#          than waiting out the timeout if the query errored);
#        - the same non-zero `frust-insets` check as leg "template".
#
# Usage:
#   scripts/testing/android-smoke.sh [--serial SERIAL] [--keep] [--repo PATH]
#                                     [--leg template|playground|all]
#                                     [--target-platform CSV]
#
#   --serial SERIAL       adb serial of the target device. Defaults to
#                          $ANDROID_SERIAL, else the single device `adb devices`
#                          reports; more than one attached device with neither
#                          set is a hard error (pass --serial to disambiguate).
#   --keep                Keep the scratch scaffold directory (leg "template")
#                          instead of removing it on exit; the kept path is
#                          printed in the final summary.
#   --leg WHICH           Which leg(s) to run: `template`, `playground`, or
#                          `all` (default: `all`).
#   --repo PATH           Repository root to build against. Defaults to the
#                          root derived from this script's own location.
#   --target-platform CSV ABI(s) to build: comma-separated list of
#                          `android-arm64`, `android-arm`, or `android-x64`.
#                          Forwarded verbatim to `frust build apk --debug`.
#                          Defaults to the CLI's default (all three ABIs).
#                          Can also be set via $ANDROID_SMOKE_TARGET_PLATFORM.
#                          A phone typically wants `android-arm64`; an x86_64
#                          emulator wants `android-x64`. The target must be
#                          installed via `rustup target add`.
#
# Cleanup and artifacts:
#   - The scratch scaffold (leg "template") is created under `mktemp -d` and
#     removed via `trap cleanup EXIT` on normal exit (unless `--keep` is given).
#     The trap runs on all exit paths, including `exit 3` from failed build/install
#     steps, so the scratch directory is always cleaned unless explicitly kept.
#   - Artifacts (logcat dumps and visual-probe PNGs) are written to
#     $ARTIFACTS_DIR, which defaults to `$REPO_ROOT/build/android-smoke`.
#     Override via the $ANDROID_SMOKE_ARTIFACTS_DIR environment variable.
#     The build/ directory is git-ignored; artifacts persist across runs.
#   - Each leg's logcat (`logcat-template.log` / `logcat-playground.log`) is
#     dumped at the end of the leg on pass AND on assertion failure, and also
#     when `adb install` or `adb shell am start` fails (exit 3) — in that case
#     adb's own output is echoed to stderr first, so an install/launch failure
#     never leaves the run without diagnostics.
#
# Toolchain notes:
#   - Requires `adb`, `cargo-ndk`, a JDK 17+ (via $JAVA_HOME or PATH), and
#     $ANDROID_HOME pointing at an installed Android SDK. Each missing piece
#     fails fast with a named message rather than failing deep inside a
#     Gradle build; `cargo run -p frust-cli -- doctor` is also run as a
#     backstop and its output is surfaced verbatim on any failing check.
#   - This script does not set $CARGO_TARGET_DIR, $ANDROID_HOME, or
#     $JAVA_HOME itself — it inherits whatever the caller's environment
#     already has, so a shared/warm target directory keeps working exactly
#     as the caller configured it.
#   - A dozing device reports zeroed insets and an all-black screencap: this
#     script wakes the device (`input keyevent KEYCODE_WAKEUP`) and dismisses
#     the keyguard (`wm dismiss-keyguard`) before every launch and before any
#     screencap.
#   - A target whose gesture/navigation bar is hidden cannot ever report a
#     non-zero bottom inset (`b>0` requires a visible gesture or 3-button nav
#     bar) — do not point this script at such a device as a pass criterion.
#   - `shellcheck`/`actionlint` are not assumed to be installed; when either
#     is unavailable this script is still checked with `bash -n` and the
#     caller should note the gap rather than install anything.
#
# NEGATIVE CONTROL (documented, not automated by this script): pointing this
# script at an APK built from a manifest that still names the legacy theme
# must fail leg "template"'s steps 3/4 above (the insets line reports a
# zeroed `view_padding`, or the legacy-theme warning is present). Running
# that once, by hand, against a deliberately-reverted manifest is a periodic
# gate check, not part of this script's own run.
#
# Exit codes:
#   0   every assertion in every requested leg passed.
#   1   a usage error (bad argument, ambiguous/missing device) or an adb
#       command reported a usage error (adb serial not found, etc.).
#   2   a missing toolchain component (adb/JDK/cargo-ndk/ANDROID_HOME) or a
#       failing `frust doctor` check.
#   3   a build, install, or launch step failed (frust build apk, adb install,
#       adb shell am start, or similar device interaction).
#   4   a logcat assertion failed (the actual smoke-check failure this
#       script exists to catch).

set -euo pipefail

# --- Derive repo root from script location -----------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
DEFAULT_REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." >/dev/null 2>&1 && pwd)"

# --- Arg parsing --------------------------------------------------------------

REPO_ROOT="$DEFAULT_REPO_ROOT"
SERIAL="${ANDROID_SERIAL:-}"
KEEP=0
LEG="all"
TARGET_PLATFORM="${ANDROID_SMOKE_TARGET_PLATFORM:-}"

usage() {
  cat <<'EOF' >&2
Usage: android-smoke.sh [--serial SERIAL] [--keep] [--leg template|playground|all] [--repo PATH] [--target-platform CSV]
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --serial)
      if [ $# -lt 2 ]; then
        echo "error: --serial requires an adb serial argument" >&2
        exit 1
      fi
      SERIAL="$2"
      shift 2
      ;;
    --keep)
      KEEP=1
      shift
      ;;
    --leg)
      if [ $# -lt 2 ]; then
        echo "error: --leg requires an argument (template|playground|all)" >&2
        exit 1
      fi
      LEG="$2"
      shift 2
      ;;
    --repo)
      if [ $# -lt 2 ]; then
        echo "error: --repo requires a path argument" >&2
        exit 1
      fi
      REPO_ROOT="$2"
      shift 2
      ;;
    --target-platform)
      if [ $# -lt 2 ]; then
        echo "error: --target-platform requires a comma-separated ABI list" >&2
        exit 1
      fi
      TARGET_PLATFORM="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      usage
      exit 1
      ;;
  esac
done

case "$LEG" in
  template|playground|all) ;;
  *)
    echo "error: --leg must be one of: template, playground, all (got: $LEG)" >&2
    exit 1
    ;;
esac

REPO_ROOT="$(cd "$REPO_ROOT" >/dev/null 2>&1 && pwd)"

# --- Artifacts directory -------------------------------------------------------

ARTIFACTS_DIR="${ANDROID_SMOKE_ARTIFACTS_DIR:-$REPO_ROOT/build/android-smoke}"
mkdir -p "$ARTIFACTS_DIR"

# Confirm build/android-smoke is git-ignored
if ! git -C "$REPO_ROOT" check-ignore "$ARTIFACTS_DIR" >/dev/null 2>&1; then
  echo "warning: $ARTIFACTS_DIR is not git-ignored; artifacts may be committed" >&2
fi

# --- Scratch cleanup on exit ---------------------------------------------------

SCRATCH=""

cleanup() {
  if [ -n "$SCRATCH" ] && [ "$KEEP" -eq 0 ] && [ -d "$SCRATCH" ]; then
    rm -rf "$SCRATCH"
  fi
}

trap cleanup EXIT

# --- Result tracking -----------------------------------------------------------

FAILED_ASSERTIONS=0
PASS_LINES=()
FAIL_LINES=()

pass() {
  local msg="$1"
  echo "[PASS] $msg" >&2
  PASS_LINES+=("$msg")
}

fail_assertion() {
  local msg="$1"
  echo "[FAIL] $msg" >&2
  FAIL_LINES+=("$msg")
  FAILED_ASSERTIONS=$((FAILED_ASSERTIONS + 1))
}

note() {
  echo "[NOTE] $1" >&2
}

fail_usage() {
  echo "error: $1" >&2
  exit 1
}

fail_tool() {
  echo "error: $1" >&2
  exit 2
}

fail_build() {
  echo "error: $1" >&2
  exit 3
}

# --- Device command helpers ---------------------------------------------------

# Path of the current leg's logcat artifact. Each leg sets it before its
# first device step so a failing install/launch can still dump logcat there.
LEG_LOGCAT=""

# Shared failure path for the device-step wrappers: surface the adb output
# that was captured, dump the device logcat to the current leg's artifact (if
# a leg has declared one), then exit 3.
_device_step_failed() {
  local serial="$1" what="$2" output="$3"
  if [ -n "$output" ]; then
    echo "--- $what output ---" >&2
    printf '%s\n' "$output" >&2
    echo "--- end $what output ---" >&2
  fi
  if [ -n "$LEG_LOGCAT" ]; then
    dump_logcat "$serial" "$LEG_LOGCAT"
    [ -s "$LEG_LOGCAT" ] && echo "logcat dumped to $LEG_LOGCAT" >&2
  fi
  fail_build "$what failed on $serial"
}

# Wraps `adb install` and routes failure to fail_build(3). adb's own
# stdout+stderr is captured and shown only on failure.
adb_install() {
  local serial="$1" apk="$2" output=""
  if ! output="$(adb -s "$serial" install -r "$apk" 2>&1)"; then
    _device_step_failed "$serial" "adb install" "$output"
  fi
}

# Wraps `adb shell am start -W` and routes failure to fail_build(3). adb's
# own stdout+stderr is captured and shown only on failure.
# Arguments: serial, am-start-args (passed verbatim after `-W`)
adb_shell_am_start() {
  local serial="$1" output=""
  shift
  if ! output="$(adb -s "$serial" shell am start -W "$@" 2>&1)"; then
    _device_step_failed "$serial" "adb shell am start" "$output"
  fi
}

# Dumps full logcat to a file. Best-effort: skipped (with a note) when the
# device is not reachable — `adb logcat` blocks indefinitely waiting for a
# missing device, which would turn an install/launch failure into a hang —
# and bounded by `timeout` where that binary exists (Linux; macOS lacks it).
dump_logcat() {
  local serial="$1" outfile="$2"
  if ! adb -s "$serial" get-state >/dev/null 2>&1; then
    note "device $serial not reachable; skipping logcat dump to $outfile"
    return 0
  fi
  if command -v timeout >/dev/null 2>&1; then
    timeout 60 adb -s "$serial" logcat -d >"$outfile" 2>/dev/null || true
  else
    adb -s "$serial" logcat -d >"$outfile" 2>/dev/null || true
  fi
}

# --- Preflight: required tools ------------------------------------------------

require_adb() {
  command -v adb >/dev/null 2>&1 || fail_tool "adb not found on PATH; install Android SDK platform-tools"
}

require_android_home() {
  if [ -z "${ANDROID_HOME:-}" ]; then
    fail_tool "ANDROID_HOME is not set; export it to an installed Android SDK"
  fi
  if [ ! -d "$ANDROID_HOME" ]; then
    fail_tool "ANDROID_HOME ($ANDROID_HOME) is not a directory"
  fi
}

require_jdk() {
  if [ -n "${JAVA_HOME:-}" ] && [ -x "${JAVA_HOME}/bin/java" ]; then
    return 0
  fi
  command -v java >/dev/null 2>&1 && return 0
  fail_tool "no JDK 17+ found; set JAVA_HOME or put java on PATH"
}

require_cargo_ndk() {
  command -v cargo-ndk >/dev/null 2>&1 || fail_tool "cargo-ndk not found on PATH; run: cargo install cargo-ndk"
}

run_doctor_backstop() {
  local log="$1"
  if ! (cd "$REPO_ROOT" && cargo run --manifest-path "$REPO_ROOT/Cargo.toml" -p frust-cli -- doctor) >"$log" 2>&1; then
    echo "--- frust doctor output ---" >&2
    cat "$log" >&2
    echo "--- end frust doctor output ---" >&2
    fail_tool "\`frust doctor\` reported a failing toolchain check; see output above"
  fi
}

# --- Device resolution ---------------------------------------------------------

resolve_serial() {
  if [ -n "$SERIAL" ]; then
    return 0
  fi
  local devices=()
  while IFS= read -r line; do
    devices+=("$line")
  done < <(adb devices | awk 'NR>1 && $2=="device" {print $1}')
  case "${#devices[@]}" in
    0)
      fail_usage "no attached device (state=device) found; attach one or pass --serial"
      ;;
    1)
      SERIAL="${devices[0]}"
      ;;
    *)
      fail_usage "multiple attached devices found (${devices[*]}); pass --serial to disambiguate"
      ;;
  esac
}

wake_device() {
  local serial="$1"
  adb -s "$serial" shell input keyevent KEYCODE_WAKEUP >/dev/null 2>&1 || true
  adb -s "$serial" shell wm dismiss-keyguard >/dev/null 2>&1 || true
}

# --- CLI invocation ------------------------------------------------------------

cli() {
  cargo run --manifest-path "$REPO_ROOT/Cargo.toml" -p frust-cli -- "$@"
}

# Runs `frust build apk --debug` with cwd = $1 (a project root containing
# `android/`), tees combined output to $2 for diagnostics, and echoes the
# absolute APK path parsed from the CLI's own `Built: <path>` line (the last
# such line, in case a future flag ever produces more than one).
# Forwards --target-platform $TARGET_PLATFORM when set.
build_apk() {
  local project_dir="$1" build_log="$2"
  local -a build_args=(build apk --debug)
  if [ -n "$TARGET_PLATFORM" ]; then
    build_args+=(--target-platform "$TARGET_PLATFORM")
  fi
  if ! (cd "$project_dir" && cli "${build_args[@]}") >"$build_log" 2>&1; then
    cat "$build_log" >&2
    fail_build "\`frust build apk --debug\` failed in $project_dir; see output above"
  fi
  local apk
  apk="$(grep '^Built: ' "$build_log" | tail -n1 | sed 's/^Built: //')"
  if [ -z "$apk" ] || [ ! -f "$apk" ]; then
    cat "$build_log" >&2
    fail_build "could not locate the built APK from $project_dir's \`Built:\` output"
  fi
  echo "$apk"
}

# Extracts the applicationId (== the Android package) from a generated
# project's `android/app/build.gradle.kts` rather than assuming a fixed
# org/project-name mapping.
application_id() {
  local project_dir="$1"
  local gradle="$project_dir/android/app/build.gradle.kts"
  local id
  id="$(grep -m1 -E 'applicationId[[:space:]]*=[[:space:]]*"[^"]+"' "$gradle" | sed -E 's/.*"([^"]+)".*/\1/')"
  if [ -z "$id" ]; then
    fail_build "could not read applicationId from $gradle"
  fi
  echo "$id"
}

# --- Logcat assertions -----------------------------------------------------

# True iff the given logcat dump contains a `frust-insets` line whose
# `view_padding` block has both t>0 and b>0 — proof of real, non-zeroed
# system-bar/gesture insets rather than the legacy zeroed layout.
has_nonzero_insets() {
  local dump="$1" line t b
  while IFS= read -r line; do
    case "$line" in
      *frust-insets*view_padding*) ;;
      *) continue ;;
    esac
    if [[ "$line" =~ view_padding\ l=([0-9.]+)\ t=([0-9.]+)\ r=([0-9.]+)\ b=([0-9.]+) ]]; then
      t="${BASH_REMATCH[2]}"
      b="${BASH_REMATCH[4]}"
      if awk -v t="$t" -v b="$b" 'BEGIN { exit !(t > 0 && b > 0) }'; then
        return 0
      fi
    fi
  done <<<"$dump"
  return 1
}

# Polls `adb logcat -d -s frust` for up to $2 seconds until $1's predicate
# (a function name taking the dump as its one argument) reports success.
poll_logcat() {
  local serial="$1" predicate="$2" timeout="$3"
  local start now dump
  start="$(date +%s)"
  while true; do
    dump="$(adb -s "$serial" logcat -d -s frust 2>/dev/null || true)"
    if "$predicate" "$dump"; then
      return 0
    fi
    now="$(date +%s)"
    if [ "$((now - start))" -ge "$timeout" ]; then
      return 1
    fi
    sleep 1
  done
}

_predicate_nonzero_insets() { has_nonzero_insets "$1"; }

# Line-scoped (never cross-line) substring checks: the dump is the whole
# multi-line logcat buffer, and matching "smoke:" and "error" independently
# anywhere in it could false-positive on two unrelated lines.
_predicate_db_smoke_ok() {
  local line
  while IFS= read -r line; do
    case "$line" in
      *"frust-database smoke: ok"*) return 0 ;;
    esac
  done <<<"$1"
  return 1
}

_predicate_db_smoke_error() {
  local line
  while IFS= read -r line; do
    case "$line" in
      *"frust-database smoke:"*"error"*) return 0 ;;
    esac
  done <<<"$1"
  return 1
}

# Polls once for either the smoke-ok line or the smoke-error line (whichever
# appears first), up to $2 seconds; echoes "ok", "error", or "timeout". A
# single race loop rather than two sequential bounded polls, so a real error
# is reported as soon as it appears instead of always paying a fixed wait.
wait_for_db_smoke() {
  local serial="$1" timeout="$2"
  local start now dump
  start="$(date +%s)"
  while true; do
    dump="$(adb -s "$serial" logcat -d -s frust 2>/dev/null || true)"
    if _predicate_db_smoke_error "$dump"; then
      echo "error"
      return 0
    fi
    if _predicate_db_smoke_ok "$dump"; then
      echo "ok"
      return 0
    fi
    now="$(date +%s)"
    if [ "$((now - start))" -ge "$timeout" ]; then
      echo "timeout"
      return 0
    fi
    sleep 1
  done
}

# --- Optional visual probe --------------------------------------------------

# Screencaps the device and, if python3+Pillow are available, samples a 4x4
# block in the top status-bar band and the bottom gesture band, failing only
# if every sampled pixel in a band is pure black. Never fails the gate on a
# missing optional tool — it reports a NOTE and returns success instead.
visual_probe() {
  local serial="$1" out_png="$2"
  wake_device "$serial"
  if ! adb -s "$serial" exec-out screencap -p >"$out_png" 2>/dev/null; then
    note "screencap failed; skipping the optional visual probe"
    return 0
  fi
  if ! command -v python3 >/dev/null 2>&1; then
    note "python3 not available; skipping the optional visual probe"
    return 0
  fi
  if ! python3 -c 'import PIL' >/dev/null 2>&1; then
    note "Pillow not available; skipping the optional visual probe"
    return 0
  fi
  if python3 - "$out_png" <<'PYEOF'
import sys
from PIL import Image

path = sys.argv[1]
img = Image.open(path).convert("RGB")
w, h = img.size

def band_all_black(x0, y0):
    for x in range(x0, min(x0 + 4, w)):
        for y in range(y0, min(y0 + 4, h)):
            if img.getpixel((x, y)) != (0, 0, 0):
                return False
    return True

top_black = band_all_black(w // 2, 0)
bottom_black = band_all_black(w // 2, max(h - 4, 0))
sys.exit(1 if (top_black or bottom_black) else 0)
PYEOF
  then
    pass "visual probe: top/bottom bands are not pure black ($out_png)"
  else
    fail_assertion "visual probe: a sampled band in $out_png is pure black"
  fi
}

# --- Leg: template -----------------------------------------------------------

run_leg_template() {
  local serial="$1"
  local scaffold build_log apk package png

  SCRATCH="$(mktemp -d)"
  if [ "$KEEP" -eq 1 ]; then
    note "keeping scratch scaffold directory: $SCRATCH"
  fi

  scaffold="$SCRATCH/smokeapp"
  echo "== leg template: scaffolding a fresh app at $scaffold ==" >&2
  if ! cli create "$scaffold" \
      --org dev.frust.smoke \
      --project-name smokeapp \
      --frust-path "$REPO_ROOT" \
      --platforms android \
      >"$SCRATCH/create.log" 2>&1; then
    cat "$SCRATCH/create.log" >&2
    fail_build "\`frust create\` failed; see output above"
  fi

  echo "== leg template: building the debug APK ==" >&2
  build_log="$SCRATCH/build.log"
  apk="$(build_apk "$scaffold" "$build_log")"
  package="$(application_id "$scaffold")"

  echo "== leg template: install, launch, and assert on $serial (package=$package) ==" >&2
  LEG_LOGCAT="$ARTIFACTS_DIR/logcat-template.log"
  adb_install "$serial" "$apk"
  adb -s "$serial" logcat -c
  wake_device "$serial"
  adb -s "$serial" shell am force-stop "$package" >/dev/null 2>&1 || true
  adb_shell_am_start "$serial" -n "$package/.MainActivity"

  if poll_logcat "$serial" _predicate_nonzero_insets 20; then
    pass "template: frust-insets reports non-zero view_padding (t>0, b>0)"
  else
    fail_assertion "template: no frust-insets line with non-zero view_padding within timeout"
  fi

  local dump
  dump="$(adb -s "$serial" logcat -d -s frust 2>/dev/null || true)"
  if [[ "$dump" == *"Frust: AndroidManifest.xml still names a legacy theme"* ]]; then
    fail_assertion "template: legacy-theme warning present in a fresh scaffold"
  else
    pass "template: legacy-theme warning is absent"
  fi

  png="$ARTIFACTS_DIR/template.png"
  visual_probe "$serial" "$png"

  dump_logcat "$serial" "$LEG_LOGCAT"

  adb -s "$serial" shell am force-stop "$package" >/dev/null 2>&1 || true
  adb -s "$serial" uninstall "$package" >/dev/null 2>&1 || true
}

# --- Leg: playground ---------------------------------------------------------

run_leg_playground() {
  local serial="$1"
  local project_dir build_log apk package

  project_dir="$REPO_ROOT/examples/playground"
  build_log="$(mktemp)"

  echo "== leg playground: building the debug APK ==" >&2
  apk="$(build_apk "$project_dir" "$build_log")"
  package="$(application_id "$project_dir")"

  echo "== leg playground: install, launch via deep link, and assert on $serial (package=$package) ==" >&2
  LEG_LOGCAT="$ARTIFACTS_DIR/logcat-playground.log"
  adb_install "$serial" "$apk"
  adb -s "$serial" logcat -c
  wake_device "$serial"
  adb_shell_am_start "$serial" \
    -a android.intent.action.VIEW \
    -d "frustplay://section/db" \
    "$package"

  local db_result
  db_result="$(wait_for_db_smoke "$serial" 60)"
  case "$db_result" in
    ok)
      pass "playground: frust-database smoke: ok observed in logcat"
      ;;
    error)
      fail_assertion "playground: frust-database smoke reported an error"
      ;;
    *)
      fail_assertion "playground: no frust-database smoke: ok within timeout"
      ;;
  esac

  if poll_logcat "$serial" _predicate_nonzero_insets 20; then
    pass "playground: frust-insets reports non-zero view_padding (t>0, b>0)"
  else
    fail_assertion "playground: no frust-insets line with non-zero view_padding within timeout"
  fi

  dump_logcat "$serial" "$LEG_LOGCAT"

  rm -f "$build_log"
  # Deliberately left installed per the header's leave-it-installed policy.
}

# --- Main --------------------------------------------------------------------

require_adb
require_android_home
require_jdk
require_cargo_ndk

DOCTOR_LOG="$(mktemp)"
run_doctor_backstop "$DOCTOR_LOG"
rm -f "$DOCTOR_LOG"

resolve_serial
echo "using device: $SERIAL" >&2

if [ "$LEG" = "template" ] || [ "$LEG" = "all" ]; then
  run_leg_template "$SERIAL"
fi
if [ "$LEG" = "playground" ] || [ "$LEG" = "all" ]; then
  run_leg_playground "$SERIAL"
fi

echo "" >&2
echo "== summary ==" >&2
for line in "${PASS_LINES[@]:-}"; do
  [ -n "$line" ] && echo "PASS: $line" >&2
done
for line in "${FAIL_LINES[@]:-}"; do
  [ -n "$line" ] && echo "FAIL: $line" >&2
done

echo "artifacts directory: $ARTIFACTS_DIR" >&2
if [ "$KEEP" -eq 1 ] && [ -n "$SCRATCH" ]; then
  echo "scratch directory (kept): $SCRATCH" >&2
fi

if [ "$FAILED_ASSERTIONS" -eq 0 ]; then
  echo "android-smoke: all assertions passed (leg=$LEG, device=$SERIAL)" >&2
  exit 0
fi

echo "android-smoke: $FAILED_ASSERTIONS assertion(s) failed (leg=$LEG, device=$SERIAL)" >&2
exit 4
