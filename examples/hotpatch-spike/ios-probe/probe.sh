#!/usr/bin/env bash
# iOS simulator patch-load probe: can a simctl-launched Frust app dlopen a dylib from its container?
#
# Scaffolds a throwaway `frust create` app into a mktemp dir (never into this repo), overlays
# app_lib.rs as its src/lib.rs, adds the frust-hotpatch / frust-paths / libloading dependencies,
# builds it for the simulator with xcodebuild and installs it with `simctl install`. Builds patch/
# for aarch64-apple-ios-sim and copies four files into the app's data container (resolved with
# `simctl get_app_container <udid> <bundle-id> data`): the dylib with its signature removed, as the
# linker signed it, re-signed ad hoc, and 11 bytes of text (negative control). Then it launches the
# app once per strategy (plain-dlopen, linker-signed, adhoc-signed, negative-control) with
# `simctl launch --console-pty`, prints the `frust-probe:` lines and the image `vmmap` shows, and
# finally launches once more with the files removed. Load-only: no jump table, no relocation.
#
# Usage: probe.sh [--udid <udid>] [--keep] [--log-dir <dir>]
#   --udid     simulator udid (default: $SIMULATOR_UDID, else the only booted iOS simulator)
#   --keep     keep the scaffold dir and leave the probe app installed (default: remove both)
#   --log-dir  also write the per-launch console logs, vmmap output, signatures and screenshots
#              into <dir> (must exist)
#
# Needs: Xcode with a booted iOS simulator, the aarch64-apple-ios-sim Rust target, an Apple
# Silicon host. Exit 0 when a frust-probe line was captured for every strategy, else 1.
set -uo pipefail

UDID="${SIMULATOR_UDID:-}"
KEEP=0
LOG_DIR=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --udid) UDID="${2:?--udid needs a value}"; shift 2 ;;
        --keep) KEEP=1; shift ;;
        --log-dir) LOG_DIR="${2:?--log-dir needs a value}"; shift 2 ;;
        -h|--help) sed -n '2,21p' "$0"; exit 0 ;;
        *) echo "probe.sh: unknown argument: $1" >&2; exit 2 ;;
    esac
done
[[ -z "$LOG_DIR" || -d "$LOG_DIR" ]] || { echo "probe.sh: --log-dir $LOG_DIR does not exist" >&2; exit 2; }

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(git -C "$HERE" rev-parse --show-toplevel)" || { echo "probe.sh: not in a git checkout" >&2; exit 2; }
TRIPLE="aarch64-apple-ios-sim"
STRATEGIES=(plain-dlopen linker-signed adhoc-signed negative-control)
# strategy -> delivered file name; app_lib.rs's STRATEGIES table must match.
file_for() {
    case "$1" in
        plain-dlopen) echo libfrust_probe_unsigned.dylib ;;
        linker-signed) echo libfrust_probe_linker.dylib ;;
        adhoc-signed) echo libfrust_probe_adhoc.dylib ;;
        negative-control) echo libfrust_probe_control.dylib ;;
    esac
}

die() { echo "probe.sh: $*" >&2; exit 1; }
step() { echo "== $*"; }
# Hide the udid in anything printed or logged.
redact() { if [[ -n "$UDID" ]]; then sed "s/$UDID/<udid>/g"; else cat; fi; }
save() { if [[ -n "$LOG_DIR" ]]; then redact >"$LOG_DIR/$1"; else cat >/dev/null; fi; }

[[ "$(uname -m)" == arm64 ]] || die "needs an Apple Silicon host (the patch is built for $TRIPLE)"
if [[ -z "$UDID" ]]; then
    BOOTED="$(xcrun simctl list devices booted | sed -n 's/.*(\([0-9A-F-]\{36\}\)) (Booted).*/\1/p')"
    [[ "$(grep -c . <<<"$BOOTED")" == 1 ]] || die "pass --udid: $(grep -c . <<<"$BOOTED") booted simulators"
    UDID="$BOOTED"
fi
xcrun simctl list devices booted | grep -q "($UDID) (Booted)" || die "simulator <udid> is not booted"

# Canonical (`pwd -P`): on macOS the temp dir sits behind the /var -> /private/var symlink, and the
# relative path dependencies `cargo add` writes must be computed from the real location.
WORK="$(mktemp -d "${TMPDIR:-/tmp}/frust-ios-probe.XXXXXX")" || exit 2
WORK="$(cd "$WORK" && pwd -P)" || exit 2
APP="$WORK/probeapp"
BUNDLE_ID=""
INSTALLED=0
LAUNCH_PID=""

cleanup() {
    [[ -n "$LAUNCH_PID" ]] && kill "$LAUNCH_PID" >/dev/null 2>&1
    if [[ "$KEEP" == 1 ]]; then
        echo "probe.sh: kept $WORK (app $BUNDLE_ID left installed)"
    else
        if [[ "$INSTALLED" == 1 ]]; then
            xcrun simctl terminate "$UDID" "$BUNDLE_ID" >/dev/null 2>&1
            xcrun simctl uninstall "$UDID" "$BUNDLE_ID" >/dev/null 2>&1
        fi
        rm -rf "$WORK"
    fi
}
trap cleanup EXIT

# Run the repo's own CLI from the current directory.
frust() { cargo run -q --manifest-path "$REPO/Cargo.toml" -p frust-cli -- "$@"; }

step "scaffold $APP"
(cd "$WORK" && frust create "$APP" --frust-path "$REPO/crates/frust" --org dev.frust.probe \
    --project-name probeapp --platforms ios) || die "frust create failed"
cp "$HERE/app_lib.rs" "$APP/src/lib.rs" || die "overlay app_lib.rs failed"
for dep in "frust-hotpatch --path $REPO/crates/frust-hotpatch" \
    "frust-paths --path $REPO/crates/frust-paths" "libloading@0.8.8"; do
    # shellcheck disable=SC2086 # word-splitting the dependency spec is intended
    cargo add -q --manifest-path "$APP/Cargo.toml" $dep || die "cargo add $dep failed"
done

PBXPROJ="$APP/ios/Runner.xcodeproj/project.pbxproj"
BUNDLE_ID="$(sed -n 's/^.*PRODUCT_BUNDLE_IDENTIFIER = "\{0,1\}\([^";]*\)"\{0,1\};$/\1/p' "$PBXPROJ" | head -1)"
[[ -n "$BUNDLE_ID" ]] || die "no PRODUCT_BUNDLE_IDENTIFIER in $PBXPROJ"
if xcrun simctl get_app_container "$UDID" "$BUNDLE_ID" app >/dev/null 2>&1; then
    die "$BUNDLE_ID is already installed on the simulator; refusing to run (it would be overwritten and uninstalled)"
fi

# The same xcodebuild invocation `frust run` uses for the simulator (frust-drive ios_run/xcodebuild.rs).
step "build Debug-iphonesimulator for $BUNDLE_ID"
(cd "$APP" && xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Debug \
    -sdk iphonesimulator -destination "id=$UDID" -derivedDataPath build/ios ARCHS=arm64 build \
    >"$WORK/xcodebuild.log" 2>&1) || { tail -40 "$WORK/xcodebuild.log" | redact >&2; die "xcodebuild failed"; }
RUNNER_APP="$APP/build/ios/Build/Products/Debug-iphonesimulator/Runner.app"
[[ -d "$RUNNER_APP" ]] || die "no $RUNNER_APP"

step "install"
xcrun simctl install "$UDID" "$RUNNER_APP" || die "simctl install failed"
INSTALLED=1

step "build patch/ for $TRIPLE"
cp -R "$HERE/patch" "$WORK/patch" || die "copy patch/ failed"
(cd "$WORK/patch" && cargo build -q --release --target "$TRIPLE") || die "patch build failed"
DYLIB="$WORK/patch/target/$TRIPLE/release/libfrust_probe_patch.dylib"
[[ -f "$DYLIB" ]] || die "no $DYLIB"

step "prepare the four files"
STAGE="$WORK/stage"
mkdir -p "$STAGE"
cp "$DYLIB" "$STAGE/$(file_for plain-dlopen)"
codesign --remove-signature "$STAGE/$(file_for plain-dlopen)" || die "codesign --remove-signature failed"
cp "$DYLIB" "$STAGE/$(file_for linker-signed)"
cp "$DYLIB" "$STAGE/$(file_for adhoc-signed)"
codesign -f -s - "$STAGE/$(file_for adhoc-signed)" 2>/dev/null || die "codesign -s - failed"
printf 'not a dylib' >"$STAGE/$(file_for negative-control)"
SIGNATURES="$(for s in "${STRATEGIES[@]}"; do
    f="$STAGE/$(file_for "$s")"
    echo "-- $s: $(file -b "$f"), $(wc -c <"$f" | tr -d ' ') bytes"
    codesign -dv "$f" 2>&1 | grep -E 'Signature|Identifier|CodeDirectory|not signed' || true
done
echo "-- the app executable Runner.app/Runner (for comparison)"
codesign -dv "$RUNNER_APP" 2>&1 | grep -E 'Signature|Identifier|CodeDirectory|not signed' || true)"
echo "$SIGNATURES"
save signatures.txt <<<"$SIGNATURES"

# The data container may not exist until the first launch; fall back to one launch if so.
step "resolve the data container"
CONTAINER_HOW="xcrun simctl get_app_container <udid> $BUNDLE_ID data"
DATA="$(xcrun simctl get_app_container "$UDID" "$BUNDLE_ID" data 2>/dev/null)"
if [[ ! -d "$DATA" ]]; then
    xcrun simctl launch "$UDID" "$BUNDLE_ID" >/dev/null && sleep 3
    xcrun simctl terminate "$UDID" "$BUNDLE_ID" >/dev/null 2>&1
    CONTAINER_HOW="$CONTAINER_HOW (after one launch + terminate)"
    DATA="$(xcrun simctl get_app_container "$UDID" "$BUNDLE_ID" data)" || die "get_app_container data failed"
fi
[[ -d "$DATA" ]] || die "data container $DATA is not a directory"
TARGET_DIR="$DATA/Library/Application Support"
CONTAINER="$CONTAINER_HOW
-> $DATA
files go to: $TARGET_DIR (frust_paths::data_dir() = \$HOME/Library/Application Support)"
redact <<<"$CONTAINER"
save container.txt <<<"$CONTAINER"

step "deliver"
mkdir -p "$TARGET_DIR" || die "mkdir $TARGET_DIR failed"
for s in "${STRATEGIES[@]}"; do
    cp "$STAGE/$(file_for "$s")" "$TARGET_DIR/" || die "copy $(file_for "$s") failed"
done
# shellcheck disable=SC2012 # the listing is for the reader; names are the four fixed ones
ls -l "$TARGET_DIR" | redact

# Launch the app with `simctl launch --console-pty` (stdout + stderr) in the background, wait for
# `want` frust-probe lines (1 for one strategy, 4 for all), run vmmap on the live process, take
# a screenshot and terminate it. Writes console-<tag>.txt / vmmap-<tag>.txt.
LINES=""
launch() {
    local tag="$1" strategy="$2" want="$3" console="$WORK/console-$1.txt" pid="" got=""
    xcrun simctl terminate "$UDID" "$BUNDLE_ID" >/dev/null 2>&1
    if [[ -n "$strategy" ]]; then
        SIMCTL_CHILD_FRUST_PROBE_STRATEGY="$strategy" \
            xcrun simctl launch --console-pty "$UDID" "$BUNDLE_ID" >"$console" 2>&1 &
    else
        xcrun simctl launch --console-pty "$UDID" "$BUNDLE_ID" >"$console" 2>&1 &
    fi
    LAUNCH_PID=$!
    for _ in $(seq 1 60); do
        got="$(grep -c 'frust-probe: strategy=' "$console")"
        [[ "$got" -ge "$want" ]] && break
        kill -0 "$LAUNCH_PID" 2>/dev/null || break
        sleep 1
    done
    sleep 1
    pid="$(sed -n 's/.*frust-probe-attempt: .*pid=\([0-9]*\) .*/\1/p' "$console" | tail -1)"
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
        vmmap -w "$pid" 2>&1 | save "vmmap-$tag.txt"
        echo "-- vmmap $pid, images matching libfrust_probe:"
        vmmap -w "$pid" 2>/dev/null | grep 'libfrust_probe' | redact || echo "(none)"
    elif [[ -n "$pid" ]]; then
        echo "-- process $pid is gone after the attempt"
    fi
    [[ -n "$LOG_DIR" ]] && xcrun simctl io "$UDID" screenshot "$LOG_DIR/screen-$tag.png" >/dev/null 2>&1
    xcrun simctl terminate "$UDID" "$BUNDLE_ID" >/dev/null 2>&1
    wait "$LAUNCH_PID" 2>/dev/null
    LAUNCH_PID=""
    tr -d '\r' <"$console" | save "console-$tag.txt"
    echo "-- console ($tag), frust-probe lines:"
    tr -d '\r' <"$console" | grep 'frust-probe' | redact
    LINES+="$(tr -d '\r' <"$console" | grep 'frust-probe: strategy=' | redact)"$'\n'
}

START="$(date '+%Y-%m-%d %H:%M:%S')"
for s in "${STRATEGIES[@]}"; do
    step "launch: strategy=$s"
    launch "$s" "$s" 1
done
PER_STRATEGY="$LINES"

step "launch: all strategies with the files removed (all must report patch file missing)"
for s in "${STRATEGIES[@]}"; do rm -f "$TARGET_DIR/$(file_for "$s")"; done
launch missing "" "${#STRATEGIES[@]}"

# Host unified log: code-signing or dyld complaints about the delivered files while the probe ran.
SYSLOG="$(log show --style compact --start "$START" \
    --predicate 'eventMessage CONTAINS "libfrust_probe" AND process != "log"' 2>/dev/null | redact)"
save syslog.txt <<<"$SYSLOG"

RUNTIME="$(xcrun simctl list devices | grep -B999 "($UDID)" | sed -n 's/^-- \(.*\) --$/\1/p' | tail -1)"
DEVICE="model=$(xcrun simctl list devices | sed -n "s/^ *\(.*\) ($UDID).*/\1/p")
runtime=$RUNTIME
xcode=$(xcodebuild -version | tr '\n' ' ')
host=$(sw_vers -productName) $(sw_vers -productVersion) $(uname -m)
app=$BUNDLE_ID"

echo
echo "== device"
echo "$DEVICE"
echo "== container"
redact <<<"$CONTAINER"
echo "== frust-probe lines (one launch per strategy)"
printf '%s' "$PER_STRATEGY"
echo "== host log lines naming libfrust_probe"
if [[ -n "$SYSLOG" ]]; then tail -20 <<<"$SYSLOG"; else echo "(none)"; fi
save device.txt <<<"$DEVICE"
save probe.txt <<<"$LINES"

for s in "${STRATEGIES[@]}"; do
    grep -q "strategy=$s " <<<"$PER_STRATEGY" || die "no frust-probe line captured for strategy $s"
done
