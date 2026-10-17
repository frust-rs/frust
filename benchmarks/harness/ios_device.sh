#!/usr/bin/env bash
# benchmarks/harness/ios_device.sh — the physical-iPhone device helpers shared
# by run.sh and matrix.sh (`--platform ios`). Sourced, never executed; every
# function reads the caller's `DEVICE` (the UDID) and is best-effort: a tool
# that is missing or a reading that is unavailable prints nothing and returns
# non-zero, so the caller records "n/a" instead of failing a run.
#
# Tools (each verified on the iPhone SE / iOS 26.7.1 / Xcode 27, 2026-10-17):
#   xcrun devicectl   install / launch / terminate / process list / screenshot
#   ideviceinfo       battery level + charging (libimobiledevice, USB lockdown)
#   idevicediagnostics battery temperature (AppleSmartBattery ioreg entry)
#   idevicesyslog     the device syslog relay — FrontBoard's launch lifecycle
#                     (the external cold-start anchors, see matrix.sh)
#   xcrun xctrace     Instruments; `Activity Monitor` sysmon rows for the S7
#                     idle window. Only reachable while Xcode.app is running —
#                     see ios_xctrace_online / ios_xctrace_ensure.

# ios_pid <bundle-id> — the running pid of an installed app, by matching the
# process's executable URL against the bundle's installation URL (Xcode 26+
# devicectl has no `--pid-of <bundle>`). Empty when not running.
ios_pid() {
  local bundle="$1" apps_json procs_json pid
  apps_json="$(mktemp)"; procs_json="$(mktemp)"
  xcrun devicectl device info apps --device "${DEVICE}" --bundle-id "${bundle}" --json-output "${apps_json}" >/dev/null 2>&1 || true
  xcrun devicectl device info processes --device "${DEVICE}" --json-output "${procs_json}" >/dev/null 2>&1 || true
  pid="$(python3 - "${apps_json}" "${procs_json}" <<'PY' 2>/dev/null
import json, sys
try:
    apps = json.load(open(sys.argv[1]))["result"]["apps"]
    procs = json.load(open(sys.argv[2]))["result"]["runningProcesses"]
    url = apps[0]["url"]  # file:///private/var/containers/Bundle/Application/<UUID>/X.app/
    for p in procs:
        if p.get("executable", "").startswith(url):
            print(p["processIdentifier"])
            break
except Exception:
    pass
PY
)"
  rm -f "${apps_json}" "${procs_json}"
  [ -n "${pid}" ] && echo "${pid}"
}

# ios_terminate <bundle-id> — terminate a running instance (a not-running app
# is not an error).
ios_terminate() {
  local pid
  pid="$(ios_pid "$1")"
  [ -n "${pid}" ] || return 0
  xcrun devicectl device process terminate --device "${DEVICE}" --pid "${pid}" >/dev/null 2>&1 || true
}

# ios_battery_level — percent (`BatteryCurrentCapacity`), via lockdown.
ios_battery_level() {
  command -v ideviceinfo >/dev/null 2>&1 || return 1
  ideviceinfo -u "${DEVICE}" -q com.apple.mobile.battery 2>/dev/null | sed -n 's/^BatteryCurrentCapacity: \([0-9]*\).*/\1/p' | head -1 | grep .
}

# ios_battery_state — `charging` / `external` / `battery` from the same query.
ios_battery_state() {
  local q
  command -v ideviceinfo >/dev/null 2>&1 || return 1
  q="$(ideviceinfo -u "${DEVICE}" -q com.apple.mobile.battery 2>/dev/null)" || return 1
  [ -n "${q}" ] || return 1
  if echo "${q}" | grep -q '^BatteryIsCharging: true'; then echo charging
  elif echo "${q}" | grep -q '^ExternalConnected: true'; then echo external
  else echo battery; fi
}

# ios_battery_temp_c — the battery temperature in °C with one decimal
# (AppleSmartBattery reports centi-degrees: 3079 -> 30.8).
ios_battery_temp_c() {
  command -v idevicediagnostics >/dev/null 2>&1 || return 1
  idevicediagnostics -u "${DEVICE}" ioregentry AppleSmartBattery 2>/dev/null \
    | awk '/<key>Temperature<\/key>/ { getline; gsub(/[^0-9-]/, ""); if ($0 != "") { printf "%.1f\n", $0 / 100; found = 1 } exit } END { exit !found }'
}

# ios_screenshot <png> — one frame of the device's screen (Xcode 27 devicectl).
ios_screenshot() {
  xcrun devicectl device capture screenshot --device "${DEVICE}" --destination "$1" >/dev/null 2>&1
}

# ios_xctrace_online — whether Instruments can reach the device right now.
# xctrace lists a paired, connected iPhone under "Devices Offline" unless
# Xcode.app is running (observed with Xcode 27.0 / iOS 26.7.1).
ios_xctrace_online() {
  xcrun xctrace list devices 2>/dev/null | sed -n '/== Devices ==/,/== Devices Offline ==/p' | grep -q "${DEVICE}"
}

# ios_xctrace_ensure [<max-wait-secs>] — bring the device online for
# Instruments by opening Xcode in the background (no focus steal) when it is
# not; prints `online`/`offline`. The caller logs it as a rig fact.
ios_xctrace_ensure() {
  local max="${1:-60}" waited=0
  if ios_xctrace_online; then echo online; return 0; fi
  open -g -a Xcode 2>/dev/null || { echo offline; return 1; }
  while [ "${waited}" -lt "${max}" ]; do
    sleep 10; waited=$((waited + 10))
    if ios_xctrace_online; then echo online; return 0; fi
  done
  echo offline; return 1
}

# ios_sysmon_record <trace-path> <secs> — start an all-process Activity
# Monitor recording in the background and leave its pid in IOS_SYSMON_XPID
# for the caller to `wait` on (the Mac-side save step after the time limit
# took 1–10 min for a 15 s trace in practice). Call it in the caller's own shell, never
# inside `$(...)`: a subshell's child cannot be waited on by the parent.
# xctrace's own chatter ("Starting recording…", "Recording completed…") goes
# to <trace>.xctrace.log beside the trace, timestamped by the caller's start —
# the evidence for a recording that started late.
IOS_SYSMON_XPID=""
ios_sysmon_record() {
  local xlog="${1%.trace}.xctrace.log"
  rm -rf "$1"
  { echo "== $(date -u '+%Y-%m-%dT%H:%M:%SZ') record ${2}s -> $1"; xcrun xctrace record --template 'Activity Monitor' --device "${DEVICE}" --all-processes \
    --time-limit "$2s" --output "$1" --no-prompt 2>&1; echo "== $(date -u '+%Y-%m-%dT%H:%M:%SZ') exit $?"; } >"${xlog}" 2>&1 &
  IOS_SYSMON_XPID=$!
}

# ios_sysmon_export <trace-path> <pid> <out-file> — the bench pid's rows in
# the whitelisted `run-NN.sysmon.txt` shape (sysmon_export.py); the trace and
# the all-process XML are removed either way. Returns sysmon_export.py's
# status (3 = no rows for the pid).
ios_sysmon_export() {
  local trace="$1" pid="$2" out="$3" xml rc here
  here="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
  xml="${trace%.trace}.sysmon.xml"
  xcrun xctrace export --input "${trace}" \
    --xpath '/trace-toc/run[@number="1"]/data/table[@schema="sysmon-process"]' >"${xml}" 2>/dev/null
  python3 "${here}/sysmon_export.py" "${xml}" --pid "${pid}" --out "${out}"; rc=$?
  rm -rf "${trace}" "${xml}"
  return "${rc}"
}
