#!/usr/bin/env bash
# benchmarks/harness/device_state.sh — Android device-state fairness gate.
#
# Sets/checks the protocol's fairness preconditions (see
# benchmarks/PROTOCOL.md) on an attached Android device before a benchmark
# run: fixes brightness (no auto-brightness drift mid-run), checks
# airplane-mode/charger state (warns rather than forcing — see below), and
# gates on a thermal cooldown via `dumpsys battery` temperature so a hot
# device from a previous run doesn't bias the next one.
#
# Usage: device_state.sh --device <serial> [--brightness <0-255>]
#                         [--max-temp-c <n>] [--cooldown-timeout <secs>]
#                         [--cooldown-poll <secs>]
#
#   --device <serial>       adb device serial (required; see `adb devices`).
#   --brightness <0-255>    fixed screen brightness to set (default: 128).
#   --max-temp-c <n>        thermal cooldown ceiling in whole Celsius
#                           (default: 38).
#   --cooldown-timeout <s>  give up waiting for cooldown after this many
#                           seconds and continue anyway, with a warning
#                           (default: 120).
#   --cooldown-poll <s>     seconds between temperature polls while waiting
#                           (default: 5).
#
# Exit status: non-zero only when `adb` is missing or the device itself is
# unreachable (get-state fails) — every other precondition (airplane mode,
# charger, thermal ceiling) degrades to a printed warning rather than a
# failure, since this script cannot force a charger unplug or an airplane
# toggle a human didn't already do, and refusing to run at all over a
# borderline temperature would make the harness less useful, not more
# rigorous. `run.sh` is what decides whether a warning here should stop a
# run.

set -uo pipefail

DEVICE=""
BRIGHTNESS=128
MAX_TEMP_C=38
COOLDOWN_TIMEOUT=120
COOLDOWN_POLL=5

usage() {
  sed -n '2,32p' "$0"
}

while [ $# -gt 0 ]; do
  case "$1" in
    --device)
      [ $# -ge 2 ] || { echo "error: --device requires a serial argument" >&2; exit 2; }
      DEVICE="$2"
      shift 2
      ;;
    --device=*)
      DEVICE="${1#--device=}"
      shift
      ;;
    --brightness)
      [ $# -ge 2 ] || { echo "error: --brightness requires a value" >&2; exit 2; }
      BRIGHTNESS="$2"
      shift 2
      ;;
    --brightness=*)
      BRIGHTNESS="${1#--brightness=}"
      shift
      ;;
    --max-temp-c)
      [ $# -ge 2 ] || { echo "error: --max-temp-c requires a value" >&2; exit 2; }
      MAX_TEMP_C="$2"
      shift 2
      ;;
    --max-temp-c=*)
      MAX_TEMP_C="${1#--max-temp-c=}"
      shift
      ;;
    --cooldown-timeout)
      [ $# -ge 2 ] || { echo "error: --cooldown-timeout requires a value" >&2; exit 2; }
      COOLDOWN_TIMEOUT="$2"
      shift 2
      ;;
    --cooldown-timeout=*)
      COOLDOWN_TIMEOUT="${1#--cooldown-timeout=}"
      shift
      ;;
    --cooldown-poll)
      [ $# -ge 2 ] || { echo "error: --cooldown-poll requires a value" >&2; exit 2; }
      COOLDOWN_POLL="$2"
      shift 2
      ;;
    --cooldown-poll=*)
      COOLDOWN_POLL="${1#--cooldown-poll=}"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unrecognized argument '$1'" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [ -z "${DEVICE}" ]; then
  echo "error: --device <serial> is required" >&2
  usage >&2
  exit 2
fi

if ! command -v adb >/dev/null 2>&1; then
  echo "error: adb not found on PATH — Android SDK platform-tools required" >&2
  exit 1
fi

adb_shell() {
  adb -s "${DEVICE}" shell "$@"
}

if ! adb -s "${DEVICE}" get-state >/dev/null 2>&1; then
  echo "error: device '${DEVICE}' not reachable (adb get-state failed) — check \`adb devices\`" >&2
  exit 1
fi

echo "== device_state: ${DEVICE} =="

# --- Brightness: fixed value, auto-brightness disabled ----------------

echo "-- Brightness --"
if adb_shell settings put system screen_brightness_mode 0 >/dev/null 2>&1 \
  && adb_shell settings put system screen_brightness "${BRIGHTNESS}" >/dev/null 2>&1; then
  echo "Set fixed brightness=${BRIGHTNESS}, auto-brightness disabled."
else
  echo "warning: could not set brightness via \`settings put system\` — set it manually before running" >&2
fi

# --- Airplane mode: check only, never force (see header) ---------------

echo "-- Airplane mode --"
AIRPLANE_STATE="$(adb_shell settings get global airplane_mode_on 2>/dev/null | tr -d '\r\n ')"
case "${AIRPLANE_STATE}" in
  1)
    echo "Airplane mode: on."
    ;;
  0)
    echo "warning: airplane mode is OFF — protocol requires it on for a fairness run (radio wake/traffic can steal CPU/thermal headroom); enable it manually" >&2
    ;;
  *)
    echo "warning: could not read airplane_mode_on (got '${AIRPLANE_STATE}') — verify manually" >&2
    ;;
esac

# --- Charger: check only, never force ---------------------------------

echo "-- Charger --"
BATTERY_DUMP="$(adb_shell dumpsys battery 2>/dev/null)"
if printf '%s\n' "${BATTERY_DUMP}" | grep -Eq 'AC powered: true|USB powered: true|Wireless powered: true'; then
  echo "warning: device appears to be charging — protocol requires charger off (charging affects CPU/thermal boost behavior); unplug it" >&2
else
  echo "Charger: off (or undetermined-but-not-flagged-charging)."
fi

# --- Thermal cooldown gate ----------------------------------------------

echo "-- Thermal cooldown (ceiling ${MAX_TEMP_C}C, timeout ${COOLDOWN_TIMEOUT}s) --"

read_temp_c() {
  # `dumpsys battery`'s `temperature` field is tenths of a degree Celsius
  # (e.g. "350" -> 35.0C) — Android's documented BatteryManager convention.
  raw="$(adb_shell dumpsys battery 2>/dev/null | sed -n 's/^[[:space:]]*temperature: \([0-9-]*\).*/\1/p' | head -n1 | tr -d '\r')"
  if [ -z "${raw}" ]; then
    return 1
  fi
  awk -v t="${raw}" 'BEGIN { printf "%.1f", t / 10.0 }'
}

waited=0
while true; do
  temp_c="$(read_temp_c || true)"
  if [ -z "${temp_c}" ]; then
    echo "warning: could not read battery temperature via dumpsys — skipping thermal gate" >&2
    break
  fi
  # Integer-truncated compare (awk avoids a bash-only float-compare dependency).
  over="$(awk -v t="${temp_c}" -v m="${MAX_TEMP_C}" 'BEGIN { print (t > m) ? "1" : "0" }')"
  if [ "${over}" = "0" ]; then
    echo "Temperature ${temp_c}C at or below ceiling ${MAX_TEMP_C}C — proceeding."
    break
  fi
  if [ "${waited}" -ge "${COOLDOWN_TIMEOUT}" ]; then
    echo "warning: still ${temp_c}C after ${waited}s wait (ceiling ${MAX_TEMP_C}C) — timed out, proceeding anyway" >&2
    break
  fi
  echo "Temperature ${temp_c}C above ceiling ${MAX_TEMP_C}C — waiting ${COOLDOWN_POLL}s (${waited}/${COOLDOWN_TIMEOUT}s elapsed)..."
  sleep "${COOLDOWN_POLL}"
  waited=$((waited + COOLDOWN_POLL))
done

echo "== device_state: ready =="
