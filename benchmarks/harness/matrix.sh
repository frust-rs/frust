#!/usr/bin/env bash
# benchmarks/harness/matrix.sh — the full Frust-vs-Flutter S1–S8 matrix on ONE
# device, unattended: device-state setup, fresh installs, one `run.sh` block
# per (scenario, app) interleaved frust→flutter per scenario (PROTOCOL §3/§6),
# per-block sanity + one retry, S7 extras (idle-CPU sampling, post-matrix
# external cold start), state restore, and a sanitized copy of every raw
# artifact into a staging tree shaped exactly like `benchmarks/raw/<device>/`.
# It never writes under `benchmarks/raw/` itself — the operator moves the
# staged tree there after reviewing the numbers.
#
# Usage:
#   matrix.sh --platform android --device <serial> --device-name <slug> --out <dir>
#             --frust-apk <apk> --flutter-apk <apk>
#             [--scenarios s1,...,s8] [--runs 12] [--duration 30] [--s7-duration 60]
#             [--pin-refresh <hz>] [--min-level <pct>] [--charge-wait-max <secs>]
#             [--wireless] [--skip-install] [--dry-run]
#   --wireless   the device is reached over adb-over-Wi-Fi: airplane mode / Wi-Fi / Bluetooth are
#                left untouched (toggling them would cut the link) — record it as a deviation.
#   matrix.sh --platform ios --device <udid> --device-name <slug> --out <dir>
#             --frust-app <Runner.app> --flutter-apps-dir <dir with flutter_sN.app>
#             [--scenarios ...] [--runs 12] [--duration 30] [--s7-duration 30]
#             [--cooldown 60] [--skip-install] [--dry-run]
#
# Output tree (`<out>`):
#   progress.log            one line per event (tail this)
#   blocks.tsv              block, app, scenario, attempt, rc, wall_s, temp_before, temp_after, level, min/max lines, verdict
#   device.txt              device facts + settings read-back before/after
#   <app>/<sN>/             run.sh's own out dir: run-NN.log, run-NN.pss_*.txt, run.log (run.sh stdout), stats.txt, screencap.png, cpuinfo.txt, coldstart.txt
#   raw/<device-name>/{frust_profile,flutter}/<sN>/   sanitized copies (run-NN.log, run-NN.pss_*.txt, stats.txt)
#   RAW_OK | RAW_CHECK_FAILED, DONE | ABORTED, status.json
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
PLATFORM=android; DEVICE=""; DEVICE_NAME=""; OUT=""
FRUST_APK=""; FLUTTER_APK=""; FRUST_APP=""; FLUTTER_APPS_DIR=""
SCENARIOS="s1,s2,s3,s4,s5,s6,s7,s8"; RUNS=12; DURATION=30; S7_DURATION=""
PIN_REFRESH=""; MIN_LEVEL=0; CHARGE_WAIT_MAX=5400; COOLDOWN=60
SKIP_INSTALL=0; DRY_RUN=0; WIRELESS=0
FRUST_PKG=it.f0x.frustbench; FLUTTER_PKG=it.f0x.flutter_bench
FRUST_SCHEME=frustbench; FLUTTER_SCHEME=flutterbench
FRUST_BUNDLE=it.f0x.frustbench; FLUTTER_BUNDLE=it.f0x.flutterBench
MIN_RAW_LINES=20; MIN_PLUGIN_LINES=100

while [ $# -gt 0 ]; do
  case "$1" in
    --platform) PLATFORM="$2"; shift 2;;
    --device) DEVICE="$2"; shift 2;;
    --device-name) DEVICE_NAME="$2"; shift 2;;
    --out) OUT="$2"; shift 2;;
    --frust-apk) FRUST_APK="$2"; shift 2;;
    --flutter-apk) FLUTTER_APK="$2"; shift 2;;
    --frust-app) FRUST_APP="$2"; shift 2;;
    --flutter-apps-dir) FLUTTER_APPS_DIR="$2"; shift 2;;
    --scenarios) SCENARIOS="$2"; shift 2;;
    --runs) RUNS="$2"; shift 2;;
    --duration) DURATION="$2"; shift 2;;
    --s7-duration) S7_DURATION="$2"; shift 2;;
    --pin-refresh) PIN_REFRESH="$2"; shift 2;;
    --min-level) MIN_LEVEL="$2"; shift 2;;
    --charge-wait-max) CHARGE_WAIT_MAX="$2"; shift 2;;
    --cooldown) COOLDOWN="$2"; shift 2;;
    --wireless) WIRELESS=1; shift;;
    --skip-install) SKIP_INSTALL=1; shift;;
    --dry-run) DRY_RUN=1; shift;;
    -h|--help) sed -n '2,30p' "$0"; exit 0;;
    *) echo "error: unknown argument $1" >&2; exit 2;;
  esac
done
[ -n "$DEVICE" ] && [ -n "$DEVICE_NAME" ] && [ -n "$OUT" ] || { echo "error: --device, --device-name and --out are required" >&2; exit 2; }
[[ "$DEVICE_NAME" =~ ^[a-z0-9_]+$ ]] || { echo "error: --device-name must match [a-z0-9_]+" >&2; exit 2; }
case "$PLATFORM" in android|ios) ;; *) echo "error: --platform android|ios" >&2; exit 2;; esac
if [ -z "$S7_DURATION" ]; then if [ "$PLATFORM" = android ]; then S7_DURATION=60; else S7_DURATION=30; fi; fi
if [ "$PLATFORM" = android ] && [ "$SKIP_INSTALL" = 0 ]; then
  [ -f "$FRUST_APK" ] && [ -f "$FLUTTER_APK" ] || { echo "error: --frust-apk/--flutter-apk must exist (or pass --skip-install)" >&2; exit 2; }
fi
if [ "$PLATFORM" = ios ] && [ "$SKIP_INSTALL" = 0 ]; then
  [ -d "$FRUST_APP" ] && [ -d "$FLUTTER_APPS_DIR" ] || { echo "error: --frust-app/--flutter-apps-dir must exist (or pass --skip-install)" >&2; exit 2; }
fi
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd -P)"
PROGRESS="$OUT/progress.log"; BLOCKS="$OUT/blocks.tsv"; DEVFILE="$OUT/device.txt"
STAGE_RAW="$OUT/raw/$DEVICE_NAME"
ts() { date -u '+%Y-%m-%dT%H:%M:%SZ'; }
log() { echo "$(ts) $*" | tee -a "$PROGRESS"; }
run_or_echo() { if [ "$DRY_RUN" = 1 ]; then echo "DRY: $*"; else "$@"; fi; }
DONE_OK=0

# ---------------------------------------------------------------- android helpers
ash() { adb -s "$DEVICE" shell "$@" 2>/dev/null | tr -d '\r'; }
a_get() { ash settings get "$1" "$2"; }
a_put() { [ "$DRY_RUN" = 1 ] && { echo "DRY: settings put $1 $2 $3"; return 0; }; ash settings put "$1" "$2" "$3" >/dev/null; }
a_del() { [ "$DRY_RUN" = 1 ] && { echo "DRY: settings delete $1 $2"; return 0; }; ash settings delete "$1" "$2" >/dev/null; }
a_temp() { ash dumpsys battery | sed -n 's/^[[:space:]]*temperature: \([0-9-]*\).*/\1/p' | head -1 | awk '{printf "%.1f", $1/10}'; }
a_level() { ash dumpsys battery | sed -n 's/^[[:space:]]*level: \([0-9]*\).*/\1/p' | head -1; }
a_wake() {
  [ "$DRY_RUN" = 1 ] && return 0
  local w; w="$(ash dumpsys power | sed -n 's/.*mWakefulness=\([A-Za-z]*\).*/\1/p' | head -1)"
  [ "$w" = "Awake" ] || ash input keyevent KEYCODE_WAKEUP >/dev/null
  sleep 1
  if ash dumpsys activity activities | grep -q 'mKeyguardShowing=true'; then ash input keyevent KEYCODE_MENU >/dev/null; sleep 1; fi
  ash svc power stayon true >/dev/null
}
# Saved state (restored on exit)
SAVED_AIRPLANE=""; SAVED_WIFI=""; SAVED_BT=""; SAVED_LOWPOWER=""; SAVED_LPTRIG=""; SAVED_PEAK=""; SAVED_MIN=""; SAVED_TIMEOUT=""; SAVED_BRIGHT_MODE=""; SAVED_BRIGHT=""
android_record_device() {
  {
    echo "== device facts $(ts) =="
    echo "model=$(ash getprop ro.product.model) device=$(ash getprop ro.product.device) android=$(ash getprop ro.build.version.release) sdk=$(ash getprop ro.build.version.sdk)"
    echo "build=$(ash getprop ro.build.display.id) fingerprint=$(ash getprop ro.build.fingerprint)"
    echo "hardware=$(ash getprop ro.hardware) board=$(ash getprop ro.board.platform) gpu=$(ash dumpsys SurfaceFlinger | grep -m1 -i 'GLES:' )"
    ash dumpsys display | grep -E 'DisplayMode\{|mActiveSfDisplayMode' | head -6
    echo "battery: $(ash dumpsys battery | grep -E 'level|temperature|USB powered|AC powered|status' | tr '\n' ' ')"
    echo "settings: airplane=$(a_get global airplane_mode_on) wifi_on=$(a_get global wifi_on) bt_on=$(a_get global bluetooth_on) low_power=$(a_get global low_power) low_power_trigger=$(a_get global low_power_trigger_level) peak_refresh=$(a_get system peak_refresh_rate) min_refresh=$(a_get system min_refresh_rate) brightness=$(a_get system screen_brightness) brightness_mode=$(a_get system screen_brightness_mode) screen_off_timeout=$(a_get system screen_off_timeout)"
    echo "packages: $(ash pm list packages | grep -E 'frustbench|flutter_bench' | tr '\n' ' ')"
  } >>"$DEVFILE"
}
android_setup() {
  SAVED_AIRPLANE="$(a_get global airplane_mode_on)"; SAVED_WIFI="$(a_get global wifi_on)"; SAVED_BT="$(a_get global bluetooth_on)"
  SAVED_LOWPOWER="$(a_get global low_power)"; SAVED_LPTRIG="$(a_get global low_power_trigger_level)"
  SAVED_PEAK="$(a_get system peak_refresh_rate)"; SAVED_MIN="$(a_get system min_refresh_rate)"; SAVED_TIMEOUT="$(a_get system screen_off_timeout)"
  SAVED_BRIGHT_MODE="$(a_get system screen_brightness_mode)"; SAVED_BRIGHT="$(a_get system screen_brightness)"
  log "saved state: airplane=$SAVED_AIRPLANE wifi=$SAVED_WIFI bt=$SAVED_BT low_power=$SAVED_LOWPOWER trig=$SAVED_LPTRIG peak=$SAVED_PEAK min=$SAVED_MIN timeout=$SAVED_TIMEOUT bright=$SAVED_BRIGHT/$SAVED_BRIGHT_MODE"
  if [ "$DRY_RUN" = 0 ]; then
    if [ "$WIRELESS" = 1 ]; then log "WIRELESS: airplane/Wi-Fi/Bluetooth left as found (adb-over-Wi-Fi link)"; else
      ash cmd connectivity airplane-mode enable >/dev/null || a_put global airplane_mode_on 1
      ash svc wifi disable >/dev/null; ash svc bluetooth disable >/dev/null
    fi
    a_put global low_power 0; a_put global low_power_trigger_level 0
    a_put system screen_off_timeout 1800000
    if [ -n "$PIN_REFRESH" ]; then a_put system min_refresh_rate "$PIN_REFRESH.0"; a_put system peak_refresh_rate "$PIN_REFRESH.0"; fi
    a_wake
    sleep 2
    log "state after setup: airplane=$(a_get global airplane_mode_on) wifi_on=$(a_get global wifi_on) bt_on=$(a_get global bluetooth_on) low_power=$(a_get global low_power) peak=$(a_get system peak_refresh_rate) min=$(a_get system min_refresh_rate) timeout=$(a_get system screen_off_timeout)"
  fi
}
android_restore() {
  [ "$DRY_RUN" = 1 ] && return 0
  ash dumpsys battery reset >/dev/null; ash svc power stayon false >/dev/null
  if [ -n "$PIN_REFRESH" ]; then
    if [ "$SAVED_MIN" = "null" ] || [ -z "$SAVED_MIN" ]; then a_del system min_refresh_rate; else a_put system min_refresh_rate "$SAVED_MIN"; fi
    if [ "$SAVED_PEAK" = "null" ] || [ -z "$SAVED_PEAK" ]; then a_del system peak_refresh_rate; else a_put system peak_refresh_rate "$SAVED_PEAK"; fi
  fi
  [ "$SAVED_TIMEOUT" = "null" ] || [ -z "$SAVED_TIMEOUT" ] || a_put system screen_off_timeout "$SAVED_TIMEOUT"
  [ "$SAVED_LOWPOWER" = "null" ] || [ -z "$SAVED_LOWPOWER" ] || a_put global low_power "$SAVED_LOWPOWER"
  [ "$SAVED_LPTRIG" = "null" ] || [ -z "$SAVED_LPTRIG" ] || a_put global low_power_trigger_level "$SAVED_LPTRIG"
  [ "$SAVED_BRIGHT_MODE" = "null" ] || [ -z "$SAVED_BRIGHT_MODE" ] || a_put system screen_brightness_mode "$SAVED_BRIGHT_MODE"
  [ "$SAVED_BRIGHT" = "null" ] || [ -z "$SAVED_BRIGHT" ] || a_put system screen_brightness "$SAVED_BRIGHT"
  if [ "$WIRELESS" = 0 ]; then
    if [ "$SAVED_AIRPLANE" = "0" ]; then ash cmd connectivity airplane-mode disable >/dev/null || a_put global airplane_mode_on 0; fi
    [ "$SAVED_WIFI" = "1" ] && ash svc wifi enable >/dev/null
    [ "$SAVED_BT" = "1" ] && ash svc bluetooth enable >/dev/null
  fi
  log "state restored: airplane=$(a_get global airplane_mode_on) wifi_on=$(a_get global wifi_on) bt_on=$(a_get global bluetooth_on) low_power=$(a_get global low_power) peak=$(a_get system peak_refresh_rate) min=$(a_get system min_refresh_rate) timeout=$(a_get system screen_off_timeout) battery=$(ash dumpsys battery | grep -E 'USB powered|level' | tr '\n' ' ')"
  android_record_device
}
android_install() {
  [ "$SKIP_INSTALL" = 1 ] && { log "install skipped (--skip-install)"; return 0; }
  for p in "$FRUST_PKG" "$FLUTTER_PKG"; do run_or_echo adb -s "$DEVICE" uninstall "$p" >/dev/null 2>&1 || true; done
  run_or_echo adb -s "$DEVICE" install -r "$FRUST_APK" >/dev/null || { log "FATAL: frust apk install failed"; exit 1; }
  run_or_echo adb -s "$DEVICE" install -r "$FLUTTER_APK" >/dev/null || { log "FATAL: flutter apk install failed"; exit 1; }
  [ "$DRY_RUN" = 1 ] || log "installed: $(ash pm list packages | grep -E 'frustbench|flutter_bench' | tr '\n' ' ') frust_md5=$(md5 -q "$FRUST_APK") flutter_md5=$(md5 -q "$FLUTTER_APK")"
}
android_battery_guard() {
  [ "$MIN_LEVEL" -gt 0 ] || return 0
  [ "$DRY_RUN" = 1 ] && return 0
  ash dumpsys battery reset >/dev/null; sleep 2
  local waited=0 lvl
  while true; do
    lvl="$(a_level)"; [ -n "$lvl" ] || return 0
    [ "$lvl" -ge "$MIN_LEVEL" ] && { log "battery guard: level=$lvl >= $MIN_LEVEL, proceeding"; return 0; }
    [ "$waited" -ge "$CHARGE_WAIT_MAX" ] && { log "battery guard: level=$lvl still < $MIN_LEVEL after ${waited}s — proceeding anyway"; return 0; }
    log "battery guard: level=$lvl < $MIN_LEVEL — waiting 120s to charge (${waited}/${CHARGE_WAIT_MAX}s)"
    ash svc power stayon false >/dev/null   # screen off while charging
    sleep 120; waited=$((waited+120))
  done
}
android_screencap_bg() { # $1 pkg, $2 out png — waits for the app to be foreground, then grabs a frame
  ( for _ in $(seq 1 90); do if ash dumpsys window 2>/dev/null | grep -E 'mCurrentFocus|mFocusedApp' | grep -q "$1"; then sleep 8; adb -s "$DEVICE" exec-out screencap -p >"$2" 2>/dev/null; exit 0; fi; sleep 2; done ) >/dev/null 2>&1 </dev/null &
}
SAMPLER_PID=""
android_cpu_sampler_bg() { # $1 pkg, $2 out file -> sets SAMPLER_PID (never call inside $(...): the
  # background subshell would hold the substitution's pipe open and block the caller forever)
  ( while true; do echo "== $(ts)"; ash dumpsys cpuinfo | grep -E "$1|TOTAL" ; sleep 30; done ) >>"$2" 2>/dev/null </dev/null &
  SAMPLER_PID=$!
}
android_coldstart() { # $1 app
  local pkg scheme out n
  if [ "$1" = frust ]; then pkg=$FRUST_PKG; scheme=$FRUST_SCHEME; else pkg=$FLUTTER_PKG; scheme=$FLUTTER_SCHEME; fi
  out="$OUT/$1/s7/coldstart.txt"; mkdir -p "$OUT/$1/s7"
  a_wake
  for n in 1 2 3 4; do
    ash am force-stop "$pkg" >/dev/null; sleep 3
    echo "== launch $n $(ts)" >>"$out"
    ash am start -W -a android.intent.action.VIEW -d "$scheme://s7" "$pkg" >>"$out"
    sleep 5
  done
  ash am force-stop "$pkg" >/dev/null
  log "cold start $1: $(grep -oE 'TotalTime: [0-9]+' "$out" | tr '\n' ' ')"
}

# ---------------------------------------------------------------- ios helpers
ios_record_device() {
  { echo "== device facts $(ts) =="; xcrun devicectl device info details --device "$DEVICE" 2>/dev/null | grep -E 'marketingName|osVersionNumber|productType|name:|developerModeStatus|cpuType' ; } >>"$DEVFILE"
}

# ---------------------------------------------------------------- block runner
count_lines() { local n; n="$(grep -c -E "$1" "$2" 2>/dev/null | head -1)"; echo "${n:-0}"; }
block_sanity() { # $1 scenario, $2 dir -> prints "min-max" line counts over kept logs; returns 1 if degenerate
  local s="$1" d="$2" pat min= max= n i log lines bad=0
  case "$s" in s8) pat='-perf plugin( scenario=[a-z0-9-]+)? op='; ;; *) pat='-perf raw'; ;; esac
  i=0
  for log in "$d"/run-[0-9][0-9].log; do
    i=$((i+1)); [ "$i" -le 2 ] && continue   # first two are warm-up (PROTOCOL §4)
    lines="$(count_lines "$pat" "$log")"
    [ -z "$min" ] || [ "$lines" -lt "$min" ] && min=$lines
    [ -z "$max" ] || [ "$lines" -gt "$max" ] && max=$lines
    grep -Eq "bench-scenario-start( n=[0-9]+)? $s" "$log" || bad=1
    case "$s" in
      s7) grep -q -E '(frust|flutter)-perf startup' "$log" || bad=1 ;;
      s8) [ "$lines" -ge "$MIN_PLUGIN_LINES" ] || bad=1 ;;
      *)  [ "$lines" -ge "$MIN_RAW_LINES" ] || bad=1 ;;
    esac
  done
  echo "${min:-0}-${max:-0}"
  return $bad
}
run_block() { # $1 app, $2 scenario, $3 attempt -> rc
  local app="$1" s="$2" attempt="$3" dir dur rc t0 t1 tb ta lvl sampler="" extra=() lines verdict
  dir="$OUT/$app/$s"; [ "$attempt" -gt 1 ] && dir="$OUT/$app/$s.retry$((attempt-1))"
  if [ "$s" = s7 ]; then dur=$S7_DURATION; else dur=$DURATION; fi
  mkdir -p "$dir"
  tb=""; ta=""; lvl=""
  if [ "$PLATFORM" = android ]; then
    android_battery_guard
    [ "$DRY_RUN" = 1 ] || { ash dumpsys battery unplug >/dev/null; a_wake; }
    tb="$(a_temp)"; lvl="$(a_level)"
    local pkg; if [ "$app" = frust ]; then pkg=$FRUST_PKG; else pkg=$FLUTTER_PKG; fi
    extra=(--pkg "$pkg")
    [ "$DRY_RUN" = 1 ] || android_screencap_bg "$pkg" "$dir/screencap.png"
    if [ "$s" = s7 ] && [ "$DRY_RUN" = 0 ]; then android_cpu_sampler_bg "$pkg" "$dir/cpuinfo.txt"; sampler="$SAMPLER_PID"; fi
  else
    extra=(--platform ios)
    if [ "$app" = frust ] && [ "$FRUST_INSTALLED" = 0 ] && [ "$SKIP_INSTALL" = 0 ]; then extra+=(--install "$FRUST_APP"); FRUST_INSTALLED=1; fi
    if [ "$app" = flutter ] && [ "$SKIP_INSTALL" = 0 ]; then extra+=(--install "$FLUTTER_APPS_DIR/flutter_$s.app"); fi
  fi
  log "BLOCK START app=$app scenario=$s attempt=$attempt runs=$RUNS duration=${dur}s temp=${tb:-n/a} level=${lvl:-n/a}"
  t0=$(date +%s)
  if [ "$DRY_RUN" = 1 ]; then
    echo "DRY: run.sh $s --app $app --device $DEVICE --runs $RUNS --duration $dur ${extra[*]} --out $dir"; rc=0
  else
    "$SCRIPT_DIR/run.sh" "$s" --app "$app" --device "$DEVICE" --runs "$RUNS" --duration "$dur" "${extra[@]}" --out "$dir" >"$dir/run.log" 2>&1; rc=$?
    sed -n '/^-- stats\.py/,$p' "$dir/run.log" >"$dir/stats.txt"
  fi
  t1=$(date +%s)
  [ -n "$sampler" ] && kill "$sampler" 2>/dev/null
  if [ "$PLATFORM" = android ] && [ "$DRY_RUN" = 0 ]; then ta="$(a_temp)"; fi
  lines="n/a"; verdict="ok"
  if [ "$DRY_RUN" = 0 ]; then
    if lines="$(block_sanity "$s" "$dir")"; then verdict="ok"; else verdict="DEGENERATE"; fi
    [ "$rc" -eq 0 ] || verdict="RC$rc"
  fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$(ts)" "$app" "$s" "$attempt" "$rc" "$((t1-t0))" "${tb:-}" "${ta:-}" "${lvl:-}" "$lines" "$verdict" >>"$BLOCKS"
  log "BLOCK DONE app=$app scenario=$s attempt=$attempt rc=$rc wall=$((t1-t0))s temp=${tb:-n/a}->${ta:-n/a} kept_lines=$lines verdict=$verdict $( [ -f "$dir/stats.txt" ] && grep -E '^p50=' "$dir/stats.txt" | head -1 )"
  [ "$verdict" = ok ]
}

# ---------------------------------------------------------------- main
trap 'rc=$?; if [ "$DONE_OK" = 0 ]; then log "ABORTED (exit $rc)"; touch "$OUT/ABORTED"; fi; if [ "$PLATFORM" = android ]; then android_restore; fi' EXIT
printf 'ts\tapp\tscenario\tattempt\trc\twall_s\ttemp_before\ttemp_after\tlevel\tkept_lines_min_max\tverdict\n' >"$BLOCKS"
log "matrix start platform=$PLATFORM device=$DEVICE name=$DEVICE_NAME scenarios=$SCENARIOS runs=$RUNS duration=$DURATION s7=$S7_DURATION pin=${PIN_REFRESH:-none} min_level=$MIN_LEVEL wireless=$WIRELESS out=$OUT dry=$DRY_RUN"
FRUST_INSTALLED=0
if [ "$PLATFORM" = android ]; then
  adb -s "$DEVICE" get-state >/dev/null 2>&1 || { log "FATAL: device $DEVICE unreachable"; exit 1; }
  android_record_device; android_setup; android_install
else
  xcrun devicectl device info details --device "$DEVICE" >/dev/null 2>&1 || { log "FATAL: iOS device $DEVICE unreachable"; exit 1; }
  ios_record_device
fi
FAILED_BLOCKS=""
first=1
for s in $(echo "$SCENARIOS" | tr ',' ' '); do
  for app in frust flutter; do
    if [ "$PLATFORM" = ios ] && [ "$first" = 0 ] && [ "$DRY_RUN" = 0 ]; then log "cooldown ${COOLDOWN}s"; sleep "$COOLDOWN"; fi
    first=0
    if ! run_block "$app" "$s" 1; then
      log "RETRY app=$app scenario=$s"
      [ "$PLATFORM" = ios ] && [ "$DRY_RUN" = 0 ] && sleep "$COOLDOWN"
      run_block "$app" "$s" 2 || FAILED_BLOCKS="$FAILED_BLOCKS $app/$s"
    fi
  done
done
if [ "$PLATFORM" = android ] && [ "$DRY_RUN" = 0 ] && echo ",$SCENARIOS," | grep -q ',s7,'; then
  ash dumpsys battery unplug >/dev/null
  android_coldstart frust; android_coldstart flutter
fi

# ---------------------------------------------------------------- raw staging (sanitized copies only)
pick_src() { # $1 app, $2 scenario -> the attempt dir whose block verdict was ok (prefer attempt 1), else attempt 1
  local v1 v2
  v1="$(awk -F'\t' -v a="$1" -v s="$2" '$2==a && $3==s && $4=="1" {print $NF}' "$BLOCKS" | tail -1)"
  v2="$(awk -F'\t' -v a="$1" -v s="$2" '$2==a && $3==s && $4=="2" {print $NF}' "$BLOCKS" | tail -1)"
  if [ "$v1" = ok ]; then echo "$OUT/$1/$2"; elif [ "$v2" = ok ]; then echo "$OUT/$1/$2.retry1"; else echo "$OUT/$1/$2"; fi
}
stage_ok=1
if [ "$DRY_RUN" = 0 ]; then
  rm -rf "$STAGE_RAW"
  for s in $(echo "$SCENARIOS" | tr ',' ' '); do
    for app in frust flutter; do
      src="$(pick_src "$app" "$s")"
      [ -d "$src" ] || continue
      if [ "$app" = frust ]; then dst="$STAGE_RAW/frust_profile/$s"; else dst="$STAGE_RAW/flutter/$s"; fi
      mkdir -p "$dst"
      cp "$src"/run-[0-9][0-9].log "$dst"/ || stage_ok=0
      ls "$src"/run-[0-9][0-9].pss_*.txt >/dev/null 2>&1 && cp "$src"/run-[0-9][0-9].pss_*.txt "$dst"/
      [ -f "$src/stats.txt" ] && cp "$src/stats.txt" "$dst/stats.txt"
      [ -f "$src/coldstart.txt" ] && cp "$src/coldstart.txt" "$dst/coldstart.txt"
      [ -f "$src/cpuinfo.txt" ] && cp "$src/cpuinfo.txt" "$dst/cpuinfo.txt"
      n="$(ls "$dst"/run-[0-9][0-9].log 2>/dev/null | wc -l | tr -d ' ')"; [ "$n" -eq "$RUNS" ] || { log "stage: $dst has $n logs, expected $RUNS"; stage_ok=0; }
    done
  done
  # self-check, same rules as benchmarks/.gitignore's pre-commit block
  if grep -rlvE 'frust-perf|flutter-perf|bench-scenario|^[[:space:]]*$' "$STAGE_RAW" --include='*.log' | grep -q .; then log "stage: UNSANITIZED .log lines found"; stage_ok=0; fi
  if grep -rlE 'package|Application' "$STAGE_RAW" --include='*.pss_*.txt' | xargs -I{} grep -HoE '\b[a-z]+(\.[a-z_]+){2,}\b' {} 2>/dev/null | grep -vE 'it\.f0x\.[a-z_]*bench|android\.(app|os|view)|java\.' | grep -q .; then log "stage: PSS snapshot names a foreign package (inspect before committing)"; fi
  if grep -rlvE '^-- stats\.py( --dclass)? \(discarding first [0-9]+ runs?, per protocol convention\) --$|^== [A-Za-z0-9_. ()-]+ ==$|^frames: [0-9]+ total, [0-9]+ active, [0-9]+ skipped$|^p50=[0-9.]+ms[[:space:]]+p95=[0-9.]+ms[[:space:]]+p99=[0-9.]+ms[[:space:]]+worst=[0-9.]+ms$|^missed_60hz=[0-9]+ \(budget [0-9.]+ms\)[[:space:]]+missed_120hz=[0-9]+ \(budget [0-9.]+ms\)$|^[[:space:]]*$' "$STAGE_RAW" --include='stats.txt' | grep -q .; then log "stage: stats.txt outside whitelist"; stage_ok=0; fi
  if [ "$stage_ok" = 1 ]; then touch "$OUT/RAW_OK"; log "raw staged + self-check passed: $STAGE_RAW"; else touch "$OUT/RAW_CHECK_FAILED"; log "raw staged with CHECK FAILURES: $STAGE_RAW"; fi
fi
printf '{"platform":"%s","device":"%s","device_name":"%s","runs":%s,"duration":%s,"s7_duration":%s,"failed_blocks":"%s","raw_ok":%s,"finished":"%s"}\n' "$PLATFORM" "$DEVICE" "$DEVICE_NAME" "$RUNS" "$DURATION" "$S7_DURATION" "${FAILED_BLOCKS# }" "$stage_ok" "$(ts)" >"$OUT/status.json"
DONE_OK=1
touch "$OUT/DONE"
log "matrix DONE failed_blocks='${FAILED_BLOCKS# }' raw_ok=$stage_ok"
