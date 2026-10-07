#!/usr/bin/env bash
# Android patch-load probe: can an installed Frust app load a code library delivered after install?
#
# Scaffolds a throwaway `frust create` app into a mktemp dir (never into this repo), overlays
# app_lib.rs as its src/lib.rs, adds the frust-hotpatch / frust-paths / libloading dependencies,
# builds and installs the debug APK (arm64-v8a), builds patch/ with cargo-ndk, pushes the .so to
# /data/local/tmp and copies it into the app's files dir with `run-as`. Then it force-stops and
# launches the app and prints the `frust-probe:` logcat lines (one per strategy: memfd,
# plain-dlopen, cache-dir), plus any SELinux denials logged meanwhile. Load-only: no jump table,
# no relocation. See README.md.
#
# Usage: probe.sh [--serial <adb-serial>] [--keep] [--log-dir <dir>]
#   --serial   adb device serial (default: $ANDROID_SERIAL; required when several devices attach)
#   --keep     keep the scaffold dir and leave the probe app installed (default: remove both)
#   --log-dir  also write device.txt, probe.txt and logcat.txt into <dir> (must exist)
#
# Needs: cargo-ndk, the aarch64-linux-android Rust target, ANDROID_HOME + an NDK, a JDK for
# Gradle, adb. Exit 0 when all three strategy lines were captured (whatever they say), else 1.
set -uo pipefail

SERIAL="${ANDROID_SERIAL:-}"
KEEP=0
LOG_DIR=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --serial) SERIAL="${2:?--serial needs a value}"; shift 2 ;;
        --keep) KEEP=1; shift ;;
        --log-dir) LOG_DIR="${2:?--log-dir needs a value}"; shift 2 ;;
        -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
        *) echo "probe.sh: unknown argument: $1" >&2; exit 2 ;;
    esac
done

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(git -C "$HERE" rev-parse --show-toplevel)" || { echo "probe.sh: not in a git checkout" >&2; exit 2; }
PATCH_SO="libfrust_probe_patch.so"
DEVICE_TMP="/data/local/tmp/$PATCH_SO"

ADB=(adb)
[[ -n "$SERIAL" ]] && ADB=(adb -s "$SERIAL")

# Canonical (`pwd -P`): on macOS the temp dir sits behind the /var -> /private/var symlink, and the
# relative path dependencies `cargo add` writes must be computed from the real location.
WORK="$(mktemp -d "${TMPDIR:-/tmp}/frust-android-probe.XXXXXX")" || exit 2
WORK="$(cd "$WORK" && pwd -P)" || exit 2
APP="$WORK/probeapp"
APP_ID=""

cleanup() {
    "${ADB[@]}" shell rm -f "$DEVICE_TMP" >/dev/null 2>&1
    if [[ "$KEEP" == 1 ]]; then
        echo "probe.sh: kept $WORK (app $APP_ID left installed)"
    else
        [[ -n "$APP_ID" ]] && "${ADB[@]}" uninstall "$APP_ID" >/dev/null 2>&1
        rm -rf "$WORK"
    fi
}
trap cleanup EXIT

die() { echo "probe.sh: $*" >&2; exit 1; }
step() { echo "== $*"; }

"${ADB[@]}" get-state >/dev/null 2>&1 || die "no device reachable through: ${ADB[*]}"

# Run the repo's own CLI from the current directory.
frust() { cargo run -q --manifest-path "$REPO/Cargo.toml" -p frust-cli -- "$@"; }

step "scaffold $APP"
(cd "$REPO" && frust create "$APP" --frust-path "$REPO/crates/frust" --org dev.frust.probe \
    --project-name probeapp --platforms android) || die "frust create failed"
cp "$HERE/app_lib.rs" "$APP/src/lib.rs" || die "overlay app_lib.rs failed"
for dep in "frust-hotpatch --path $REPO/crates/frust-hotpatch" \
    "frust-paths --path $REPO/crates/frust-paths" "libloading@0.8.8"; do
    # shellcheck disable=SC2086 # word-splitting the dependency spec is intended
    cargo add -q --manifest-path "$APP/Cargo.toml" $dep || die "cargo add $dep failed"
done

APP_ID="$(sed -n 's/^ *applicationId = "\(.*\)"$/\1/p' "$APP/android/app/build.gradle.kts" | head -1)"
[[ -n "$APP_ID" ]] || die "no applicationId in $APP/android/app/build.gradle.kts"

step "build debug APK (arm64-v8a) for $APP_ID"
(cd "$APP" && frust build apk --debug --target-platform android-arm64) || die "frust build apk failed"
APK="$(find "$APP/build/android/app/outputs/apk" -name '*debug*.apk' | head -1)"
[[ -f "$APK" ]] || die "no debug APK under $APP/build/android/app/outputs/apk"

step "install $APK"
"${ADB[@]}" install -r "$APK" || die "adb install failed"

step "build patch/ with cargo-ndk"
cp -R "$HERE/patch" "$WORK/patch" || die "copy patch/ failed"
(cd "$WORK/patch" && cargo ndk -t arm64-v8a build --release) || die "cargo ndk build failed"
SO="$WORK/patch/target/aarch64-linux-android/release/$PATCH_SO"
[[ -f "$SO" ]] || die "no $SO"

step "deliver $PATCH_SO into the files dir of $APP_ID"
"${ADB[@]}" push "$SO" "$DEVICE_TMP" || die "adb push failed"
"${ADB[@]}" shell run-as "$APP_ID" mkdir -p files || die "run-as mkdir failed"
"${ADB[@]}" shell run-as "$APP_ID" rm -f "cache/$PATCH_SO"
if ! "${ADB[@]}" shell run-as "$APP_ID" cp "$DEVICE_TMP" "files/$PATCH_SO"; then
    # Fallback when run-as cannot read /data/local/tmp: stream the bytes through the shell.
    "${ADB[@]}" shell "cat $DEVICE_TMP | run-as $APP_ID sh -c 'cat > files/$PATCH_SO'" \
        || die "copying into the files dir failed"
fi
"${ADB[@]}" shell run-as "$APP_ID" ls -l "files/$PATCH_SO"

step "launch"
ACTIVITY="$("${ADB[@]}" shell cmd package resolve-activity --brief -c android.intent.category.LAUNCHER "$APP_ID" | tr -d '\r' | tail -1)"
[[ "$ACTIVITY" == */* ]] || die "no launcher activity for $APP_ID: $ACTIVITY"
"${ADB[@]}" shell am force-stop "$APP_ID"
"${ADB[@]}" logcat -c
"${ADB[@]}" shell am start -W -n "$ACTIVITY" >/dev/null || die "am start failed"

LINES=""
for _ in $(seq 1 30); do
    LINES="$("${ADB[@]}" logcat -d | grep 'frust-probe:')"
    [[ "$(grep -c 'strategy=' <<<"$LINES")" -ge 3 ]] && break
    sleep 1
done
LOGCAT="$("${ADB[@]}" logcat -d)"

DEVICE="model=$("${ADB[@]}" shell getprop ro.product.model | tr -d '\r')
android=$("${ADB[@]}" shell getprop ro.build.version.release | tr -d '\r') (sdk $("${ADB[@]}" shell getprop ro.build.version.sdk | tr -d '\r'))
fingerprint=$("${ADB[@]}" shell getprop ro.build.fingerprint | tr -d '\r')
app=$APP_ID"

echo
echo "== device"
echo "$DEVICE"
echo "== frust-probe lines"
echo "$LINES"
echo "== SELinux denials while the probe ran"
grep 'avc: *denied' <<<"$LOGCAT" || echo "(none)"

if [[ -n "$LOG_DIR" ]]; then
    printf '%s\n' "$DEVICE" >"$LOG_DIR/device.txt"
    printf '%s\n' "$LINES" >"$LOG_DIR/probe.txt"
    printf '%s\n' "$LOGCAT" >"$LOG_DIR/logcat.txt"
fi

[[ "$(grep -c 'strategy=' <<<"$LINES")" -ge 3 ]] || die "fewer than three frust-probe lines captured"
