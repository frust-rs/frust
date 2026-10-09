#!/usr/bin/env bash
# examples/hotpatch-spike/measure.sh — edit-to-frame timing for the hot-patch spike.
#
# Launches the spike app, waits for its first `frust-hotpatch: frame` line, then per run edits one
# source file IN PLACE, stamps the save time (unix ms) and waits for the app's next
# `frust-hotpatch: applied` / `frust-hotpatch: frame` lines stamped later than the save. Prints
# the per-run deltas and their medians. Results are recorded by hand in RESULTS.md.
#
# BASELINE: from the tip that added `PatchError::AnchorMismatch` onward, the runtime refuses dx
# 0.7.10's jump tables: dx anchors both table addresses on `main`, not on `__frust_hotpatch_anchor`,
# so the offsets they imply disagree with the images' slides. The runner then prints
# `frust-hotpatch: refused AnchorMismatch`, and this script records the run as "refused (anchor
# mismatch)", distinct from applied and from a silent no-op. Rows D-D3 of RESULTS.md reproduce only
# against the runtime at 1c29ba1d (before that check), which rebased by the wrong constant.
#
# Usage: examples/hotpatch-spike/measure.sh [--hotpatch | --restart | --frust-run | --frust-restart]
#                                           [--app <dir>] [--runs <n>] [--target <t>] [--row <label>]
#                                           [--serial <adb-serial> | -d <adb-serial> | --android]
#                                           [--windows <ssh-alias>]
#        examples/hotpatch-spike/measure.sh --prepare-app <dir> [--windows <ssh-alias>]
#   --hotpatch     (default) `dx serve --hot-patch --platform desktop --interactive false --verbose`
#                  from runner/; DX from $DX, else `dx` on PATH (must be dioxus-cli 0.7.10).
#   --restart      Baseline: `cargo run -p hotpatch-spike`, killed and relaunched by this script
#                  after each edit (`frust run --watch` rejects this two-package workspace: see
#                  README.md). No `applied` line exists here; only the frame delta is recorded.
#   --frust-run    Milestone 1 (RESULTS.md): `frust run --watch` (hot by default for a debug desktop
#                  run) from --app, a scratch `frust create` app outside this repo. FRUST=<path>
#                  names the frust binary (default: `frust` on PATH); FRUST_RUN_ARGS is appended to
#                  `frust run --watch` word by word. Every output line is logged as
#                  `<arrival unix ms> <line>`, so the CLI's own `patched in N ms (k components
#                  rebuilt)` / `restart required: <reason>` lines are timed beside the app's probe
#                  lines. A `restart required` run waits for the relaunched app's first frame and
#                  follows its new PID; each run reports the app's RSS (`ps -o rss=`).
#   --frust-restart  Row F baseline: `frust run --watch --no-hot --features frust/hotpatch` from
#                  --app (the CLI's own kill + `cargo run` relaunch; the feature only turns on the
#                  first-frame probe line). Only the frame delta of the relaunched app is recorded.
#   --app <dir>    The scratch app for --frust-run / --frust-restart (required by both).
#   --prepare-app <dir>  Rewrites a freshly generated `frust create` app at <dir> into the measured
#                  layout and exits: `src/home_page.rs` keeps the template's scaffold, app bar, FAB
#                  and label but gets a named `HomeState`, the markers below and a `CounterCard`
#                  child (`src/counter_card.rs`, new); `src/lib.rs` gains `mod counter_card;` and its
#                  root `build` hosts `HomePage` in one arm of `either(home_page::SHOW_HOME, ..)`.
#                  The package, its crate types, `main.rs` and the `frust::app!` line stay generated.
#   --runs <n>     Edits to measure (default: 5).
#   --row <label>  A RESULTS.md row label (e.g. `A`, `D3gm`) printed on the header and summary lines,
#                  so a saved transcript names its row. It changes nothing that runs.
#   --target <t>   What each run edits (default: home):
#                    home         bump `hotpatch-sentinel: vN` (app/src/home_page.rs, // SENTINEL-HOME)
#                    card         bump `card-sentinel: vN` (app/src/counter_card.rs, // SENTINEL-CARD)
#                    helper       add a new private fn called from HomePage::build
#                    state-type   swap HomePage's `State` (the named `HomeState`) for an (N+1)-tuple
#                                 of u32: a type-IDENTITY change (RESULTS.md row D, a silent no-op)
#                    state-field  add `extra: u32` to `struct HomeState` (same type identity, new
#                                 layout; RESULTS.md row D2). The patched build reads it into the
#                                 sentinel and logs `frust-hotpatch-spike: state-field vN build ran
#                                 extra=<v>`, so each run records whether the new build ran, the
#                                 value it read, and whether the app PID survived.
#                    return-type  wrap HomePage::build's root `column()` in a `stack()`, changing the
#                                 concrete type behind `impl View<State>` from FlexView to StackView
#                                 (RESULTS.md row D3). It adds NO new crate reference and NO log line
#                                 (D2's confound), so "did the patched build run" is judged from the
#                                 sentinel on screen (AFTER_RUN_HOOK captures it), not from a log line;
#                                 without a capture file the outcome is reported as unjudged.
#                                 RETURN_TYPE_WRAP=column wraps in another `column()` instead: FlexView
#                                 children are type-erased, so that keeps the type (the control).
#                  With --frust-run / --frust-restart the targets edit --app's sources, and every
#                  hazard edit is CUMULATIVE, so run N still changes the layout against the base a
#                  `restart required` relaunched from run N-1's edit:
#                    stock-label  the generated template's label `You have pushed the button this
#                                 many times:` gets ` vN` (needs no --prepare-app: the pristine app)
#                    home, card, helper  as above, in the prepared app
#                    state-type   `HomeState` -> an (N+1)-tuple of u32 (row D)
#                    state-field  `HomeState` gains `extra1`..`extraN` (row D2)
#                    return-type  `HomePage::build`'s root `scaffold(..)` wrapped in N `stack()`s (D3)
#                    badge        row D4 in pairs: odd run 2k-1 adds `struct Badge<k> { n: u32 }`
#                                 (a new type: patch 1), even run 2k gives it a second field
#                    either-swap  `HomeState` gains `extra1`..`extraN` AND `SHOW_HOME` flips, so the
#                                 root's `either(..)` swaps arms in the same patch (row D5)
#                    framework    appends `// measure.sh framework edit vN` to FRAMEWORK_FILE, a
#                                 source file of the app's frust path dependency (row E). It must
#                                 lie outside this repository (a scratch copy of frust).
#                  A crash, an app exit or a timeout is recorded as a run result, never fatal.
#   STARTUP_TIMEOUT=<s> overrides the first-frame wait (default 900: a cold dx fat build is slow).
#   PRE_RUN_PAUSE=<s>   sleeps <s> seconds after the first frame, before the first edit, so the
#                       counter can be clicked (state-preservation check); default 0.
#   POST_RUN_PAUSE=<s>  sleeps <s> seconds after the last run, before the app is killed, so the
#                       patched window can be inspected; default 0.
#   AFTER_RUN_HOOK=<cmd> run via `bash -c` after each hot run's frame (2 s settle first) as
#                       `<cmd> <run> <app pid>` (the two values are appended to the command string,
#                       so a script reads them as $1 and $2); its output is echoed. A capture file
#                       is whatever the hook writes to $AFTER_RUN_CAPTURE_DIR/run-<run>.png.
#   STATE_FIELD_WRITE=1 with --target state-field, the patched build also WRITES the new field
#                       (`state.extra = state.extra.wrapping_add(1)`) before reading it; default 0 (read only).
#   RESTART_TIMEOUT=<s> --frust-run: how long a `restart required` run waits for the relaunched
#                       app's first frame (default 600: a framework edit re-fat-builds frust).
#   APP_TARGET_DIR=<d>  the app's cargo target dir, where its processes are found by executable path
#                       (default: <app>/build/rust, the template's `.cargo/config.toml` value).
#
# --frust-run also prints, per patched run, the host side of the latency breakdown and the patch
# transport (R3-04). `host:` gives the mtimes of the session dir's newest `stub-N.o` and
# `patch-N.dylib` (`<APP_TARGET_DIR>/frust-hotpatch/session-<package>/`) as save offsets, plus the
# patch's size and mode. `hand-off:` says whether the app logged `frust-devtools: patch file handed
# off (<len> bytes, checks passed)` after the save. That line is debug level, so it appears only
# when the app runs with FRUST_LOG=debug (FRUST_RUN_ARGS="--define FRUST_LOG=debug"); without it
# the run reports "not logged", which is evidence neither way.
#
# ANDROID (H2-04): `--frust-run --serial <adb-serial>` (or `-d <adb-serial>`, or `--android` with the
# serial taken from $ANDROID_SERIAL) runs `frust run --watch -d <serial>` instead: the hot session
# on a physical device. The serial is never written anywhere but the log dir. The device must be
# awake and unlocked (checked; the script never touches the power state). What changes:
#   - instruments (H2-07): the Android shell logs `frust-hotpatch: applied t_unix_ms=<ms>` when a
#     UI tick drains the patch latch and `frust-hotpatch: frame t_unix_ms=<ms>` on the frame that
#     consumed it (H2-05). Per patched run the script reads `applied` and `frame` from those probe
#     lines of the app's pid in the device log (t_unix_ms is the device clock, moved onto the
#     host's). A tree without them falls back to the 5 s frame-wait backstop line (`frust-devtools:
#     no frame followed the patch within 5s; answering anyway`): `applied` = its device time -
#     5000 ms, no `frame`. Each run line names its source (`source: probe` / `source: backstop`)
#     and whether a backstop line followed the save (`backstop: none` / `PRESENT`); the exit
#     report counts every backstop line. The CLI's `patched` line (host clock) is reported beside
#     them. A restart run's first frame is the relaunched activity's `ActivityTaskManager:
#     Displayed <package>/...` line. `host:` also names `patch-N.upload.so`, the stripped copy the
#     app is sent (H2-06), with its size.
#   - clocks: device times are moved onto the host clock by an offset measured before every run
#     (`clock:` line: the min-RTT sample of `adb shell echo $EPOCHREALTIME`, error <= rtt/2).
#   - device log: `adb logcat -v epoch` from the session start into <log dir>/logcat.txt (bounded
#     by the device clock, never `logcat -c`); its `avc: denied` lines and the crash buffer are
#     printed at exit.
#   - per run: the app PID (`pidof <package>`), RSS (VmRSS of /proc/<pid>/status), `maps:` the
#     `/memfd:frust-hotpatch (deleted)` mappings of /proc/<pid>/maps (read through `run-as`, saved
#     as maps-run-N.txt), `info:` the app's own `hotpatch_info` counters (patches_applied,
#     patch_bytes_loaded, anchor_runtime) over a private `adb forward` to the devtools port of the
#     discovery line (HOTPATCH_INFO=0 skips it), and a copy of the run's stub-N.o (relocation
#     evidence: its thunks hold the slid base addresses).
#   - SCREENCAP_DIR=<dir> (outside this repo): `adb exec-out screencap -p` into before.png (after
#     PRE_RUN_HOOK) and run-N.png (2 s after each run).
#   - ANDROID_PACKAGE overrides the package read from android/app/build.gradle.kts.
#   - an app still running after the CLI exits is `am force-stop`ped (that package only).
#   --frust-restart is refused with --serial: it passes `--features frust/hotpatch`, which
#   `frust run --watch -d` refuses, and its timing reads the desktop probe. The device restart
#   baseline is the hot session's own `restart required` rerun (a hazard target such as
#   state-field), timed to `Displayed`.
#   PRE_RUN_HOOK=<cmd>  run via `bash -c` after the first frame, before PRE_RUN_PAUSE, as `<cmd> <app
#                       pid>` (e.g. taps that raise the counter), desktop and Android alike.
#
# WINDOWS (H3-03): `--frust-run --windows <ssh-alias>` (`--windows-host` is the same flag; with no
# alias after it, the alias comes from $WINDOWS_HOST) drives a Windows x64 rig over
# ssh from this host: `--app` and FRUST are paths ON THE RIG (e.g. `--app 'C:\scratch\hotapp'`,
# FRUST='C:\...\frust.exe'), and `--prepare-app <dir> --windows <alias>` prepares an app there.
# The rig's shell is cmd.exe; every remote step is one ssh call running PowerShell from
# `-EncodedCommand` (no Unix tools there, nothing copied onto it but the edited sources). What changes:
#   - runner: `frust run --watch` cannot hold a window from an ssh logon, so the script writes
#     `<WINDOWS_ROOT>\run.cmd` (`cd /d <app> || exit /b 1`, then the CLI redirected into
#     `<WINDOWS_ROOT>\watch-<row>-<n>.log`) and starts it as a one-shot scheduled task (WINDOWS_TASK,
#     default h3gate: `schtasks /create /sc once` + `/run`), on the interactive desktop.
#   - clocks: one clock, the rig's. A save is written by PowerShell (`WriteAllBytes` of the edited
#     file, no BOM, same path) and stamped by `[DateTimeOffset]::UtcNow` in the same call, so
#     save->frame = the app's `frame t_unix_ms=` minus that stamp, both on the rig's clock.
#   - log: a PowerShell tailer (one long-lived ssh call) reads the redirected log every 20 ms and
#     streams each new line prefixed with the rig's unix ms at the read, into <log dir>/runner.log;
#     the CLI's outcome line is timed by that stamp (late by at most ~20 ms plus the flush).
#   - probe source: a patched run reads `applied` / `frame` from the app's probe lines; without an
#     `applied` line within 10 s it falls back to the 5 s frame-wait backstop line (`applied` = the
#     line's stamp - 5000 ms, no frame), and says which (`source: probe|backstop`, `backstop:
#     none|PRESENT`), as the Android leg does.
#   - per run: PID and working set (RSS) of the app (`Get-Process`: image `<package>.exe` under
#     APP_TARGET_DIR), `host:` (the session dir's stub/patch-N.dll on the rig, stamped on its clock),
#     `counters:` and `ui:` (below), and with SCREENCAP_DIR a desktop capture made by the task
#     `<WINDOWS_TASK>shot`, copied back into SCREENCAP_DIR.
#   - stop: `taskkill /T /F` of the frust.exe at FRUST and of the app's processes under
#     APP_TARGET_DIR, then the scheduled task is deleted. Any devtools token is redacted in the rig's
#     log and in runner.log.
#   - restore: the pristine sources are written back on the rig and compared by SHA-256.
#   - state: WINDOWS_PRESSES=<n> presses the app's `Increment` button n times after the first frame
#     (UI Automation InvokePattern, run on the desktop by the task `<WINDOWS_TASK>ui`; no pointer
#     input), and a `ui:` line before run 1 and after each run lists the window's text elements as
#     UI Automation reports them (the counter, the sentinels): the State evidence when the rig's
#     user session is disconnected or locked and a desktop capture comes out blank.
#   - counters: the CLI redacts the devtools token, so the app's `hotpatch_info` is not asked from
#     here; `counters:` gives the app's own `applying patch <n> (<len> bytes` lines (debug level:
#     FRUST_RUN_ARGS="--define FRUST_LOG=debug") and the session dir's patch-N.dll count and bytes.
#   WINDOWS_ROOT  scratch dir on the rig (default `C:\Dev\h3-gate`): run.cmd, watch logs, shots.
#   APP_TARGET_DIR  the app's target dir on the rig (default `<app>\build\rust`).
#   Rig paths may not hold spaces or any of `"'%&|<>^`. --target framework is refused here.
#
# Every edited file is restored on exit (trap EXIT, Ctrl-C included) and the runner's process group
# is killed; the summary ends with `git status` of this directory as proof (with --frust-run /
# --frust-restart: a `cmp` of each restored file against its pristine copy, and any app process of
# the scratch app that outlived the CLI is killed by executable path). The manual restore command
# (pristine copies in the log dir) is printed up front, for a SIGKILLed script. Logs live in a
# mktemp dir whose path is printed. A failed wait is recorded as a run result, never a failure.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
RUNNER_DIR="${SCRIPT_DIR}/runner"
HOME_RS="${SCRIPT_DIR}/app/src/home_page.rs"
CARD_RS="${SCRIPT_DIR}/app/src/counter_card.rs"

MODE="hotpatch"
RUNS=5
TARGET="home"
APP=""
PREPARE_APP=""
EDIT_TIMEOUT=60
STARTUP_TIMEOUT="${STARTUP_TIMEOUT:-900}"
RESTART_TIMEOUT="${RESTART_TIMEOUT:-600}"
FRAMEWORK_FILE="${FRAMEWORK_FILE:-}"
FRUST_RUN_ARGS="${FRUST_RUN_ARGS:-}"
PRE_RUN_PAUSE="${PRE_RUN_PAUSE:-0}"
POST_RUN_PAUSE="${POST_RUN_PAUSE:-0}"
STATE_FIELD_WRITE="${STATE_FIELD_WRITE:-0}"
TARGETS="home|card|helper|state-type|state-field|return-type|stock-label|badge|either-swap|framework"
AFTER_RUN_HOOK="${AFTER_RUN_HOOK:-}"
RETURN_TYPE_WRAP="${RETURN_TYPE_WRAP:-stack}"
AFTER_RUN_CAPTURE_DIR="${AFTER_RUN_CAPTURE_DIR:-}"
PRE_RUN_HOOK="${PRE_RUN_HOOK:-}"
ROW=""
# Android leg (--serial / -d / --android): see the header.
ANDROID=0
SERIAL=""
ANDROID_PACKAGE="${ANDROID_PACKAGE:-}"
SCREENCAP_DIR="${SCREENCAP_DIR:-}"
HOTPATCH_INFO="${HOTPATCH_INFO:-1}"
# Device clock minus host clock, in ms (0 on the desktop: one clock).
CLOCK_OFFSET_MS=0
# Windows leg (--windows <ssh-alias>): see the header.
WINDOWS=0
WINDOWS_HOST="${WINDOWS_HOST:-}"
WINDOWS_ROOT="${WINDOWS_ROOT:-C:\\Dev\\h3-gate}"
WINDOWS_TASK="${WINDOWS_TASK:-h3gate}"
WINDOWS_PRESSES="${WINDOWS_PRESSES:-0}"
WIN_LOG=""
WIN_MIRROR=""

# The comment header (line 2 up to the first non-comment line), without the `# ` prefix.
usage() {
  awk 'NR == 1 { next } /^#/ { sub(/^# ?/, ""); print; next } { exit }' "$0"
}

while [ $# -gt 0 ]; do
  case "$1" in
    --hotpatch) MODE="hotpatch"; shift ;;
    --restart) MODE="restart"; shift ;;
    --frust-run) MODE="frust-run"; shift ;;
    --frust-restart) MODE="frust-restart"; shift ;;
    --app)
      [ $# -ge 2 ] || { echo "error: --app requires a directory" >&2; exit 2; }
      APP="$2"; shift 2 ;;
    --app=*) APP="${1#--app=}"; shift ;;
    --prepare-app)
      [ $# -ge 2 ] || { echo "error: --prepare-app requires a directory" >&2; exit 2; }
      PREPARE_APP="$2"; shift 2 ;;
    --prepare-app=*) PREPARE_APP="${1#--prepare-app=}"; shift ;;
    --runs)
      [ $# -ge 2 ] || { echo "error: --runs requires a number" >&2; exit 2; }
      RUNS="$2"; shift 2 ;;
    --runs=*) RUNS="${1#--runs=}"; shift ;;
    --target)
      [ $# -ge 2 ] || { echo "error: --target requires ${TARGETS}" >&2; exit 2; }
      TARGET="$2"; shift 2 ;;
    --target=*) TARGET="${1#--target=}"; shift ;;
    --row)
      [ $# -ge 2 ] || { echo "error: --row requires a label" >&2; exit 2; }
      ROW="$2"; shift 2 ;;
    --row=*) ROW="${1#--row=}"; shift ;;
    -d|--serial)
      [ $# -ge 2 ] || { echo "error: $1 requires an adb serial" >&2; exit 2; }
      ANDROID=1; SERIAL="$2"; shift 2 ;;
    --serial=*) ANDROID=1; SERIAL="${1#--serial=}"; shift ;;
    --android) ANDROID=1; shift ;;
    --windows|--windows-host)
      # A bare flag (no alias after it) takes the alias from $WINDOWS_HOST.
      WINDOWS=1
      if [ $# -ge 2 ] && [ "${2#-}" = "$2" ]; then WINDOWS_HOST="$2"; shift 2; else shift; fi ;;
    --windows=*|--windows-host=*) WINDOWS=1; WINDOWS_HOST="${1#*=}"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "error: unknown argument '$1' (see --help)" >&2; exit 2 ;;
  esac
done

command -v python3 >/dev/null 2>&1 || { echo "error: python3 is required" >&2; exit 2; }

REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel 2>/dev/null || true)"

# Prints `$1`'s physical absolute path (its directory resolved with `pwd -P`); `$1` must exist.
physical_path() {
  local dir base
  dir="$(cd "$(dirname "$1")" >/dev/null 2>&1 && pwd -P)" || return 1
  base="$(basename "$1")"
  if [ "$base" = "." ]; then echo "$dir"; else echo "${dir%/}/${base}"; fi
}

# Fails when `$1` (an existing path) lies inside this repository: the scratch app and the framework
# copy row E edits must live elsewhere, so a run never writes into a checkout.
outside_repo() {
  local real repo
  [ -n "$REPO_ROOT" ] || return 0
  real="$(physical_path "$1")" || return 1
  repo="$(cd "$REPO_ROOT" && pwd -P)"
  case "${real%/}/" in "${repo%/}/"*) return 1 ;; esac
  return 0
}

# ---- Windows leg helpers (--windows) ------------------------------------------------------------
# Every remote step is one `ssh <alias> powershell -EncodedCommand <script>` call: the script is
# UTF-16LE base64, so cmd.exe never parses it. Values reach it as `$A[0]`, `$A[1]`, ... (prepended
# as PowerShell single-quoted literals), never spliced into the script text.

# Whether `$1` is a rig path the script may also hand to cmd.exe and schtasks: drive-absolute, no
# spaces, no quotes and no cmd metacharacters.
win_path_ok() {
  case "$1" in [A-Za-z]:\\*) ;; *) return 1 ;; esac
  case "$1" in *[[:space:]\"\'%\&\|\<\>^]*) return 1 ;; esac
  return 0
}

# A PowerShell single-quoted literal of `$1`.
ps_q() {
  local q="'"
  printf "'%s'" "${1//$q/$q$q}"
}

# Runs PowerShell script `$1` on the rig with `$A` = the remaining arguments; stdin and stdout pass
# through byte for byte.
win_ps_raw() {
  local script="$1" pre="\$ProgressPreference = 'SilentlyContinue'; \$A = @(" sep="" a b64
  shift
  for a in "$@"; do pre="${pre}${sep}$(ps_q "$a")"; sep=", "; done
  b64="$(printf '%s); %s' "$pre" "$script" | iconv -f UTF-8 -t UTF-16LE | base64 | tr -d '\n')"
  # cmd.exe's command-line limit is 8191 characters.
  if [ "${#b64}" -gt 8000 ]; then echo "error: PowerShell script too long for one ssh call" >&2; return 2; fi
  ssh -o BatchMode=yes "$WINDOWS_HOST" \
    "powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand ${b64}"
}

# win_ps_raw with the CRs of PowerShell's text output removed.
win_ps() {
  win_ps_raw "$@" | tr -d '\r'
}

# Copies rig file `$1` to stdout, byte for byte.
win_get() {
  win_ps_raw '$b = [IO.File]::ReadAllBytes($A[0]); $o = [Console]::OpenStandardOutput()
    $o.Write($b, 0, $b.Length); $o.Flush()' "$1" < /dev/null
}

# Writes stdin to rig file `$1` (WriteAllBytes: truncated in place, no BOM added), then prints the
# rig's unix ms taken right after the write in the same process: the save stamp.
win_put() {
  win_ps '$m = New-Object IO.MemoryStream; [Console]::OpenStandardInput().CopyTo($m)
    [IO.File]::WriteAllBytes($A[0], $m.ToArray()); [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()' "$1"
}

# The rig's clock in unix ms.
win_now_ms() {
  win_ps '[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()' < /dev/null
}

# The rig path of mirror file `$1` (a source of --app, kept locally in WIN_MIRROR).
win_rig_path_of() {
  printf '%s\\src\\%s' "$APP" "$(basename "$1")"
}

# The app's processes on the rig, oldest first, as `<pid> <working set bytes>`: image name
# `<package>.exe` AND an image path under APP_TARGET_DIR, so only this scratch app's processes match.
win_app_procs() {
  win_ps 'Get-Process -Name $A[0] -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and $_.Path.StartsWith($A[1] + "\", [StringComparison]::OrdinalIgnoreCase) } |
    Sort-Object StartTime | ForEach-Object { "$($_.Id) $($_.WorkingSet64)" }' \
    "$APP_BIN" "$APP_TARGET_DIR" < /dev/null
}

# The log tailer, run on the rig for the whole session: reads `$A[0]` (the CLI's redirected output)
# every 20 ms and writes each new complete line to stdout as `<rig unix ms at the read> <line>`;
# exits once `$A[1]` exists.
WIN_TAIL_PS='$o = [Console]::OpenStandardOutput(); $enc = New-Object Text.UTF8Encoding $false
$dec = $enc.GetDecoder(); $buf = New-Object byte[] 65536; $chars = New-Object char[] 65536
$pend = ""; $k = 0
while (-not (Test-Path -LiteralPath $A[0])) {
  if (Test-Path -LiteralPath $A[1]) { exit 0 }; Start-Sleep -Milliseconds 50 }
$fs = [IO.File]::Open($A[0], "Open", "Read", "ReadWrite, Delete")
while ($true) {
  $n = $fs.Read($buf, 0, $buf.Length)
  if ($n -gt 0) {
    $t = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    $c = $dec.GetChars($buf, 0, $n, $chars, 0); $pend += New-Object string($chars, 0, $c)
    $i = $pend.LastIndexOf("`n")
    if ($i -ge 0) {
      $sb = New-Object Text.StringBuilder
      foreach ($l in $pend.Substring(0, $i).Split("`n")) {
        [void]$sb.Append("$t ").Append($l.TrimEnd("`r")).Append("`n") }
      $bytes = $enc.GetBytes($sb.ToString()); $o.Write($bytes, 0, $bytes.Length); $o.Flush()
      $pend = $pend.Substring($i + 1)
    }
  } else {
    $k++; if ($k % 25 -eq 0 -and (Test-Path -LiteralPath $A[1])) { exit 0 }
    Start-Sleep -Milliseconds 20
  }
}'

# The desktop capture script written to <WINDOWS_ROOT>\shot.ps1 (run by a scheduled task, on the
# interactive desktop): the whole virtual screen, DPI-aware, saved via a temp name so a reader never
# sees a half-written PNG.
WIN_SHOT_PS='param([string]$Out)
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type -Name Dpi -Namespace MeasureSh -MemberDefinition @"
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
"@
[void][MeasureSh.Dpi]::SetProcessDPIAware()
Start-Sleep -Milliseconds 700
$b = [System.Windows.Forms.SystemInformation]::VirtualScreen
$bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
$bmp.Save("$Out.tmp", [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
Move-Item -Force -LiteralPath "$Out.tmp" -Destination $Out
'

# The UI Automation reader written to <WINDOWS_ROOT>\ui.ps1 (see win_ui).
WIN_UI_PS='param([int]$TargetPid, [int]$Presses, [string]$Out)
$lines = @()
try {
  Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
  $h = (Get-Process -Id $TargetPid -ErrorAction Stop).MainWindowHandle
  if ($h -eq [IntPtr]::Zero) { throw "pid $TargetPid has no main window" }
  $root = [System.Windows.Automation.AutomationElement]::FromHandle($h)
  $any = [System.Windows.Automation.Condition]::TrueCondition
  $scope = [System.Windows.Automation.TreeScope]::Descendants
  if ($Presses -gt 0) {
    $btn = $root.FindAll($scope, $any) | Where-Object { $_.Current.Name -eq "Increment" } | Select-Object -First 1
    if (-not $btn) { throw "no Increment button" }
    $ip = $btn.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
    for ($i = 0; $i -lt $Presses; $i++) { $ip.Invoke(); Start-Sleep -Milliseconds 400 }
    Start-Sleep -Milliseconds 800
    $lines += "pressed Increment x$Presses (UI Automation Invoke)"
  }
  $text = [System.Windows.Automation.ControlType]::Text
  $names = $root.FindAll($scope, $any) | Where-Object { $_.Current.ControlType -eq $text } |
    ForEach-Object { [char]39 + $_.Current.Name + [char]39 }
  $lines += "pid ${TargetPid} texts: " + ($names -join " | ")
} catch { $lines += "failed: $($_.Exception.Message)" }
[IO.File]::WriteAllText("$Out.tmp", ($lines -join "`n") + "`n")
Move-Item -Force -LiteralPath "$Out.tmp" -Destination $Out
'

# `--prepare-app <dir> --windows <alias>`: prepare_app on a local mirror of the rig app's sources,
# then write the three rewritten files back on the rig.
win_prepare_app() {
  local dir="$1" mirror f
  win_path_ok "$dir" || { echo "error: '${dir}' is not a usable rig path (see --help)" >&2; return 2; }
  [ "$(win_ps 'Test-Path -LiteralPath $A[0]' "${dir}\\src\\counter_card.rs" < /dev/null)" = "False" ] \
    || { echo "error: '${dir}' is already prepared (or the rig is unreachable)" >&2; return 2; }
  mirror="$(mktemp -d "${TMPDIR:-/tmp}/hotpatch-spike-prep.XXXXXX")" || return 2
  mkdir -p "${mirror}/src"
  for f in Cargo.toml src/lib.rs src/home_page.rs; do
    win_get "${dir}\\${f//\//\\}" > "${mirror}/${f}"
    [ -s "${mirror}/${f}" ] || { echo "error: could not read ${dir}\\${f//\//\\} on the rig" >&2; return 2; }
  done
  prepare_app "$mirror" || return $?
  for f in src/lib.rs src/home_page.rs src/counter_card.rs; do
    win_put "${dir}\\${f//\//\\}" < "${mirror}/${f}" > /dev/null \
      || { echo "error: could not write ${dir}\\${f//\//\\} on the rig" >&2; return 2; }
  done
  echo "Prepared ${dir} on the Windows rig (local mirror: ${mirror})."
}

# `--prepare-app <dir>`: rewrite a freshly generated `frust create` app into the measured layout
# (see the header). Refuses an app already prepared or not in the generated shape.
prepare_app() {
  local dir="$1"
  [ -f "${dir}/Cargo.toml" ] && [ -f "${dir}/src/lib.rs" ] && [ -f "${dir}/src/home_page.rs" ] \
    || { echo "error: '${dir}' is not a generated frust create app" >&2; return 2; }
  outside_repo "$dir" || { echo "error: '${dir}' lies inside this repository" >&2; return 2; }
  [ ! -e "${dir}/src/counter_card.rs" ] || { echo "error: '${dir}' is already prepared" >&2; return 2; }
  python3 - "$dir" <<'PY'
import re, sys
root = sys.argv[1]
lib_path = f"{root}/src/lib.rs"
lib = open(lib_path).read()
call = re.compile(r"        component\(home_page::HomePage \{\n            (title: [^\n]*)\n        \}\)\n")
if lib.count("mod home_page;\n") != 1 or len(call.findall(lib)) != 1 \
        or lib.count("use frust::{Component, View, component};\n") != 1:
    sys.exit("measure.sh: src/lib.rs is not the generated template's")
lib = lib.replace("mod home_page;\n", "mod counter_card;\nmod home_page;\n")
lib = lib.replace("use frust::{Component, View, component};\n",
                  "use frust::{Component, View, component, either, text};\n")
lib = call.sub(lambda m: (
    "        // measure.sh --prepare-app: HomePage lives in one arm of an `either(..)` (row D5).\n"
    "        either(\n"
    "            home_page::SHOW_HOME,\n"
    "            || {\n"
    "                component(home_page::HomePage {\n"
    f"                    {m.group(1)}\n"
    "                })\n"
    "            },\n"
    "            || text(\"HomePage is in the other arm (SHOW_HOME = false)\"),\n"
    "        )\n"), lib)
open(lib_path, "w").write(lib)
home = """//! The home screen of the `frust create` counter, rewritten by `measure.sh --prepare-app`: the
//! template's scaffold, app bar, FAB and label, plus a named State, a child card and the
//! `// SENTINEL-*`, `// STATE-*`, `// ROOT-*`, `// USE-FRUST` and `// SHOW-HOME` markers that
//! measure.sh edits in place. Keep each marker on the line it annotates.

use frust::{Align, Alignment, Component, CrossAxisAlignment, View, column, component, scaffold, text}; // USE-FRUST

use crate::counter_card::CounterCard;

/// Which arm of the root's `either(..)` is live: `true` hosts this page (row D5 flips it).
pub const SHOW_HOME: bool = true; // SHOW-HOME

/// The counter screen; the count is its local state.
pub struct HomePage {
    pub title: String,
}

/// HomePage's local state, a named struct so a field can be added to it (row D2).
pub struct HomeState {
    pub count: u32, // STATE-FIELDS
}

/// The state type the widget fns are written against, so a state-type edit rewrites only the
/// marked lines.
type PageState = <HomePage as Component>::State;

impl Component for HomePage {
    type State = HomeState; // STATE-TYPE

    fn init(&self) -> PageState {
        HomeState { count: 0 } // STATE-INIT
    }

    fn build(&self, state: &mut PageState) -> impl View<PageState> {
        let count = state.count; // STATE-READ
        let sentinel = text("hotpatch-sentinel: v0"); // SENTINEL-HOME
        scaffold(counter_body(count, sentinel)) // ROOT-OPEN
            .app_bar(app_bar(&self.title))
            .fab(increment_fab()) // ROOT-CLOSE
    }
}

// The Material app bar insets itself under the status bar, so no safe_area wrapper is needed.
fn app_bar(title: &str) -> impl View<PageState> + use<> {
    frust_material::app_bar::<PageState>(title)
        .container(frust_material::AppBarContainer::InversePrimary)
        .elevation(2)
}

fn counter_body(count: u32, sentinel: impl View<PageState>) -> impl View<PageState> {
    Align(
        Alignment::CENTER,
        column()
            .child(text("You have pushed the button this many times:"))
            .child(text(format!("count: {count}")).size(48.0))
            .child(sentinel)
            .child(component(CounterCard))
            .cross_axis(CrossAxisAlignment::Center),
    )
}

// The Scaffold keeps the FAB clear of the gesture-nav inset.
fn increment_fab() -> impl View<PageState> + use<> {
    frust_material::fab(frust::icon(frust_material::icons::ADD), |state: &mut PageState| {
        state.count += 1 // STATE-INC
    })
    .label("Increment")
}
"""
card = """//! A second child component (row B): its own module and its own sentinel line.

use frust::{Component, View, column, text};

/// A card under the counter showing its own sentinel line.
pub struct CounterCard;

impl Component for CounterCard {
    type State = ();

    fn init(&self) {}

    fn build(&self, _state: &mut ()) -> impl View<()> {
        let sentinel = text("card-sentinel: v0"); // SENTINEL-CARD
        column().child(text("Counter card")).child(sentinel)
    }
}
"""
open(f"{root}/src/home_page.rs", "w").write(home)
open(f"{root}/src/counter_card.rs", "w").write(card)
PY
  local status=$?
  [ "$status" -eq 0 ] && echo "Prepared ${dir}: src/home_page.rs, src/counter_card.rs (new), src/lib.rs."
  return "$status"
}

if [ "$WINDOWS" = 1 ] && [ -z "$WINDOWS_HOST" ]; then
  echo "error: --windows needs an ssh alias (argument or WINDOWS_HOST)" >&2; exit 2
fi
if [ -n "$PREPARE_APP" ]; then
  if [ "$WINDOWS" = 1 ]; then win_prepare_app "$PREPARE_APP"; else prepare_app "$PREPARE_APP"; fi
  exit $?
fi

case "$RUNS" in
  ''|*[!0-9]*|0) echo "error: --runs must be a positive integer, got '$RUNS'" >&2; exit 2 ;;
esac

FRUST_MODE=0
case "$MODE" in frust-run|frust-restart) FRUST_MODE=1 ;; esac
if [ "$WINDOWS" = 1 ]; then
  [ "$FRUST_MODE" = 1 ] || { echo "error: --windows drives only --frust-run / --frust-restart" >&2; exit 2; }
  [ "$ANDROID" = 0 ] || { echo "error: --windows and --serial/--android exclude each other" >&2; exit 2; }
  [ "$TARGET" != "framework" ] || { echo "error: --target framework is not supported with --windows" >&2; exit 2; }
  command -v iconv >/dev/null 2>&1 && command -v base64 >/dev/null 2>&1 \
    || { echo "error: --windows needs iconv and base64" >&2; exit 2; }
  for v in APP FRUST WINDOWS_ROOT; do
    win_path_ok "${!v:-}" || { echo "error: ${v} '${!v:-}' is not a usable rig path (see --help)" >&2; exit 2; }
  done
  APP_TARGET_DIR="${APP_TARGET_DIR:-${APP}\\build\\rust}"
  win_path_ok "$APP_TARGET_DIR" \
    || { echo "error: APP_TARGET_DIR '${APP_TARGET_DIR}' is not a usable rig path" >&2; exit 2; }
  # A local mirror of the app's edited sources: apply_edit rewrites the mirror, win_put saves it on
  # the rig.
  WIN_MIRROR="$(mktemp -d "${TMPDIR:-/tmp}/hotpatch-spike-win.XXXXXX")" || exit 2
  mkdir -p "${WIN_MIRROR}/src"
  win_get "${APP}\\Cargo.toml" > "${WIN_MIRROR}/Cargo.toml"
  [ -s "${WIN_MIRROR}/Cargo.toml" ] \
    || { echo "error: no ${APP}\\Cargo.toml on the rig (or the rig is unreachable)" >&2; exit 2; }
  for f in home_page.rs counter_card.rs; do
    win_get "${APP}\\src\\${f}" > "${WIN_MIRROR}/src/${f}" 2>/dev/null
    [ -s "${WIN_MIRROR}/src/${f}" ] || rm -f "${WIN_MIRROR}/src/${f}"
  done
  HOME_RS="${WIN_MIRROR}/src/home_page.rs"
  CARD_RS="${WIN_MIRROR}/src/counter_card.rs"
  APP_BIN="$(awk -F'"' '/^\[/ { p = ($0 == "[package]") } p && /^name *=/ { print $2; exit }' \
    "${WIN_MIRROR}/Cargo.toml")"
  [ -n "$APP_BIN" ] || { echo "error: no package name in ${APP}\\Cargo.toml" >&2; exit 2; }
elif [ "$FRUST_MODE" = 1 ]; then
  if [ -z "$APP" ] || [ ! -f "${APP}/Cargo.toml" ]; then
    echo "error: --${MODE} needs --app <dir>, a scratch frust create app" >&2
    exit 2
  fi
  APP="$(physical_path "$APP")"
  outside_repo "$APP" || { echo "error: --app '${APP}' lies inside this repository" >&2; exit 2; }
  HOME_RS="${APP}/src/home_page.rs"
  CARD_RS="${APP}/src/counter_card.rs"
  FRUST="${FRUST:-$(command -v frust 2>/dev/null || true)}"
  if [ -z "$FRUST" ] || [ ! -x "$FRUST" ]; then
    echo "error: no frust found; set FRUST=<path to the frust binary>" >&2
    exit 2
  fi
  APP_BIN="$(awk -F'"' '/^\[/ { p = ($0 == "[package]") } p && /^name *=/ { print $2; exit }' \
    "${APP}/Cargo.toml")"
  [ -n "$APP_BIN" ] || { echo "error: no package name in ${APP}/Cargo.toml" >&2; exit 2; }
  APP_TARGET_DIR="${APP_TARGET_DIR:-${APP}/build/rust}"
else
  case "$TARGET" in
    stock-label|badge|either-swap|framework)
      echo "error: --target ${TARGET} needs --frust-run or --frust-restart" >&2; exit 2 ;;
  esac
fi
case "$TARGET" in
  home|helper|state-type|state-field|return-type|stock-label|badge|either-swap) EDIT_FILE="$HOME_RS" ;;
  card) EDIT_FILE="$CARD_RS" ;;
  framework)
    if [ -z "$FRAMEWORK_FILE" ] || [ ! -f "$FRAMEWORK_FILE" ]; then
      echo "error: --target framework needs FRAMEWORK_FILE=<a frust source file>" >&2; exit 2
    fi
    FRAMEWORK_FILE="$(physical_path "$FRAMEWORK_FILE")"
    outside_repo "$FRAMEWORK_FILE" || {
      echo "error: FRAMEWORK_FILE must lie outside this repository (use a scratch frust copy)" >&2
      exit 2
    }
    EDIT_FILE="$FRAMEWORK_FILE" ;;
  *) echo "error: --target must be ${TARGETS}, got '$TARGET'" >&2; exit 2 ;;
esac
[ -f "$EDIT_FILE" ] || { echo "error: ${EDIT_FILE} does not exist (--prepare-app first?)" >&2; exit 2; }

# The session dir of the host side (`host:` lines): `session-<package>` on the desktop.
SESSION_DIR_NAME="session-${APP_BIN:-}"
if [ "$ANDROID" = 1 ]; then
  SERIAL="${SERIAL:-${ANDROID_SERIAL:-}}"
  [ -n "$SERIAL" ] || { echo "error: --android needs --serial <adb-serial> or ANDROID_SERIAL" >&2; exit 2; }
  if [ "$MODE" != "frust-run" ]; then
    echo "error: --serial drives only --frust-run (--frust-restart passes --features, which" \
      "\`frust run --watch -d\` refuses; time a device restart with a hazard target instead)" >&2
    exit 2
  fi
  command -v adb >/dev/null 2>&1 || { echo "error: adb is required" >&2; exit 2; }
  [ "$(adb -s "$SERIAL" get-state 2>/dev/null)" = "device" ] \
    || { echo "error: the device given by --serial/ANDROID_SERIAL is not attached" >&2; exit 2; }
  adb -s "$SERIAL" shell dumpsys power 2>/dev/null | grep 'mWakefulness=Awake' >/dev/null \
    || { echo "error: the device is asleep; wake and unlock it first (keyevent 26 toggles)" >&2; exit 2; }
  if [ -z "$ANDROID_PACKAGE" ]; then
    ANDROID_PACKAGE="$(sed -n 's/^ *applicationId = "\(.*\)"$/\1/p' \
      "${APP}/android/app/build.gradle.kts" 2>/dev/null | head -1)"
  fi
  [ -n "$ANDROID_PACKAGE" ] \
    || { echo "error: no applicationId in ${APP}/android/app/build.gradle.kts; set ANDROID_PACKAGE" >&2; exit 2; }
  # `session-<crate>-<triple>` (hotpatch::android): the lib crate name, `-` folded to `_`.
  SESSION_DIR_NAME="session-${APP_BIN//-/_}-aarch64-linux-android"
fi
if [ -n "$SCREENCAP_DIR" ]; then
  [ "$ANDROID" = 1 ] || [ "$WINDOWS" = 1 ] \
    || { echo "error: SCREENCAP_DIR needs --serial (adb screencap) or --windows" >&2; exit 2; }
  [ -d "$SCREENCAP_DIR" ] || { echo "error: SCREENCAP_DIR '${SCREENCAP_DIR}' is not a directory" >&2; exit 2; }
  outside_repo "$SCREENCAP_DIR" || { echo "error: SCREENCAP_DIR must lie outside this repository" >&2; exit 2; }
fi

if [ "$MODE" = "hotpatch" ]; then
  DX="${DX:-$(command -v dx 2>/dev/null || true)}"
  if [ -z "$DX" ] || [ ! -x "$DX" ]; then
    echo "error: no dx found; set DX=<path> (dioxus-cli 0.7.10, see README.md)" >&2
    exit 2
  fi
  DX_VERSION="$("$DX" --version 2>/dev/null || true)"
  case "$DX_VERSION" in
    *0.7.10*) ;;
    *) echo "warning: '$DX' reports '$DX_VERSION', not 0.7.10 (subsecond is pinned =0.7.10)" >&2 ;;
  esac
fi

TMP_ROOT="${TMPDIR:-/tmp}"
LOG_DIR="$(mktemp -d "${TMP_ROOT%/}/hotpatch-spike.XXXXXX")"
LOG="${LOG_DIR}/runner.log"
: > "$LOG"
# Pristine copies of every file a run may edit, restored in place on exit: `<file>\t<copy>` lines.
RESTORE_LIST="${LOG_DIR}/restore.list"
: > "$RESTORE_LIST"
for f in "$HOME_RS" "$CARD_RS" "$EDIT_FILE"; do
  [ -f "$f" ] || continue
  grep -qF "${f}"$'\t' "$RESTORE_LIST" && continue
  copy="${LOG_DIR}/$(wc -l < "$RESTORE_LIST" | tr -d ' ')-$(basename "$f").orig"
  cp "$f" "$copy"
  printf '%s\t%s\n' "$f" "$copy" >> "$RESTORE_LIST"
done
EDIT_ORIG="$(awk -F'\t' -v f="$EDIT_FILE" '$1 == f { print $2; exit }' "$RESTORE_LIST")"

RUNNER_PID=""
APP_PID=""

now_ms() {
  python3 -c 'import time;print(int(time.time()*1000))'
}

# ---- Android leg helpers (--serial) -------------------------------------------------------------
DEVLOG="${LOG_DIR}/logcat.txt"
DEVLOG_PID=""
DEVLOG_SINCE=""

adb_() {
  adb -s "$SERIAL" "$@"
}

# The package's PIDs on the device, one per line (none when it is not running).
device_pids() {
  adb_ shell pidof "$ANDROID_PACKAGE" 2>/dev/null | tr -d '\r' | tr ' ' '\n' | grep '^[0-9][0-9]*$'
}

# Whether pid `$1` of the app is alive: `kill -0` on the desktop, `pidof` on the device,
# `Get-Process` on the Windows rig.
app_alive() {
  if [ "$ANDROID" = 1 ]; then
    device_pids | grep -x "$1" >/dev/null
  elif [ "$WINDOWS" = 1 ]; then
    win_app_procs | awk -v p="$1" '$1 == p { hit = 1 } END { exit !hit }'
  else
    kill -0 "$1" 2>/dev/null
  fi
}

# Sets CLOCK_OFFSET_MS (device minus host) from the min-RTT sample of `$1` (default 5)
# `adb shell echo $EPOCHREALTIME` round trips and prints `clock: ...`. Keeps the old offset when no
# sample parses.
measure_clock_offset() {
  local out
  out="$(python3 - "$SERIAL" "${1:-5}" <<'PY'
import subprocess, sys, time
serial, n = sys.argv[1], int(sys.argv[2])
best = None
for _ in range(n):
    t0 = time.time() * 1000
    out = subprocess.run(["adb", "-s", serial, "shell", "echo $EPOCHREALTIME"],
                         capture_output=True, text=True).stdout.strip()
    t1 = time.time() * 1000
    try:
        dev = float(out) * 1000
    except ValueError:
        continue
    if best is None or t1 - t0 < best[1]:
        best = (dev - (t0 + t1) / 2, t1 - t0)
if best is None:
    sys.exit(1)
print("%d %d" % (round(best[0]), round(best[1])))
PY
)" || { echo "clock: offset not measured (keeping ${CLOCK_OFFSET_MS} ms)"; return 0; }
  CLOCK_OFFSET_MS="${out%% *}"
  echo "clock: device - host = ${CLOCK_OFFSET_MS} ms (rtt ${out##* } ms, error <= rtt/2)"
}

# Starts `adb logcat -v epoch` from the device's current time into DEVLOG (bounded by the device
# clock: the device log is never cleared).
start_devlog() {
  DEVLOG_SINCE="$(adb_ shell 'echo $EPOCHREALTIME' | tr -d '\r')"
  # `adb` itself, not the adb_ function: `$!` must be the adb process, or the kill at exit only
  # reaches a subshell and leaves the stream running.
  adb -s "$SERIAL" logcat -v epoch -T "$DEVLOG_SINCE" > "$DEVLOG" 2>&1 < /dev/null &
  DEVLOG_PID=$!
}

# The host-clock ms of the first DEVLOG line matching ERE `$1` whose device time, moved onto the
# host clock, is later than `$2`; with `$3`, only lines of that pid. Prints nothing when none.
devlog_first() {
  awk -v re="$1" -v s="$2" -v o="$CLOCK_OFFSET_MS" -v p="${3:-}" '
    $1 ~ /^[0-9]+\.[0-9]+$/ && $0 ~ re && (p == "" || $2 == p) {
      t = $1 * 1000 - o; if (t > s) { printf "%.0f\n", t; exit } }' "$DEVLOG" 2>/dev/null
}

# Waits up to `$3` s for devlog_first `$1` `$2` (`$4`: pid filter); prints its host ms.
wait_devlog() {
  local deadline=$(( $(date +%s) + $3 )) t
  while [ "$(date +%s)" -lt "$deadline" ]; do
    t="$(devlog_first "$1" "$2" "${4:-}")"
    if [ -n "$t" ]; then echo "$t"; return 0; fi
    if ! kill -0 "$RUNNER_PID" 2>/dev/null; then return 1; fi
    sleep 0.1
  done
  return 1
}

# The relaunched activity's first frame: `ActivityTaskManager: Displayed <package>/...`.
displayed_re() {
  echo "ActivityTaskManager: Displayed ${ANDROID_PACKAGE//./\\.}/"
}

# The fallback apply moment of pid `$2`'s patch after host ms `$1`: its 5 s frame-wait backstop
# line, minus 5000 ms (see the header). Waits up to `$3` s.
BACKSTOP_RE='no frame followed the patch within 5s'
wait_applied_backstop() {
  local t
  t="$(wait_devlog "$BACKSTOP_RE" "$1" "$3" "$2")" || return 1
  echo $((t - 5000))
}

# The host-clock ms of the first `frust-hotpatch: <$1> t_unix_ms=<ms>` probe line in DEVLOG (with
# `$3`, of that pid only) whose stamp, moved onto the host clock, is later than `$2`. The stamp is
# the device's unix clock at the log call, not logcat's. Prints nothing when none.
devlog_probe() {
  awk -v k="frust-hotpatch: $1 t_unix_ms=" -v s="$2" -v o="$CLOCK_OFFSET_MS" -v p="${3:-}" '
    $1 ~ /^[0-9]+\.[0-9]+$/ && (p == "" || $2 == p) && index($0, k) {
      v = substr($0, index($0, k) + length(k)); sub(/[^0-9].*/, "", v)
      if (v == "") next
      t = v - o; if (t > s) { printf "%.0f\n", t; exit } }' "$DEVLOG" 2>/dev/null
}

# Waits up to `$3` s for devlog_probe `$1` `$2` (`$4`: pid filter); prints its host ms.
wait_probe() {
  local deadline=$(( $(date +%s) + $3 )) t
  while [ "$(date +%s)" -lt "$deadline" ]; do
    t="$(devlog_probe "$1" "$2" "${4:-}")"
    if [ -n "$t" ]; then echo "$t"; return 0; fi
    sleep 0.1
  done
  return 1
}

# Waits up to `$2` s for a devtools discovery line in the CLI's output after log line `$1`: the
# relaunched session is live once its endpoint is announced.
wait_discovery() {
  local from_line="$1" deadline=$(( $(date +%s) + $2 ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    # No `grep -q` behind a pipe: under pipefail its early exit fails the pipeline (SIGPIPE).
    awk -v f="$from_line" 'NR > f && /frust-devtools listening on [0-9]/ { hit = 1 } END { exit !hit }' \
      "$LOG" && return 0
    if ! kill -0 "$RUNNER_PID" 2>/dev/null; then return 1; fi
    sleep 0.2
  done
  return 1
}

# `maps: ...` for pid `$1` after run `$2`: its `/memfd:frust-hotpatch (deleted)` mappings (all and
# r-xp), the file saved as maps-run-<run>.txt in the log dir. `run-as` reads it as the app's uid.
device_maps() {
  local f="${LOG_DIR}/maps-run-$2.txt"
  adb_ shell run-as "$ANDROID_PACKAGE" cat "/proc/$1/maps" 2>/dev/null | tr -d '\r' > "$f"
  if [ ! -s "$f" ]; then echo "maps: unreadable (run-as needs a debuggable app)"; return 0; fi
  echo "maps: $(grep -c 'memfd:frust-hotpatch (deleted)' "$f") /memfd:frust-hotpatch (deleted)" \
    "mappings, $(grep 'memfd:frust-hotpatch (deleted)' "$f" | grep -c ' r-xp ') r-xp" \
    "(maps-run-$2.txt)"
}

# `info: ...`: the app's own `hotpatch_info` for pid `$1`, over a private `adb forward` to the
# devtools port and token of that pid's discovery line in DEVLOG (removed again at once).
device_hotpatch_info() {
  local line port token
  line="$(awk -v p="$1" '$2 == p && /frust-devtools listening on [0-9]+ token / { l = $0 } END { print l }' \
    "$DEVLOG" 2>/dev/null)"
  port="$(echo "$line" | sed -n 's/.*listening on \([0-9][0-9]*\) token .*/\1/p')"
  token="$(echo "$line" | sed -n 's/.*listening on [0-9][0-9]* token \([0-9a-f][0-9a-f]*\).*/\1/p')"
  if [ -z "$port" ] || [ -z "$token" ]; then echo "info: no discovery line for pid $1"; return 0; fi
  # The token travels in the environment of this one process, never argv (visible in `ps`).
  HOTPATCH_TOKEN="$token" python3 - "$SERIAL" "$port" <<'PY'
import json, os, socket, subprocess, sys
serial, port = sys.argv[1], sys.argv[2]
token = os.environ["HOTPATCH_TOKEN"]
fwd = subprocess.run(["adb", "-s", serial, "forward", "tcp:0", f"tcp:{port}"],
                     capture_output=True, text=True).stdout.strip()
try:
    local = int(fwd)
except ValueError:
    sys.exit(print("info: adb forward failed"))
try:
    s = socket.create_connection(("127.0.0.1", local), timeout=10)
    f = s.makefile("rwb")
    def call(i, method, params):
        f.write((json.dumps({"jsonrpc": "2.0", "id": i, "method": method, "params": params})
                 + "\n").encode())
        f.flush()
        while True:
            raw = f.readline()
            if not raw:
                raise RuntimeError("connection closed")
            msg = json.loads(raw)
            if msg.get("id") == i:
                if "error" in msg:
                    raise RuntimeError(msg["error"].get("message"))
                return msg.get("result")
    call(1, "handshake", {"token": token})
    r = call(2, "hotpatch_info", None)
    s.close()
    print(f"info: patches_applied={r['patches_applied']} patch_bytes_loaded={r['patch_bytes_loaded']}"
          f" anchor_runtime=0x{r['anchor_runtime']:x} pid={r['pid']}"
          f" pending_layout_mismatches={len(r['pending_layout_mismatches'])}")
except Exception as e:  # a failed read is a result, never fatal
    print(f"info: hotpatch_info failed: {e}")
finally:
    subprocess.run(["adb", "-s", serial, "forward", "--remove", f"tcp:{local}"],
                   capture_output=True)
PY
}

# `adb exec-out screencap -p` (Windows: win_screencap) into SCREENCAP_DIR/<$1>.png, when
# SCREENCAP_DIR is set.
screencap() {
  [ -n "$SCREENCAP_DIR" ] || return 0
  if [ "$WINDOWS" = 1 ]; then win_screencap "$1"; return 0; fi
  if adb_ exec-out screencap -p > "${SCREENCAP_DIR%/}/$1.png" 2>/dev/null; then
    echo "screencap: ${SCREENCAP_DIR%/}/$1.png"
  else
    echo "screencap: failed"
  fi
}

# ---- Windows leg session helpers (--windows) ----------------------------------------------------
WIN_RUN_N=0

# Runs `<WINDOWS_ROOT>\<script>.ps1 <args...> <out>` as the one-shot scheduled task `$1` (on the
# interactive desktop: the ssh logon has none), waits up to 30 s for rig file `$2` (the script's last
# argument) and prints `ok` or `missing`. `$3` is the script, the rest its leading arguments.
win_desktop_task() {
  local task="$1" out="$2" script="$3"
  shift 3
  win_ps '$tr = "powershell -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File " + $A[2]
    for ($i = 3; $i -lt $A.Count; $i++) { $tr += " " + $A[$i] }
    $tr += " " + $A[1]
    Remove-Item -LiteralPath $A[1] -ErrorAction SilentlyContinue
    & schtasks.exe /create /sc once /st 23:59 /f /tn $A[0] /tr $tr 2>&1 | Out-Null
    & schtasks.exe /run /tn $A[0] 2>&1 | Out-Null
    $d = (Get-Date).AddSeconds(30)
    while (-not (Test-Path -LiteralPath $A[1]) -and (Get-Date) -lt $d) { Start-Sleep -Milliseconds 200 }
    & schtasks.exe /delete /tn $A[0] /f 2>&1 | Out-Null
    if (Test-Path -LiteralPath $A[1]) { "ok" } else { "missing" }' \
    "$task" "$out" "${WINDOWS_ROOT}\\${script}" "$@" < /dev/null
}

# A desktop capture of the rig into SCREENCAP_DIR/<$1>.png (shot.ps1, then copied back). A rig whose
# user session is disconnected or locked captures a blank screen: the `ui:` line is then the
# evidence of what the window shows.
win_screencap() {
  local out="${WINDOWS_ROOT}\\shots\\$1-$$.png"
  if [ "$(win_desktop_task "${WINDOWS_TASK}shot" "$out" shot.ps1)" = "ok" ] \
    && win_get "$out" > "${SCREENCAP_DIR%/}/$1.png" && [ -s "${SCREENCAP_DIR%/}/$1.png" ]; then
    echo "screencap: ${SCREENCAP_DIR%/}/$1.png"
  else
    echo "screencap: failed"
  fi
}

# `ui: ...`: the text elements of pid `$1`'s window as UI Automation reports them (the app's
# AccessKit tree: the counter, the sentinels), read by ui.ps1 on the interactive desktop; with `$2`
# > 0 it first presses the `Increment` button that many times (InvokePattern, no pointer input).
win_ui() {
  local out="${WINDOWS_ROOT}\\ui-$$.txt"
  if [ "$(win_desktop_task "${WINDOWS_TASK}ui" "$out" ui.ps1 "$1" "${2:-0}")" = "ok" ]; then
    win_get "$out" | tr -d '\r' | sed 's/^/ui: /'
  else
    echo "ui: no answer from ui.ps1 within 30 s"
  fi
}

# `counters: ...`: what the rig shows of the app's patch counters. The CLI echoes the discovery
# line with its token redacted, so `hotpatch_info` cannot be asked from outside. Instead: the app's
# own `frust-devtools: applying patch <n> (<len> bytes, ...)` lines (debug level: FRUST_LOG=debug,
# counted after log line `$1`, so a fresh run's from 0), and the session dir's patch-N.dll images.
win_patch_counters() {
  local app
  app="$(tail -n +"$((${1:-0} + 1))" "$LOG" | awk '
    match($0, /frust-devtools: applying patch [0-9]+ \([0-9]+ bytes/) {
      split(substr($0, RSTART, RLENGTH), w, /[ (]+/); n++; last = w[4]; sum += w[5] }
    END { if (n) printf "app (debug lines): %d applying lines, last patch %s, %.0f bytes in all", n, last, sum
          else printf "app: no applying lines (debug level: FRUST_LOG=debug shows them)" }')"
  echo "counters: ${app}; $(win_ps '$d = Join-Path $A[0] ("frust-hotpatch\" + $A[1])
    $f = @(Get-ChildItem -LiteralPath $d -Filter "patch-*.dll" -File -ErrorAction SilentlyContinue)
    "session dir: $($f.Count) patch-N.dll, $(($f | Measure-Object Length -Sum).Sum + 0) bytes in all"' \
    "$APP_TARGET_DIR" "$SESSION_DIR_NAME" < /dev/null)"
}

# --frust-run on the rig: the session dir's newest `stub-N.o` / `patch-N.dll` written after the save
# at `$1` (the rig's unix ms), stamped on the rig's clock.
win_host_artifacts() {
  win_ps '$d = Join-Path $A[0] ("frust-hotpatch\" + $A[1]); $s = [int64]$A[2]; $best = @{}
    foreach ($f in @(Get-ChildItem -LiteralPath $d -File -ErrorAction SilentlyContinue)) {
      if ($f.Name -match "^(stub|patch)-(\d+)\.(o|dll)$") {
        $t = [DateTimeOffset]::new($f.LastWriteTimeUtc).ToUnixTimeMilliseconds()
        $k = $Matches[1]; $n = [int]$Matches[2]
        if ($t -gt $s -and (-not $best.ContainsKey($k) -or $n -gt $best[$k][0])) { $best[$k] = @($n, $f.Name, $t, $f.Length) }
      }
    }
    $parts = @()
    if ($best.ContainsKey("stub")) { $parts += "$($best.stub[1]) at save+$($best.stub[2] - $s)" }
    if ($best.ContainsKey("patch")) { $parts += "$($best.patch[1]) at save+$($best.patch[2] - $s) ($($best.patch[3]) bytes)" }
    if ($parts.Count) { "host: " + ($parts -join ", ") } else { "host: no stub/patch written after the save" }' \
    "$APP_TARGET_DIR" "$SESSION_DIR_NAME" "$1" < /dev/null
}

# The stamp of the first log line after line `$1` matching ERE `$2` whose stamp is later than `$3`,
# waiting at most `$4` s (0: look once). On the Windows leg the stamp is the tailer's read time.
log_line_ms() {
  local deadline=$(( $(date +%s) + $4 )) t
  while :; do
    t="$(tail -n +"$(($1 + 1))" "$LOG" | awk -v re="$2" -v s="$3" '$0 ~ re && $1 + 0 > s + 0 { print $1; exit }')"
    if [ -n "$t" ]; then echo "$t"; return 0; fi
    [ "$(date +%s)" -lt "$deadline" ] || return 1
    sleep 0.2
  done
}

# Starts `frust run --watch` on the rig's interactive desktop (run.cmd as a one-shot scheduled task)
# and the log tailer here; RUNNER_PID is the tailer's local process group.
win_start_runner() {
  local mode_args="" cmd_text
  WIN_RUN_N=$((WIN_RUN_N + 1))
  local row="${ROW:-run}"
  WIN_LOG="${WINDOWS_ROOT}\\watch-${row//[^A-Za-z0-9_-]/_}-$$-${WIN_RUN_N}.log"
  [ "$MODE" = "frust-restart" ] && mode_args=" --no-hot --features frust/hotpatch"
  cmd_text="@echo off"$'\r\n'"cd /d ${APP} || exit /b 1"$'\r\n'
  cmd_text="${cmd_text}${FRUST} run --watch${mode_args}${FRUST_RUN_ARGS:+ ${FRUST_RUN_ARGS}} > ${WIN_LOG} 2>&1"$'\r\n'
  win_ps 'New-Item -ItemType Directory -Force -Path $A[0], (Join-Path $A[0] "shots") | Out-Null
    Remove-Item -LiteralPath $A[1], ($A[1] + ".stop") -ErrorAction SilentlyContinue' \
    "$WINDOWS_ROOT" "$WIN_LOG" < /dev/null
  printf '%s' "$cmd_text" | win_put "${WINDOWS_ROOT}\\run.cmd" > /dev/null
  printf '%s' "$WIN_UI_PS" | win_put "${WINDOWS_ROOT}\\ui.ps1" > /dev/null
  [ -n "$SCREENCAP_DIR" ] && printf '%s' "$WIN_SHOT_PS" | win_put "${WINDOWS_ROOT}\\shot.ps1" > /dev/null
  echo "runner: run.cmd as scheduled task ${WINDOWS_TASK}: $(printf '%s' "$cmd_text" | tail -1 | tr -d '\r')" >> "$LOG"
  win_ps '& schtasks.exe /create /sc once /st 23:59 /f /tn $A[0] /tr ("cmd /c " + $A[1]) 2>&1 | Out-Null
    & schtasks.exe /run /tn $A[0]' "$WINDOWS_TASK" "${WINDOWS_ROOT}\\run.cmd" < /dev/null >> "$LOG"
  set -m
  win_ps_raw "$WIN_TAIL_PS" "$WIN_LOG" "${WIN_LOG}.stop" < /dev/null >> "$LOG" 2>&1 &
  RUNNER_PID=$!
  set +m
}

# Kills the CLI at FRUST and the app's processes under APP_TARGET_DIR on the rig (`taskkill /T /F`,
# the watch loop's own kill route), deletes the scheduled task, stops the tailer and redacts the
# devtools token in the rig's copy of the log.
win_stop_runner() {
  win_ps 'foreach ($p in @(Get-Process -Name frust -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $A[0] })) {
      & taskkill.exe /T /F /PID $p.Id 2>&1 | Out-Null }
    foreach ($p in @(Get-Process -Name $A[1] -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and $_.Path.StartsWith($A[2] + "\", [StringComparison]::OrdinalIgnoreCase) })) {
      & taskkill.exe /T /F /PID $p.Id 2>&1 | Out-Null }
    & schtasks.exe /delete /tn $A[3] /f 2>&1 | Out-Null
    New-Item -ItemType File -Force -Path ($A[4] + ".stop") | Out-Null
    Start-Sleep -Milliseconds 1500
    if (Test-Path -LiteralPath $A[4]) {
      $t = [IO.File]::ReadAllText($A[4])
      [IO.File]::WriteAllText($A[4], ($t -replace "(listening on [0-9]+ token )[0-9a-fA-F]+", "`$1<redacted>")) }' \
    "$FRUST" "$APP_BIN" "$APP_TARGET_DIR" "$WINDOWS_TASK" "$WIN_LOG" < /dev/null
  local i=0
  while kill -0 "$RUNNER_PID" 2>/dev/null && [ $i -lt 50 ]; do sleep 0.1; i=$((i + 1)); done
  kill -0 "$RUNNER_PID" 2>/dev/null && kill -TERM -- "-${RUNNER_PID}" 2>/dev/null
  wait "$RUNNER_PID" 2>/dev/null
  if [ -n "$(win_app_procs)" ]; then echo "warning: an app process under ${APP_TARGET_DIR} is still running" >&2; fi
  RUNNER_PID=""
  APP_PID=""
}

# Writes every restored mirror source back on the rig and compares it there by SHA-256.
win_restore_sources() {
  local file copy want got
  while IFS=$'\t' read -r file copy; do
    win_put "$(win_rig_path_of "$file")" < "$copy" > /dev/null
    want="$(shasum -a 256 "$copy" | cut -c1-64)"
    got="$(win_ps '(Get-FileHash -Algorithm SHA256 -LiteralPath $A[0]).Hash.ToLower()' \
      "$(win_rig_path_of "$file")" < /dev/null)"
    if [ "$want" = "$got" ]; then
      echo "  same: $(win_rig_path_of "$file")"
    else
      echo "  DIFFERS: $(win_rig_path_of "$file")"
    fi
  done < "$RESTORE_LIST"
}

# Rewrite `$1` in place with the content of `$2` (same inode, no rename: dx watches for in-place
# modification and a temp-file rename makes it patch off stale content).
write_in_place() {
  python3 - "$1" "$2" <<'PY'
import sys
dst, src = sys.argv[1], sys.argv[2]
new = open(src).read()
with open(dst, "r+") as f:
    if f.read() != new:
        f.seek(0)
        f.write(new)
        f.truncate()
PY
}

# Apply run `$3`'s edit for target `$2` to `$1` (in place, derived from the pristine copy `$4`),
# then print the save time in unix ms.
apply_edit() {
  python3 - "$1" "$2" "$3" "$4" "$STATE_FIELD_WRITE" "$RETURN_TYPE_WRAP" "$FRUST_MODE" <<'PY'
import re, sys, time
path, target, run, orig = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
write_field = sys.argv[5] == "1"
wrap = sys.argv[6]
# --frust-run / --frust-restart: the prepared `frust create` app, cumulative hazard edits.
frust = sys.argv[7] == "1"
src = open(orig).read().split("\n")

def replace_marked(lines, marker, fn):
    hits = [i for i, l in enumerate(lines) if l.rstrip().endswith(marker)]
    if len(hits) != 1:
        sys.exit(f"measure.sh: expected one line marked {marker}, found {len(hits)}")
    i = hits[0]
    indent = lines[i][: len(lines[i]) - len(lines[i].lstrip())]
    lines[i] = indent + fn(lines[i].strip())

bump = lambda l: re.sub(r"v\d+\"", f'v{run}"', l, count=1)
if target == "home":
    replace_marked(src, "// SENTINEL-HOME", bump)
elif target == "card":
    replace_marked(src, "// SENTINEL-CARD", bump)
elif target == "helper":
    replace_marked(src, "// SENTINEL-HOME",
                   lambda _: f"let sentinel = text(spike_helper_{run}()); // SENTINEL-HOME")
    src[-1:] = ["",
                f"/// Added by measure.sh --target helper (run {run}).",
                f"fn spike_helper_{run}() -> String {{",
                f'    "hotpatch-helper: v{run}".to_string()',
                "}", ""]
elif target == "state-type":
    # A new State TYPE: the seam's call_it instance changes its generic arguments (RESULTS.md row D).
    n = run + 1
    replace_marked(src, "// STATE-TYPE",
                   lambda _: f"type State = ({', '.join(['u32'] * n)}); // STATE-TYPE")
    replace_marked(src, "// STATE-INIT", lambda _: f"({', '.join(['0'] * n)}) // STATE-INIT")
    replace_marked(src, "// STATE-READ", lambda _: "let count = state.0; // STATE-READ")
    if frust:
        replace_marked(src, "// STATE-INC", lambda _: "state.0 += 1 // STATE-INC")
    else:
        replace_marked(src, "// STATE-INC",
                       lambda _: 'button("Increment", |state: &mut PageState| state.0 += 1) // STATE-INC')
    # `HomeState` is now unused; keep the patch warning-free.
    src.insert(next(i for i, l in enumerate(src) if l.startswith("pub struct HomeState")),
               "#[allow(dead_code)]")
    replace_marked(src, "// SENTINEL-HOME", bump)
elif target in ("state-field", "either-swap") and frust:
    # Cumulative (rows D2 and D5): run N adds `extra1`..`extraN`, so it changes the layout again
    # against the base a restart relaunched from run N-1's edit. either-swap also flips SHOW_HOME,
    # so the root's `either(..)` swaps arms in the same patch.
    fields = "".join(f"\n    pub extra{i}: u32," for i in range(1, run + 1))
    replace_marked(src, "// STATE-FIELDS",
                   lambda l: l.replace(" // STATE-FIELDS", "") + fields + " // STATE-FIELDS")
    inits = ", ".join(f"extra{i}: 4242" for i in range(1, run + 1))
    replace_marked(src, "// STATE-INIT", lambda _: f"HomeState {{ count: 0, {inits} }} // STATE-INIT")
    replace_marked(src, "// SENTINEL-HOME", lambda _: (
        f"let sentinel = {{ log::info!(\"frust-hotpatch-spike: {target} v{run} build ran "
        f"extra1={{}}\", state.extra1); text(format!(\"hotpatch-sentinel: v{run} extra1={{}}\", "
        f"state.extra1)) }}; // SENTINEL-HOME"))
    if target == "either-swap":
        show = "false" if run % 2 else "true"
        replace_marked(src, "// SHOW-HOME", lambda _: f"pub const SHOW_HOME: bool = {show}; // SHOW-HOME")
elif target == "state-field":
    # Same State type (same def-path, same symbol), new layout (RESULTS.md row D2).
    replace_marked(src, "// STATE-FIELDS",
                   lambda l: l.replace(" // STATE-FIELDS", "") + "\n    pub extra: u32, // STATE-FIELDS")
    replace_marked(src, "// STATE-INIT",
                   lambda _: "HomeState { count: 0, extra: 4242 } // STATE-INIT")
    write = "state.extra = state.extra.wrapping_add(1); " if write_field else ""
    replace_marked(src, "// SENTINEL-HOME", lambda _: (
        f"let sentinel = {{ {write}log::info!(\"frust-hotpatch-spike: state-field v{run} build ran "
        f"extra={{}}\", state.extra); text(format!(\"hotpatch-sentinel: v{run} extra={{}}\", "
        f"state.extra)) }}; // SENTINEL-HOME"))
elif target == "return-type" and frust:
    # Cumulative (row D3): run N wraps the root `scaffold(..)` of `HomePage::build` in N `stack()`s,
    # a new concrete return type every run.
    replace_marked(src, "// ROOT-OPEN",
                   lambda l: "stack().child(" * run + l)
    replace_marked(src, "// ROOT-CLOSE",
                   lambda l: l.replace(" // ROOT-CLOSE", ")" * run + " // ROOT-CLOSE"))
    replace_marked(src, "// USE-FRUST", lambda l: l.replace("scaffold, text}", "scaffold, stack, text}"))
    replace_marked(src, "// SENTINEL-HOME", bump)
elif target == "stock-label":
    # The generated template's own label (row A on the pristine app, row H).
    label = 'text("You have pushed the button this many times:")'
    hits = [i for i, l in enumerate(src) if label in l]
    if len(hits) != 1:
        sys.exit(f"measure.sh: expected one line with {label}, found {len(hits)}")
    src[hits[0]] = src[hits[0]].replace(label, f'text("You have pushed the button this many times: v{run}")')
elif target == "badge":
    # Row D4 in pairs: run 2k-1 adds `Badge<k>` (a type the accepted set has never seen, so the
    # patch passes and joins the set), run 2k changes its layout against that accepted patch.
    k = (run + 1) // 2
    fields = ["    pub n: u32,"] + (["    pub m: u32,"] if run % 2 == 0 else [])
    at = next(i for i, l in enumerate(src) if l.startswith("pub struct HomeState"))
    while at > 0 and src[at - 1].startswith("///"):
        at -= 1
    src[at:at] = [f"/// Added by measure.sh --target badge (run {run}).",
                  f"pub struct Badge{k} {{", *fields, "}", ""]
    if run % 2:
        sentinel = (f"let badge = Badge{k} {{ n: {run} }}; let sentinel = "
                    f"text(format!(\"badge{k}: n={{}}\", badge.n)); // SENTINEL-HOME")
    else:
        sentinel = (f"let badge = Badge{k} {{ n: {run}, m: 7 }}; let sentinel = "
                    f"text(format!(\"badge{k}: n={{}} m={{}}\", badge.n, badge.m)); // SENTINEL-HOME")
    replace_marked(src, "// SENTINEL-HOME", lambda _: sentinel)
elif target == "framework":
    # Row E: any content change in a file of the app's frust path dependency.
    at = len(src) - 1 if src and src[-1] == "" else len(src)
    src.insert(at, f"// measure.sh framework edit v{run}")
elif target == "return-type":
    # Wrap the root `column()` of `HomePage::build` in another container (RESULTS.md row D3). The
    # sentinel is bumped too, so a taken patch is visible on screen. No `log` line is added.
    if wrap not in ("stack", "column"):
        sys.exit(f"measure.sh: RETURN_TYPE_WRAP must be stack or column, got {wrap}")
    opens = [i for i, l in enumerate(src) if l == "        column()"]
    closes = [i for i, l in enumerate(src) if l == "            .cross_axis(CrossAxisAlignment::Center)"]
    if len(opens) != 1 or len(closes) != 1 or closes[0] < opens[0]:
        sys.exit("measure.sh: expected one root `column()` ... `.cross_axis(..)` chain in build")
    src[opens[0]] = f"        {wrap}().child(column()"
    src[closes[0]] += ")"
    if wrap == "stack":
        uses = [i for i, l in enumerate(src) if l.startswith("use frust::{")]
        if len(uses) != 1:
            sys.exit("measure.sh: expected one `use frust::{..}` line")
        src[uses[0]] = src[uses[0]].replace("component, text}", "component, stack, text}")
    replace_marked(src, "// SENTINEL-HOME", bump)
new = "\n".join(src)
with open(path, "r+") as f:
    f.seek(0)
    f.write(new)
    f.truncate()
print(int(time.time() * 1000))
PY
}

# --frust-run / --frust-restart: runs its arguments, prefixing every output line with its arrival
# time in unix ms. It leads the runner's process group; the frust CLI runs in that group, and the
# app the CLI spawns gets a group of its own. SIGTERM is passed on as the SIGINT the CLI's Ctrl-C
# handler answers (it kills the app, then exits).
TIMESTAMP_PY='
import signal, subprocess, sys, time
child = subprocess.Popen(sys.argv[1:], stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                         stderr=subprocess.STDOUT)
signal.signal(signal.SIGINT, signal.SIG_IGN)
signal.signal(signal.SIGTERM, lambda *_: child.send_signal(signal.SIGINT))
for raw in child.stdout:
    sys.stdout.write("%d %s" % (int(time.time() * 1000), raw.decode("utf-8", "replace")))
    sys.stdout.flush()
sys.exit(child.wait())
'

# Start the runner in its own process group (so one kill reaches dx/cargo and the app they spawn).
start_runner() {
  if [ "$WINDOWS" = 1 ]; then win_start_runner; return 0; fi
  set -m
  if [ "$MODE" = "hotpatch" ]; then
    (cd "$RUNNER_DIR" && exec "$DX" serve --hot-patch --platform desktop --interactive false \
      --verbose) >> "$LOG" 2>&1 < /dev/null &
  elif [ "$MODE" = "restart" ]; then
    (cd "$RUNNER_DIR" && exec cargo run -p hotpatch-spike) >> "$LOG" 2>&1 < /dev/null &
  else
    local -a cmd=("$FRUST" run --watch)
    [ "$MODE" = "frust-restart" ] && cmd+=(--no-hot --features frust/hotpatch)
    [ "$ANDROID" = 1 ] && cmd+=(-d "$SERIAL")
    # Word-split on purpose: FRUST_RUN_ARGS holds plain words (e.g. `--define KEY=VALUE`).
    # shellcheck disable=SC2206
    [ -n "$FRUST_RUN_ARGS" ] && cmd+=($FRUST_RUN_ARGS)
    echo "runner: ${cmd[*]} (in ${APP})" >> "$LOG"
    (cd "$APP" && exec python3 -u -c "$TIMESTAMP_PY" "${cmd[@]}") >> "$LOG" 2>&1 < /dev/null &
  fi
  RUNNER_PID=$!
  set +m
}

# --frust-run / --frust-restart: PIDs of the app's processes, oldest first. The app is the
# executable `<APP_TARGET_DIR>/.../<package name>` (the fat image under `frust-hotpatch/fat/..` in hot
# mode, `debug/<name>` with --no-hot), matched by its command's first word, so only this scratch
# app's processes are ever found. `cargo run` starts it by a path relative to the app dir.
frust_app_pids() {
  if [ "$ANDROID" = 1 ]; then device_pids; return 0; fi
  if [ "$WINDOWS" = 1 ]; then win_app_procs | awk '{ print $1 }'; return 0; fi
  ps -axo pid=,command= | awk -v pre="${APP_TARGET_DIR%/}/" -v bin="/${APP_BIN}" -v app="$APP" '
    { exe = $2; if (substr(exe, 1, 1) != "/") exe = app "/" exe }
    index(exe, pre) == 1 && substr(exe, length(exe) - length(bin) + 1) == bin { print $1 }'
}

# TERM the runner's process group, wait up to 5 s, then KILL whatever is left. Every KILL is guarded:
# the group only while some member of it still exists (`kill -0 -- -PID`, re-checked right before
# the KILL; bash reaps the leader on SIGCHLD, so the only window left is a reused pgid with a
# brand-new leader — negligible), and the scraped app PID only while it is still in the runner's
# process group (`ps -o pgid=`) — a stale or reused PID from the log is never killed.
stop_runner() {
  [ -n "$RUNNER_PID" ] || return 0
  if [ "$WINDOWS" = 1 ]; then win_stop_runner; return 0; fi
  if [ "$FRUST_MODE" = 1 ]; then
    # SIGINT to the CLI (its Ctrl-C handler kills the app it spawned), via the timestamper.
    kill -TERM "$RUNNER_PID" 2>/dev/null
    local j=0
    while kill -0 "$RUNNER_PID" 2>/dev/null && [ $j -lt 100 ]; do sleep 0.1; j=$((j + 1)); done
  fi
  kill -TERM -- "-${RUNNER_PID}" 2>/dev/null || kill -TERM "$RUNNER_PID" 2>/dev/null
  local i=0
  while kill -0 -- "-${RUNNER_PID}" 2>/dev/null && [ $i -lt 50 ]; do sleep 0.1; i=$((i + 1)); done
  if kill -0 -- "-${RUNNER_PID}" 2>/dev/null; then kill -KILL -- "-${RUNNER_PID}" 2>/dev/null; fi
  # (A device PID is not a host process: the Android leg never signals it.)
  if [ "$ANDROID" != 1 ] && [ -n "$APP_PID" ] && kill -0 "$APP_PID" 2>/dev/null; then
    local pgid
    pgid="$(ps -o pgid= -p "$APP_PID" 2>/dev/null | tr -d ' ')"
    if [ -z "$pgid" ]; then
      : # already gone (the group KILL above reached it; `kill -0` raced its exit)
    elif [ "$pgid" = "$RUNNER_PID" ]; then
      kill -KILL "$APP_PID" 2>/dev/null
    else
      echo "warning: app pid ${APP_PID} is not in the runner's process group" \
        "(pgid ${pgid} != ${RUNNER_PID}); not killing it" >&2
    fi
  fi
  wait "$RUNNER_PID" 2>/dev/null
  if [ "$ANDROID" = 1 ]; then
    # The CLI's Ctrl-C force-stops the package; a device app that outlived it is stopped here
    # (this scratch package only).
    if [ -n "$(device_pids)" ]; then
      echo "note: ${ANDROID_PACKAGE} outlived the CLI; am force-stop" >&2
      adb_ shell am force-stop "$ANDROID_PACKAGE" >/dev/null 2>&1
    fi
  elif [ "$FRUST_MODE" = 1 ]; then
    # The app runs in its own process group; anything the CLI did not take down (e.g. a child of
    # an interrupted first fat build) is this scratch app's executable, so kill it by path.
    local pid
    for pid in $(frust_app_pids); do
      echo "note: app pid ${pid} outlived the CLI; killing it" >&2
      kill -KILL "$pid" 2>/dev/null
    done
  fi
  RUNNER_PID=""
  APP_PID=""
}

# Replaces the devtools token of every discovery line in the saved logcat (the port stays), so
# the kept log dir holds no live credential.
redact_devlog_tokens() {
  local f="${1:-$DEVLOG}"
  [ -f "$f" ] || return 0
  sed -E 's/(listening on [0-9]+ token )[0-9a-fA-F]+/\1<redacted>/' "$f" > "${f}.redacted" \
    && mv "${f}.redacted" "$f"
}

cleanup() {
  stop_runner
  if [ -n "$DEVLOG_PID" ]; then
    kill "$DEVLOG_PID" 2>/dev/null
    wait "$DEVLOG_PID" 2>/dev/null
    DEVLOG_PID=""
    redact_devlog_tokens
    echo
    echo "Device log since ${DEVLOG_SINCE} (device clock): ${DEVLOG}"
    # The app runs as untrusted_app; other domains' denials (adb's shell, system_server) are
    # counted but not the app's.
    echo "Frame-wait backstop lines ('${BACKSTOP_RE}') in it: $(grep -c "$BACKSTOP_RE" "$DEVLOG")"
    echo "SELinux denials in it: $(grep -c 'avc: *denied' "$DEVLOG") line(s), of which" \
      "$(grep 'avc: *denied' "$DEVLOG" | grep -c 'scontext=u:r:untrusted_app') with" \
      "scontext untrusted_app (first 5 of those):"
    grep 'avc: *denied' "$DEVLOG" | grep 'scontext=u:r:untrusted_app' | cut -c1-240 | head -5 \
      | sed 's/^ */  /'
    echo "Crash buffer since then:"
    # `-d` returns at once while the device is attached; with it gone, adb would wait for it.
    if [ "$(adb_ get-state 2>/dev/null)" != "device" ]; then
      echo "  (device not attached: crash buffer not read)"
    else
      adb_ logcat -d -b crash -v epoch -T "$DEVLOG_SINCE" 2>/dev/null | grep -v '^-----' \
        | cut -c1-240 | head -10 | sed 's/^ */  /' | grep . || echo "  (empty)"
    fi
  fi
  local file copy
  while IFS=$'\t' read -r file copy; do
    write_in_place "$file" "$copy"
  done < "$RESTORE_LIST"
  echo
  if [ "$WINDOWS" = 1 ]; then
    # Belt and braces: the CLI already redacts the discovery line's token.
    redact_devlog_tokens "$LOG"
    echo "Restored edited sources on the rig (SHA-256 against the pristine copies):"
    win_restore_sources
  elif [ "$FRUST_MODE" = 1 ]; then
    echo "Restored edited sources (cmp against the pristine copies):"
    while IFS=$'\t' read -r file copy; do
      if cmp -s "$file" "$copy"; then echo "  same: ${file}"; else echo "  DIFFERS: ${file}"; fi
    done < "$RESTORE_LIST"
  else
    echo "Restored edited sources. git status (expect nothing under app/):"
    git -C "$SCRIPT_DIR" status --short -- . | grep -v '^?? target/' || echo "  (clean)"
  fi
  echo "Logs: ${LOG_DIR}"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# First `frust-hotpatch: <kind>` line after log line `$2` whose t_unix_ms exceeds `$3`, waiting at
# most `$4` seconds. Prints the t_unix_ms, or nothing on timeout / app exit.
wait_for() {
  local kind="$1" from_line="$2" after_ms="$3" timeout_s="$4"
  local deadline=$(( $(date +%s) + timeout_s )) t
  while [ "$(date +%s)" -lt "$deadline" ]; do
    t="$(tail -n +"$((from_line + 1))" "$LOG" \
      | grep -o "frust-hotpatch: ${kind} t_unix_ms=[0-9]*" | sed 's/.*=//' \
      | awk -v s="$after_ms" '$1 > s { print; exit }')"
    if [ -n "$t" ]; then echo "$t"; return 0; fi
    # A refused apply never produces an `applied` line: stop waiting for one.
    if [ "$kind" = "applied" ] && [ -n "$(refused_variant "$from_line")" ]; then return 1; fi
    # (On the Windows rig a liveness probe is an ssh call: the timeout bounds the wait instead.)
    if [ "$WINDOWS" != 1 ] && [ -n "$APP_PID" ] && ! app_alive "$APP_PID"; then return 1; fi
    if ! kill -0 "$RUNNER_PID" 2>/dev/null; then return 1; fi
    sleep 0.1
  done
  return 1
}

# The variant named by the first `frust-hotpatch: refused <variant>` line after log line `$1`, or
# nothing.
refused_variant() {
  tail -n +"$(($1 + 1))" "$LOG" | grep -o 'frust-hotpatch: refused [A-Za-z]*' | head -1 \
    | sed 's/.*refused //'
}

log_lines() {
  wc -l < "$LOG" | tr -d ' '
}

# The app's PID, once its first frame is logged: dx logs it on the devtools connection; in restart
# mode `cargo run` exec()s the binary on Unix, so it is the runner PID itself.
find_app_pid() {
  if [ "$MODE" = "hotpatch" ]; then
    APP_PID="$(grep -o 'pid: Some([0-9]*)' "$LOG" | tail -1 | grep -o '[0-9][0-9]*')"
  elif [ "$FRUST_MODE" = 1 ]; then
    APP_PID="$(frust_app_pids | tail -1)"
  else
    APP_PID="$RUNNER_PID"
  fi
}

# The `extra=<v>` the patched state-field build of run `$2` logged after log line `$1`, waiting at
# most `$3` seconds. Prints the value, or nothing.
wait_build_line() {
  local from_line="$1" run_n="$2" deadline=$(( $(date +%s) + $3 )) v
  while :; do
    v="$(tail -n +"$((from_line + 1))" "$LOG" \
      | grep -o "frust-hotpatch-spike: state-field v${run_n} build ran extra=[0-9]*" \
      | head -1 | sed 's/.*=//')"
    if [ -n "$v" ] || [ "$(date +%s)" -ge "$deadline" ]; then echo "$v"; return 0; fi
    sleep 0.1
  done
}

# Echo log lines after `$1` that look like an app failure (panic, abort, signal, guard malloc).
trouble_lines() {
  tail -n +"$(($1 + 1))" "$LOG" \
    | grep -iE 'panick|abort|sigsegv|sigbus|sigabrt|sigill|signal [0-9]|guardmalloc|segmentation|bus error|exit (code|status)|exited|crash' \
    | tr -d '\033' | sed -e 's/\[[0-9;]*m//g' -e 's/^ */    log: /' | cut -c1-200 | head -5
}

# Process survival after a hot-patch run: is the app PID found at startup alive, and is it still the
# PID the log names last (a relaunch by dx would log a new `pid: Some(..)`)?
pid_state() {
  local latest
  if [ "$FRUST_MODE" = 1 ]; then
    latest="$(frust_app_pids | tail -1)"
  else
    latest="$(grep -o 'pid: Some([0-9]*)' "$LOG" | tail -1 | grep -o '[0-9][0-9]*')"
  fi
  if [ -z "$APP_PID" ]; then
    echo "app pid unknown"
  elif ! kill -0 "$APP_PID" 2>/dev/null; then
    echo "app pid ${APP_PID} GONE"
  elif [ -n "$latest" ] && [ "$latest" != "$APP_PID" ]; then
    echo "app pid ${APP_PID} alive, but the log now names pid ${latest} (relaunch?)"
  else
    echo "app pid ${APP_PID} alive (same)"
  fi
}

# --frust-run: the first CLI outcome line after log line `$1`, waiting at most `$2` seconds. Prints
# `<arrival unix ms> <line>`, or nothing on timeout.
wait_outcome() {
  local from_line="$1" deadline=$(( $(date +%s) + $2 )) hit
  while [ "$(date +%s)" -lt "$deadline" ]; do
    hit="$(tail -n +"$((from_line + 1))" "$LOG" | grep -E '^[0-9]+ (patched in [0-9]+ ms|restart required: |compile failed|hot build failed|hot reload unavailable|the app (exited|failed))' | head -1)"
    if [ -n "$hit" ]; then echo "$hit"; return 0; fi
    if ! kill -0 "$RUNNER_PID" 2>/dev/null; then return 1; fi
    sleep 0.1
  done
  return 1
}

# Resident set size of pid `$1` in MB (one decimal), or `?`.
rss_mb() {
  local kb
  if [ "$ANDROID" = 1 ]; then
    kb="$(adb_ shell cat "/proc/$1/status" 2>/dev/null | tr -d '\r' | awk '/^VmRSS:/ { print $2 }')"
  elif [ "$WINDOWS" = 1 ]; then
    # The working set (`WorkingSet64`), Windows' resident set.
    kb="$(win_app_procs | awk -v p="$1" '$1 == p { printf "%d", $2 / 1024 }')"
  else
    kb="$(ps -o rss= -p "$1" 2>/dev/null | tr -d ' ')"
  fi
  if [ -n "$kb" ]; then awk -v k="$kb" 'BEGIN { printf "%.1f", k / 1024 }'; else echo "?"; fi
}

# --frust-run: the session dir's newest `stub-N.o` / `patch-N.<ext>` written after the save at `$1`
# (unix ms): `host: stub-N.o at save+<ms>, patch-N.dylib at save+<ms> (<bytes> bytes, mode <octal>)`.
host_artifacts() {
  python3 - "${APP_TARGET_DIR%/}/frust-hotpatch/${SESSION_DIR_NAME}" "$1" <<'PY'
import os, re, sys
d, stamp = sys.argv[1], int(sys.argv[2])
best = {}
try:
    names = os.listdir(d)
except OSError:
    names = []
for n in names:
    # `patch-N.upload.so`: the stripped copy an Android session sends (H2-06).
    m = re.fullmatch(r"(stub|patch)-(\d+)\.(o|dylib|so|dll|upload\.so)", n)
    if not m:
        continue
    kind = "upload" if m.group(3) == "upload.so" else m.group(1)
    st = os.stat(os.path.join(d, n))
    t = int(st.st_mtime * 1000)
    if t > stamp and (kind not in best or int(m.group(2)) > best[kind][0]):
        best[kind] = (int(m.group(2)), n, t, st.st_size, st.st_mode & 0o777)
parts = []
if "stub" in best:
    parts.append(f"{best['stub'][1]} at save+{best['stub'][2] - stamp}")
for kind in ("patch", "upload"):
    if kind in best:
        _, n, t, size, mode = best[kind]
        parts.append(f"{n} at save+{t - stamp} ({size} bytes, mode {mode:o})")
print("host: " + (", ".join(parts) if parts else "no stub/patch written after the save"))
PY
}

# --frust-run: whether the app logged the loopback hand-off after log line `$1` (a debug line).
handoff_line() {
  local hit
  hit="$(tail -n +"$(($1 + 1))" "$LOG" \
    | grep -o 'frust-devtools: patch file handed off ([0-9]* bytes, checks passed)' | head -1)"
  if [ -n "$hit" ]; then
    echo "hand-off: file (app logged '${hit}')"
  else
    echo "hand-off: not logged (a debug line: FRUST_LOG=debug shows it)"
  fi
}

median() {
  sort -n | awk '{ v[NR] = $1 } END {
    if (NR == 0) { print "n/a"; exit }
    if (NR % 2) print v[(NR + 1) / 2]; else print int((v[NR / 2] + v[NR / 2 + 1]) / 2) }'
}

echo "hotpatch-spike measure: ${ROW:+row=${ROW} }mode=${MODE} target=${TARGET} runs=${RUNS}"
[ "$MODE" = "hotpatch" ] && echo "dx: ${DX} (${DX_VERSION})"
if [ "$WINDOWS" = 1 ]; then
  echo "frust: ${FRUST} ($(win_ps '& $A[0] --version' "$FRUST" < /dev/null)); app: ${APP} (${APP_BIN}), on the Windows rig"
  echo "windows rig: $(win_ps '$o = Get-CimInstance Win32_OperatingSystem
    $g = (Get-CimInstance Win32_VideoController | ForEach-Object { $_.Name }) -join ", "
    "$($o.Caption) build $($o.BuildNumber), $($o.OSArchitecture); GPU $g"' < /dev/null)"
elif [ "$FRUST_MODE" = 1 ]; then
  echo "frust: ${FRUST} ($("$FRUST" --version 2>/dev/null)); app: ${APP} (${APP_BIN})"
fi
if [ "$ANDROID" = 1 ]; then
  echo "device: $(adb_ shell getprop ro.product.model | tr -d '\r'), Android" \
    "$(adb_ shell getprop ro.build.version.release | tr -d '\r')" \
    "(sdk $(adb_ shell getprop ro.build.version.sdk | tr -d '\r')), $(adb_ shell getprop ro.build.fingerprint | tr -d '\r');" \
    "package ${ANDROID_PACKAGE}"
fi
echo "Logs: ${LOG_DIR}"
echo "If this script dies without its EXIT trap, restore the sources in place with:"
while IFS=$'\t' read -r file copy; do
  echo "  cat '${copy}' > '${file}'"
  # On the Windows rig, the mirror file is then written back over ssh.
  [ "$WINDOWS" = 1 ] && echo "    (rig: $(win_rig_path_of "$file"), e.g. measure.sh's win_put)"
done < "$RESTORE_LIST"

if [ "$ANDROID" = 1 ]; then
  measure_clock_offset 7
  start_devlog
fi
if [ "$WINDOWS" = 1 ]; then START_MS="$(win_now_ms)"; else START_MS="$(now_ms)"; fi
start_runner
echo "Waiting up to ${STARTUP_TIMEOUT}s for the first frame (a cold build compiles frust and wgpu)..."
if [ "$ANDROID" = 1 ]; then
  if ! FIRST_FRAME="$(wait_devlog "$(displayed_re)" "$START_MS" "$STARTUP_TIMEOUT")"; then
    echo "error: no 'Displayed ${ANDROID_PACKAGE}/' line in ${DEVLOG}; see ${LOG}" >&2
    exit 1
  fi
  wait_discovery 0 120 || { echo "error: no devtools discovery line; see ${LOG}" >&2; exit 1; }
elif ! FIRST_FRAME="$(wait_for frame 0 0 "$STARTUP_TIMEOUT")"; then
  echo "error: no first 'frust-hotpatch: frame' line; see ${LOG}" >&2
  exit 1
fi
find_app_pid
echo "First frame after $((FIRST_FRAME - START_MS)) ms (app pid ${APP_PID:-?})."
if [ -n "$PRE_RUN_HOOK" ]; then
  bash -c "${PRE_RUN_HOOK} \"\$1\"" _ "${APP_PID:-0}" 2>&1 | sed 's/^/hook: /'
fi
if [ "$PRE_RUN_PAUSE" != "0" ]; then
  echo "Pausing ${PRE_RUN_PAUSE}s before the first edit (PRE_RUN_PAUSE)..."
  sleep "$PRE_RUN_PAUSE"
fi
if [ "$ANDROID" = 1 ] && [ -n "$APP_PID" ]; then
  echo "before run 1: pid ${APP_PID}; rss $(rss_mb "$APP_PID") MB"
  echo "    $(device_maps "$APP_PID" 0)"
  [ "$HOTPATCH_INFO" = 1 ] && echo "    $(device_hotpatch_info "$APP_PID")"
  echo "    $(screencap before)"
fi
if [ "$WINDOWS" = 1 ] && [ -n "$APP_PID" ]; then
  echo "before run 1: pid ${APP_PID}; rss $(rss_mb "$APP_PID") MB"
  win_ui "$APP_PID" "$WINDOWS_PRESSES" | sed 's/^/    /'
  [ "$MODE" = "frust-run" ] && echo "    $(win_patch_counters)"
  [ -n "$SCREENCAP_DIR" ] && echo "    $(screencap before)"
fi

APPLIED_DELTAS=""
FRAME_DELTAS=""
RESULTS=""
OUTCOMES=""
run=1
while [ "$run" -le "$RUNS" ]; do
  if [ "$ANDROID" = 1 ]; then
    # Not in a subshell: it sets CLOCK_OFFSET_MS for this run.
    measure_clock_offset 5 > "${LOG_DIR}/clock.line"
    sed 's/^/    /' "${LOG_DIR}/clock.line"
  fi
  sleep 1
  from="$(log_lines)"
  if ! stamp="$(apply_edit "$EDIT_FILE" "$TARGET" "$run" "$EDIT_ORIG")"; then
    RESULTS="${RESULTS}run ${run}: edit failed"$'\n'
    break
  fi
  if [ "$WINDOWS" = 1 ]; then
    # The save happens on the rig, stamped by its clock right after the write (see the header).
    stamp="$(win_put "$(win_rig_path_of "$EDIT_FILE")" < "$EDIT_FILE")"
    case "$stamp" in
      ''|*[!0-9]*) RESULTS="${RESULTS}run ${run}: save on the rig failed"$'\n'; break ;;
    esac
  fi
  applied="" frame="" status="ok" outcome="" old_pid="$APP_PID" detail="" source_note=""
  if [ "$MODE" = "restart" ]; then
    stop_runner
    start_runner
    frame="$(wait_for frame "$from" "$stamp" "$EDIT_TIMEOUT")" || status="timeout"
    find_app_pid
  elif [ "$MODE" = "frust-restart" ]; then
    # The CLI's relaunch loop kills the app and `cargo run`s it again by itself.
    APP_PID=""
    frame="$(wait_for frame "$from" "$stamp" "$RESTART_TIMEOUT")" || status="timeout"
    find_app_pid
    detail="; pid ${old_pid:-?} -> ${APP_PID:-?}; rss $(rss_mb "${APP_PID:-0}") MB"
    # The relaunched window's texts (State reset, new sentinel), after the measured frame.
    if [ "$WINDOWS" = 1 ] && [ -n "$APP_PID" ]; then sleep 2; win_ui "$APP_PID" 0 | sed 's/^/    /'; fi
  elif [ "$MODE" = "frust-run" ]; then
    outcome="$(wait_outcome "$from" 180)" || status="timeout"
    o_ms="${outcome%% *}" o_text="${outcome#* }"
    case "$o_text" in
      "patched in "*)
        status="patched"
        if [ "$ANDROID" = 1 ]; then
          # The app's probe lines first (H2-05); the 5 s frame-wait backstop line is the fallback.
          if applied="$(wait_probe applied "$stamp" 10 "$APP_PID")"; then
            source_note="; source: probe"
            frame="$(wait_probe frame "$stamp" 10 "$APP_PID")" || status="patched, no frame probe line"
          else
            source_note="; source: backstop"
            applied="$(wait_applied_backstop "$stamp" "$APP_PID" 10)" \
              || { applied=""; status="patched, no applied probe or backstop line"; }
          fi
          backstop="$(devlog_first "$BACKSTOP_RE" "$stamp" "$APP_PID")"
          if [ -n "$backstop" ]; then
            source_note="${source_note}; backstop: PRESENT at save+$((backstop - stamp)) ms"
          else
            source_note="${source_note}; backstop: none"
          fi
        elif [ "$WINDOWS" = 1 ]; then
          # The app's probe lines first; the 5 s frame-wait backstop line (its read stamp - 5000
          # ms, no frame) is the fallback, as on Android.
          if applied="$(wait_for applied "$from" "$stamp" 10)"; then
            source_note="; source: probe"
            frame="$(wait_for frame "$from" "$stamp" 10)" || status="patched, no frame probe line"
          else
            source_note="; source: backstop"
            if backstop="$(log_line_ms "$from" "$BACKSTOP_RE" "$stamp" 10)"; then
              applied=$((backstop - 5000))
            else
              applied="" status="patched, no applied probe or backstop line"
            fi
          fi
          if backstop="$(log_line_ms "$from" "$BACKSTOP_RE" "$stamp" 0)"; then
            source_note="${source_note}; backstop: PRESENT at save+$((backstop - stamp)) ms"
          else
            source_note="${source_note}; backstop: none"
          fi
        else
          applied="$(wait_for applied "$from" "$stamp" "$EDIT_TIMEOUT")" || status="patched, no applied line"
          frame="$(wait_for frame "$from" "$stamp" "$EDIT_TIMEOUT")" || status="patched, no frame line"
        fi
        ;;
      "restart required: "*)
        status="restart"
        # The CLI kills the app and starts a fresh fat session: wait for the new process's frame.
        APP_PID=""
        if [ "$ANDROID" = 1 ]; then
          # The whole device pipeline again; the relaunched activity's first frame is `Displayed`.
          frame="$(wait_devlog "$(displayed_re)" "$stamp" "$RESTART_TIMEOUT")" \
            || status="restart, no relaunch frame"
          wait_discovery "$from" 120 || status="${status}, no relaunch endpoint"
        else
          frame="$(wait_for frame "$from" "$stamp" "$RESTART_TIMEOUT")" || status="restart, no relaunch frame"
        fi
        find_app_pid
        ;;
      "") ;;
      *) status="other" ;;
    esac
    [ -n "$outcome" ] && echo "    cli: ${o_text} [line at save+$((o_ms - stamp)) ms]"
    if [ "${status%%,*}" = "patched" ]; then
      if [ "$WINDOWS" = 1 ]; then echo "    $(win_host_artifacts "$stamp")"; else echo "    $(host_artifacts "$stamp")"; fi
      if [ "$ANDROID" = 1 ]; then
        echo "    transport: patch_chunk uploads of patch-N.upload.so (a device session never hands off a file)"
      else
        echo "    $(handoff_line "$from")"
      fi
    fi
    [ -n "$outcome" ] && OUTCOMES="${OUTCOMES}${status}"$'\t'"$((o_ms - stamp))"$'\n'
    trouble_lines "$from"
    if [ -n "$APP_PID" ] && ! app_alive "$APP_PID"; then status="crashed"; fi
    if [ "$status" = "patched" ] && [ "$old_pid" != "$APP_PID" ]; then
      status="patched, but the app pid changed"
    fi
    detail="; pid ${old_pid:-?} -> ${APP_PID:-?}; rss $(rss_mb "${APP_PID:-0}") MB"
    if [ "$ANDROID" = 1 ] && [ -n "$APP_PID" ]; then
      echo "    $(device_maps "$APP_PID" "$run")"
      [ "$HOTPATCH_INFO" = 1 ] && echo "    $(device_hotpatch_info "$APP_PID")"
      if [ "${status%%,*}" = "patched" ]; then
        # The stub's thunks hold this process's slid base addresses (relocation evidence).
        stub="$(ls -t "${APP_TARGET_DIR%/}/frust-hotpatch/${SESSION_DIR_NAME}"/stub-*.o 2>/dev/null | head -1)"
        [ -n "$stub" ] && cp "$stub" "${LOG_DIR}/run-${run}-$(basename "$stub")"
      fi
      sleep 2
      echo "    $(screencap "run-${run}")"
    fi
    if [ "$WINDOWS" = 1 ] && [ -n "$APP_PID" ]; then
      echo "    $(win_patch_counters)"
      sleep 2
      win_ui "$APP_PID" 0 | sed 's/^/    /'
      [ -n "$SCREENCAP_DIR" ] && echo "    $(screencap "run-${run}")"
    fi
    if [ -n "$AFTER_RUN_HOOK" ] && [ -n "$APP_PID" ]; then
      sleep 2
      bash -c "${AFTER_RUN_HOOK} \"\$1\" \"\$2\"" _ "$run" "$APP_PID" 2>&1 | sed 's/^/    hook: /'
    fi
  else
    applied="$(wait_for applied "$from" "$stamp" "$EDIT_TIMEOUT")" || status="timeout"
    refused="$(refused_variant "$from")"
    if [ -n "$refused" ]; then
      status="refused"
      [ "$refused" = "AnchorMismatch" ] && status="refused (anchor mismatch)"
      tail -n +"$((from + 1))" "$LOG" | grep -E 'frust-hotpatch: (refused|apply_patch refused)' \
        | tr -d '\033' | sed -e 's/\[[0-9;]*m//g' -e 's/^ */    runner: /' | cut -c1-240 | head -3
    fi
    if [ "$status" = "ok" ]; then
      frame="$(wait_for frame "$from" "$stamp" "$EDIT_TIMEOUT")" || status="timeout"
    fi
    extra=""
    if [ "$TARGET" = "state-field" ]; then
      # Did the PATCHED build run? Only it logs this line (the edit adds it); allow 2 s after frame.
      extra="$(wait_build_line "$from" "$run" 2)"
    fi
    if [ -n "$APP_PID" ] && ! kill -0 "$APP_PID" 2>/dev/null; then status="crashed"; fi
    tail -n +"$((from + 1))" "$LOG" \
      | grep -E 'Patch rebuild:|replaying crates:|Hot-patching:|Build failed|Full rebuild' \
      | tr -d '\033' | sed -e 's/\[[0-9;]*m//g' -e 's/^ */    dx: /' | cut -c1-200
    trouble_lines "$from"
    if [ -n "$AFTER_RUN_HOOK" ]; then
      sleep 2
      bash -c "${AFTER_RUN_HOOK} \"\$1\" \"\$2\"" _ "$run" "${APP_PID:-0}" 2>&1 | sed 's/^/    hook: /'
    fi
  fi
  a_delta="n/a" f_delta="n/a"
  [ -n "$applied" ] && { a_delta=$((applied - stamp)); APPLIED_DELTAS="${APPLIED_DELTAS}${a_delta}"$'\n'; }
  [ -n "$frame" ] && { f_delta=$((frame - stamp)); FRAME_DELTAS="${FRAME_DELTAS}${f_delta}"$'\n'; }
  line="run ${run}: save->applied ${a_delta} ms, save->frame ${f_delta} ms (${status})${detail}${source_note}"
  if [ "$MODE" = "hotpatch" ]; then line="${line}; $(pid_state)"; fi
  if [ "$TARGET" = "state-field" ] && [ "$MODE" = "hotpatch" ]; then
    if [ -n "$extra" ]; then
      line="${line}; patched build RAN, read extra=${extra}"
    else
      line="${line}; patched build NOT seen (old build kept?)"
    fi
  fi
  if [ "$TARGET" = "return-type" ] && [ "$MODE" = "hotpatch" ]; then
    if [ "$status" = "crashed" ]; then
      line="${line}; no patched frame to judge (app gone)"
    elif [ "${status#refused}" != "$status" ]; then
      line="${line}; patch refused, nothing patched to judge"
    elif [ -n "$AFTER_RUN_CAPTURE_DIR" ] && [ -s "${AFTER_RUN_CAPTURE_DIR}/run-${run}.png" ]; then
      line="${line}; capture run-${run}.png exists: judge sentinel v${run} by eye (not yet judged)"
    else
      line="${line}; outcome unjudged (no capture file; set AFTER_RUN_HOOK and AFTER_RUN_CAPTURE_DIR)"
    fi
  fi
  echo "$line"
  RESULTS="${RESULTS}${line}"$'\n'
  [ "$status" = "crashed" ] && { echo "app exited; stopping runs"; break; }
  run=$((run + 1))
done

if [ "$POST_RUN_PAUSE" != "0" ]; then
  echo "Pausing ${POST_RUN_PAUSE}s after the last run (POST_RUN_PAUSE)..."
  sleep "$POST_RUN_PAUSE"
fi

echo
echo "=== Summary: ${ROW:+row ${ROW}, }mode=${MODE} target=${TARGET} ==="
printf '%s' "$RESULTS"
REFUSED_COUNT="$(printf '%s' "$RESULTS" | grep -c '(refused' || true)"
[ "${REFUSED_COUNT:-0}" -gt 0 ] && echo "refused (anchor mismatch or other): ${REFUSED_COUNT} run(s); these are neither applied nor no-op"
echo "median save->applied: $(printf '%s' "$APPLIED_DELTAS" | grep . | median) ms"
echo "median save->frame:   $(printf '%s' "$FRAME_DELTAS" | grep . | median) ms"
if [ "$MODE" = "frust-run" ]; then
  # In this mode save->frame mixes kinds: a patched run's frame is the patched process's, a
  # restart run's is the relaunched process's first frame. Split them.
  echo "outcomes: $(printf '%s' "$RESULTS" | grep -c '(patched)' || true) patched," \
    "$(printf '%s' "$RESULTS" | grep -c '(restart)' || true) restart required (of ${RUNS})"
  echo "median save->frame, patched runs:  $(printf '%s' "$RESULTS" | grep '(patched)' \
    | sed -n 's/.*save->frame \([0-9]*\) ms.*/\1/p' | median) ms"
  # Run 1 is the session's first patch, reported on its own: the steady state leaves it out.
  echo "median save->frame, patched runs 2..${RUNS}: $(printf '%s' "$RESULTS" | grep -v '^run 1:' \
    | grep '(patched)' | sed -n 's/.*save->frame \([0-9]*\) ms.*/\1/p' | median) ms"
  echo "median save->frame, restart runs (relaunched app's first frame):" \
    "$(printf '%s' "$RESULTS" | grep '(restart)' | sed -n 's/.*save->frame \([0-9]*\) ms.*/\1/p' | median) ms"
  echo "median save->CLI outcome line, restart runs:" \
    "$(printf '%s' "$OUTCOMES" | awk -F'\t' '$1 == "restart" { print $2 }' | median) ms"
  if [ "$ANDROID" = 1 ] || [ "$WINDOWS" = 1 ]; then
    # A patched run's applied/frame come from the app's probe lines, or from the backstop line
    # (- 5 s, applied only) as the fallback; the CLI's own line is on the host clock (Android) or
    # the rig's (Windows: the tailer's read stamp).
    echo "$([ "$WINDOWS" = 1 ] && echo windows || echo android): patched runs' applied/frame source:" \
      "$(printf '%s' "$RESULTS" | grep -c 'source: probe' || true) probe," \
      "$(printf '%s' "$RESULTS" | grep -c 'source: backstop' || true) backstop fallback;" \
      "backstop line after the save in $(printf '%s' "$RESULTS" | grep -c 'backstop: PRESENT' || true) run(s)"
    echo "median save->applied, patched runs:  $(printf '%s' "$RESULTS" | grep '(patched)' \
      | sed -n 's/.*save->applied \([0-9]*\) ms.*/\1/p' | median) ms"
    echo "median save->applied, patched runs 2..${RUNS}: $(printf '%s' "$RESULTS" | grep -v '^run 1:' \
      | grep '(patched)' | sed -n 's/.*save->applied \([0-9]*\) ms.*/\1/p' | median) ms"
    echo "median save->CLI outcome line, patched runs:" \
      "$(printf '%s' "$OUTCOMES" | awk -F'\t' '$1 == "patched" { print $2 }' | median) ms"
    if [ "$ANDROID" = 1 ]; then
      echo "restart runs: save->frame is the relaunched activity's 'Displayed' line"
    fi
  fi
fi
