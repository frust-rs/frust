#!/usr/bin/env bash
# scripts/testing/tui-windows-gate.sh — repeatable Windows gate for the
# `frust` TUI, driven from a Linux host.
#
# Ships one commit (`git archive`) to a Windows 11 box over ssh, runs the
# tooling crates' test suites there, then drives the real `frust.exe` TUI over
# `ssh -tt` (Windows OpenSSH allocates a ConPTY) inside a private local tmux
# server, asserting on captured screens. Mouse input and glyph rendering can't
# be judged this way — they stay manual (listed at the end of every run).
#
# Usage:
#   scripts/testing/tui-windows-gate.sh [--sha <rev>] [--host <ssh-host>]
#                                       [--skip-tests] [--skip-e2e]
#
#   --sha <rev>     commit to ship and test (default HEAD); any rev git resolves
#   --host <host>   ssh host of the Windows box (default dell_mini_pc)
#   --skip-tests    skip T1 (per-crate cargo test)
#   --skip-e2e      skip E1-E8 (the interactive TUI checks)
#
# Environment:
#   WIN_GATE_LOCK           flock file serializing use of the Windows box
#                           (default ${TMPDIR:-/tmp}/wintui-gate.lock); point it
#                           at the same file any other box user locks on
#   WIN_GATE_BUILD_TIMEOUT  seconds to wait for a scaffold's first desktop build
#                           to reach a running app (default 1800)
#   WIN_GATE_WORK           local directory for logs and captured screens
#                           (default: a fresh mktemp dir, kept and printed)
#
# What it creates on the box — nothing else is created, and nothing else is
# ever deleted:
#   C:\dev\frust-<sha8>[.tar.gz]   the shipped tree (reused when present)
#   C:\dev\wintui-gate-*           scratch projects, profiles, scripts, logs;
#                                  the project/profile dirs are recreated at
#                                  the start of each E2E run and left in place
#                                  afterwards for inspection
#   C:\dev\target-tui              the shared cargo target dir the per-crate
#                                  test recipe prescribes (frust.exe lands here)
# `C:\dev\wintui-gate-apptarget` is the CARGO_TARGET_DIR every scaffolded app
# builds into, kept across runs so a rerun doesn't pay the first ~5 minute
# desktop build again.
#
# Every interactive TUI runs with HOME cleared and USERPROFILE pointed at a
# scratch profile under C:\dev\wintui-gate-profile*, so the persisted
# `tui.toml` (recent projects, DAP preferences) starts empty each run and the
# real user profile is never written (CARGO_HOME/RUSTUP_HOME are pinned to the
# real profile first so cargo/rustup keep working).
#
# Checks (one PASS/FAIL/BLOCKED line each, then a summary):
#   T1  per-crate cargo test: 0 failures; binaries blocked by an Application
#       Control policy (os error 4551, Smart App Control) reported as BLOCKED
#   E1  startup from the repo root and from examples\ lists projects
#   E2  persistence with HOME cleared: a project opened once shows under
#       PREVIOUS PROJECTS when started from an empty directory
#   E3  `frust create` via the CLI and via the TUI `n` wizard; `cargo metadata`
#       succeeds in both and no Cargo.toml `path =` value is verbatim (\\?\)
#       or backslashed
#   E4  no duplicate project rows (also with the cwd typed in another case)
#   E5  desktop run reaches running; `x` stops it with no orphaned app/cargo/
#       rustc process; `q` with a live session asks, `y` quits, no orphans
#   E6  `i` opens Doctor on both screens, `d` without a session does nothing,
#       `?` help lists `i  Doctor`, the status bar shows `^P palette` (not ⌘)
#   E7  DAP started + IDE override Zed: a launch writes .zed\debug.json with
#       "adapter": "Delve"; a second launch leaves it byte-identical; startup
#       alone writes nothing
#   E8  MCP (4848) and DAP (4849) listen on 127.0.0.1 while started, not after
#
# Exits 0 when nothing FAILed (BLOCKED lines don't fail the run), 1 when any
# check FAILed, 2 on a usage or setup error.
set -euo pipefail

usage() {
    sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

SHA_ARG=HEAD
HOST=dell_mini_pc
SKIP_TESTS=0
SKIP_E2E=0
while [ $# -gt 0 ]; do
    case "$1" in
        --sha) [ $# -ge 2 ] || { echo "--sha needs a value" >&2; exit 2; }; SHA_ARG=$2; shift 2 ;;
        --host) [ $# -ge 2 ] || { echo "--host needs a value" >&2; exit 2; }; HOST=$2; shift 2 ;;
        --skip-tests) SKIP_TESTS=1; shift ;;
        --skip-e2e) SKIP_E2E=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown argument: $1 (see --help)" >&2; exit 2 ;;
    esac
done

for tool in git ssh scp tmux flock; do
    command -v "$tool" >/dev/null 2>&1 || { echo "missing required tool: $tool" >&2; exit 2; }
done

REPO=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
FULL_SHA=$(git -C "$REPO" rev-parse --verify "${SHA_ARG}^{commit}") || {
    echo "cannot resolve --sha $SHA_ARG" >&2; exit 2; }
SHORT=${FULL_SHA:0:8}

LOCK=${WIN_GATE_LOCK:-${TMPDIR:-/tmp}/wintui-gate.lock}
BUILD_TIMEOUT=${WIN_GATE_BUILD_TIMEOUT:-1800}
if [ -n "${WIN_GATE_WORK:-}" ]; then
    WORK=$WIN_GATE_WORK
    mkdir -p "$WORK"
else
    WORK=$(mktemp -d "${TMPDIR:-/tmp}/wintui-gate.XXXXXX")
fi
TMUX_SOCK="wintui-gate-$$"
NOISE='post-quantum|upgraded|openssh\.com|decrypt later|store now'

# Remote (Windows) paths. Single-quoted: backslashes are literal.
SRC="C:\\dev\\frust-$SHORT"
FRUST_EXE='C:\dev\target-tui\debug\frust.exe'
PROJ_A='C:\dev\wintui-gate-a'
PROJ_WIZ='C:\dev\wintui-gate-wiz'
EMPTY='C:\dev\wintui-gate-empty'
PROFILE='C:\dev\wintui-gate-profile'
PROFILE0='C:\dev\wintui-gate-profile0'
APP_EXE='wintui_gate_a.exe'

# ── Result bookkeeping ───────────────────────────────────────────────────────

declare -A RESULT=()
declare -A DETAIL=()
CHECK_ORDER=(T1 E1 E2 E3 E4 E5 E6 E7 E8)

log() { printf '[wintui-gate] %s\n' "$*" >&2; }

# record <id> <PASS|FAIL|BLOCKED|SKIP> <detail>
record() {
    RESULT[$1]=$2
    DETAIL[$1]=$3
    printf '%-7s %s  %s\n' "$2" "$1" "$3"
}

# Print a captured screen (indented) into the output as FAIL evidence.
show_screen() {
    local file=$1 title=$2
    printf '    --- screen: %s ---\n' "$title"
    if [ -s "$file" ]; then
        sed 's/^/    | /' "$file"
    else
        printf '    | (no capture)\n'
    fi
    printf '    --- end screen ---\n'
}

# ── Remote helpers ───────────────────────────────────────────────────────────

# rsh <cmd.exe command line>: run on the box, CRs stripped, ssh noise dropped.
rsh() {
    local out rc=0
    out=$(ssh -o LogLevel=ERROR -o BatchMode=yes "$HOST" "$1" 2>&1) || rc=$?
    printf '%s\n' "$out" | tr -d '\r' | grep -Ev "$NOISE" || true
    return "$rc"
}

# rput <local> <remote forward-slash path>
rput() {
    local out rc=0
    out=$(scp -q -o LogLevel=ERROR -o BatchMode=yes "$1" "$HOST:$2" 2>&1) || rc=$?
    printf '%s\n' "$out" | grep -Ev "$NOISE" | grep -v '^$' >&2 || true
    return "$rc"
}

# rget <remote forward-slash path> <local>
rget() {
    local out rc=0
    out=$(scp -q -o LogLevel=ERROR -o BatchMode=yes "$HOST:$1" "$2" 2>&1) || rc=$?
    printf '%s\n' "$out" | grep -Ev "$NOISE" | grep -v '^$' >&2 || true
    return "$rc"
}

# rcmd <name>: stdin becomes C:\dev\wintui-gate-<name>.cmd (CRLF), then run it.
# `set "X=v"` inside a .cmd avoids the trailing-space trap of `set X=v && ...`.
rcmd() {
    local name=$1 file="$WORK/wintui-gate-$1.cmd"
    sed 's/$/\r/' > "$file"
    rput "$file" "C:/dev/wintui-gate-$name.cmd" || return 1
    rsh "C:\\dev\\wintui-gate-$name.cmd"
}

# rps <name>: stdin becomes C:\dev\wintui-gate-<name>.ps1, then run it.
rps() {
    local name=$1 file="$WORK/wintui-gate-$1.ps1"
    sed 's/$/\r/' > "$file"
    rput "$file" "C:/dev/wintui-gate-$name.ps1" || return 1
    rsh "powershell -NoProfile -ExecutionPolicy Bypass -File C:\\dev\\wintui-gate-$name.ps1"
}

# rexists <remote path> -> 0 when it exists.
rexists() {
    rsh "if exist \"$1\" (echo WG-YES) else (echo WG-NO)" | grep -q WG-YES
}

# Image names of every process on the box (lowercase, one per line).
rprocs() {
    rsh 'tasklist /FO CSV /NH' | cut -d, -f1 | tr -d '"' | tr '[:upper:]' '[:lower:]'
}

# Orphans: app exe, cargo.exe or rustc.exe still alive (space-separated list).
orphans() {
    rprocs | grep -Ex "cargo\.exe|rustc\.exe|${APP_EXE//./\\.}" | sort | uniq -c \
        | awk '{printf "%s%s(x%s)", sep, $2, $1; sep=" "}' || true
}

# Poll until orphans() is empty; echo what remained on timeout.
wait_no_orphans() {
    local timeout=$1 left=""
    local end=$((SECONDS + timeout))
    while [ "$SECONDS" -lt "$end" ]; do
        left=$(orphans)
        [ -z "$left" ] && return 0
        sleep 3
    done
    printf '%s' "$left"
    return 1
}

# LISTENING sockets on 127.0.0.1:<port>.
rlistening() {
    rsh 'netstat -ano -p TCP' | grep -E "127\.0\.0\.1:$1[[:space:]].*LISTENING" || true
}

wait_listening() {
    local port=$1 want=$2 timeout=$3
    local end=$((SECONDS + timeout)) got
    while [ "$SECONDS" -lt "$end" ]; do
        got=$(rlistening "$port")
        if [ "$want" = yes ] && [ -n "$got" ]; then return 0; fi
        if [ "$want" = no ] && [ -z "$got" ]; then return 0; fi
        sleep 2
    done
    return 1
}

# ── tmux / TUI helpers ───────────────────────────────────────────────────────

# A private tmux server; fd 9 (the box lock) is never inherited by it.
tm() { tmux -L "$TMUX_SOCK" "$@" 9>&-; }

sess() { printf 'wintui-gate-%s' "$1"; }

# tui_start <tag> <remote cwd> <remote profile>: start frust.exe in a pane.
tui_start() {
    local s remote
    s=$(sess "$1")
    remote="C:\\dev\\wintui-gate-tui.cmd $2 $3"
    tm kill-session -t "$s" 2>/dev/null || true
    tm new-session -d -s "$s" -x 140 -y 40 \
        "ssh -tt -o LogLevel=ERROR $HOST '$remote'; echo WINTUI-GATE-SSH-EXIT=\$?; sleep 86400"
}

# Current screen of a pane into $WORK/screen-<tag>.txt (and stdout).
scr() {
    local f="$WORK/screen-$1.txt"
    tm capture-pane -p -t "$(sess "$1")" > "$f" 2>/dev/null || : > "$f"
    cat "$f"
}

# wait_screen <tag> <ERE> <timeout-secs>: poll until the screen matches.
wait_screen() {
    local tag=$1 re=$2 timeout=$3
    local end=$((SECONDS + timeout))
    while :; do
        if scr "$tag" | grep -Eq -- "$re"; then return 0; fi
        [ "$SECONDS" -ge "$end" ] && return 1
        sleep 0.5
    done
}

# keys <tag> <tmux key names...> (Enter, Escape, Right, BSpace, C-p, ...).
keys() {
    tm send-keys -t "$(sess "$1")" "${@:2}"
    sleep 0.4
}

# lit <tag> <text>: type literal text.
lit() {
    tm send-keys -t "$(sess "$1")" -l -- "$2"
    sleep 0.4
}

# tui_quit <tag>: `q` with no live session; waits for the process to end.
tui_quit() {
    keys "$1" q
    wait_screen "$1" 'WINTUI-GATE-SSH-EXIT=' 30
}

tui_end() {
    tm kill-session -t "$(sess "$1")" 2>/dev/null || true
}

# The PROJECTS / PREVIOUS PROJECTS rows of the sidebar (one name per line),
# read from a captured screen file: every indented row between the PROJECTS
# heading and the DEVICES heading, sidebar column only. The sidebar border is
# matched as an alternation, never inside a bracket expression: a byte-wise
# awk would split `│` into bytes there, and its lead byte also starts the `▸`
# chevron of the active project's row.
project_rows() {
    awk '
        { line = $0; sub(/(│|┃).*$/, "", line) }
        line ~ /^[[:space:]]*PROJECTS[[:space:]]*$/ { on = 1; next }
        line ~ /^[[:space:]]*DEVICES/ { on = 0 }
        on && line ~ /PREVIOUS PROJECTS/ { next }
        on && line ~ /\(none detected\)/ { next }
        on {
            gsub(/^[[:space:]]+|[[:space:]]+$/, "", line)
            sub(/^[^[:alnum:]_.-]+[[:space:]]*/, "", line)
            if (line != "") print line
        }
    ' "$1"
}

# ── Cleanup ──────────────────────────────────────────────────────────────────

REMOTE_TOUCHED=0
cleanup() {
    local rc=$?
    trap - EXIT INT TERM
    tm kill-server 2>/dev/null || true
    rm -f "${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)/$TMUX_SOCK"
    if [ "$REMOTE_TOUCHED" = 1 ]; then
        # Our own processes only: the frust.exe this script built (with its
        # whole child tree), the scratch app, and cargo/rustc working on a
        # wintui-gate path.
        rps cleanup <<'PS' >&2 || true
$frust = 'C:\dev\target-tui\debug\frust.exe'
Get-CimInstance Win32_Process -Filter "Name='frust.exe'" |
  Where-Object { $_.ExecutablePath -eq $frust } |
  ForEach-Object { Write-Output "cleanup: killing frust.exe tree $($_.ProcessId)"; taskkill /F /T /PID $_.ProcessId | Out-Null }
Get-CimInstance Win32_Process |
  Where-Object { ($_.Name -like 'wintui_gate_*.exe') -or (($_.Name -in 'cargo.exe','rustc.exe') -and ($_.CommandLine -match 'wintui[-_]gate')) } |
  ForEach-Object { Write-Output "cleanup: killing $($_.Name) $($_.ProcessId)"; Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
PS
    fi
    exit "$rc"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# ── Lock + ship ──────────────────────────────────────────────────────────────

log "commit $FULL_SHA -> $HOST:$SRC  (work dir $WORK)"
exec 9>"$LOCK"
log "waiting for the Windows box lock $LOCK ..."
flock 9
log "lock acquired"
REMOTE_TOUCHED=1

ship() {
    if rexists "$SRC\\Cargo.toml"; then
        log "reusing $SRC"
        return 0
    fi
    local tgz="$WORK/frust-$SHORT.tar.gz"
    git -C "$REPO" archive --format=tar.gz --prefix="frust-$SHORT/" "$FULL_SHA" > "$tgz"
    rput "$tgz" "C:/dev/frust-$SHORT.tar.gz"
    rsh "cd /d C:\\dev && tar -xzf frust-$SHORT.tar.gz" >&2
    rexists "$SRC\\Cargo.toml"
}
if ! ship; then
    echo "setup failed: could not ship $FULL_SHA to $HOST:$SRC" >&2
    exit 2
fi

SAC=$(rsh 'reg query HKLM\SYSTEM\CurrentControlSet\Control\CI\Policy /v VerifiedAndReputablePolicyState' \
    | awk '/VerifiedAndReputablePolicyState/ {print $NF}')
log "Smart App Control VerifiedAndReputablePolicyState=${SAC:-unknown} (0x0 = off)"

# ── T1: per-crate cargo test ─────────────────────────────────────────────────

check_t1() {
    local logf="$WORK/t1.log" crlf="$WORK/t1.log.crlf"
    log "T1: cargo test on the box (several minutes) ..."
    rcmd t1 >/dev/null <<EOF || true
@echo off
set "CARGO_TARGET_DIR=C:\\dev\\target-tui"
cd /d $SRC
cargo test -p frust-tui -p frust-dap -p frust-mcp -p frust-drive -p frust-devtools-protocol -p frust-paths -p frust-cli --no-fail-fast -j 4 > C:\\dev\\wintui-gate-t1-$SHORT.log 2>&1
echo WINTUI-GATE-EXIT=%ERRORLEVEL% >> C:\\dev\\wintui-gate-t1-$SHORT.log
EOF
    if ! rget "C:/dev/wintui-gate-t1-$SHORT.log" "$crlf"; then
        record T1 FAIL "no test log came back"
        return
    fi
    tr -d '\r' < "$crlf" > "$logf"
    rm -f "$crlf"

    local summary
    summary=$(awk '
        /^[[:space:]]+Running / { targets++; last = $0; sub(/^.*\(/, "", last); sub(/\)[[:space:]]*$/, "", last) }
        /could not execute process/ { proc = $0; sub(/^.*process `/, "", proc); sub(/`.*$/, "", proc) }
        /os error 4551/ { b = (proc != "" ? proc : last); if (!(b in seen)) { seen[b] = 1; blocked = blocked " " b; nb++ } }
        /^test result: / { for (i = 1; i <= NF; i++) { if ($(i+1) ~ /^passed;?$/) p += $i; if ($(i+1) ~ /^failed;?$/) f += $i } }
        /^test .* \.\.\. FAILED$/ { t = $2; failing = failing " " t }
        /^error(\[E[0-9]+\])?: could not compile/ { ce++ }
        /^WINTUI-GATE-EXIT=/ { sub(/^WINTUI-GATE-EXIT=/, ""); gsub(/[[:space:]]/, ""); code = $0 }
        END { printf "%d|%d|%d|%d|%s|%s|%s|%d\n", targets, p, f, nb, code, failing, blocked, ce }
    ' "$logf")
    local targets passed failed nblocked code failing blocked cerr
    IFS='|' read -r targets passed failed nblocked code failing blocked cerr <<< "$summary"
    local what="$targets test targets, $passed passed, $failed failed, $nblocked blocked (cargo exit ${code:-?})"
    if [ "${cerr:-0}" -gt 0 ]; then
        record T1 FAIL "$what; $cerr crate(s) failed to compile — see $logf"
        grep -E '^error' "$logf" | head -20 | sed 's/^/    /'
    elif [ "$failed" -gt 0 ]; then
        record T1 FAIL "$what; failing:${failing}"
    elif [ "$nblocked" -gt 0 ]; then
        record T1 BLOCKED "$what; blocked by Application Control (os error 4551, SAC state ${SAC:-?}):${blocked}"
    elif [ "$code" = 0 ] && [ "$targets" -gt 0 ]; then
        record T1 PASS "$what"
    else
        record T1 FAIL "$what; unexplained non-zero exit — see $logf"
        tail -20 "$logf" | sed 's/^/    /'
    fi
}

if [ "$SKIP_TESTS" = 1 ]; then
    record T1 SKIP "--skip-tests"
else
    check_t1
fi

if [ "$SKIP_E2E" = 1 ]; then
    for id in E1 E2 E3 E4 E5 E6 E7 E8; do record "$id" SKIP "--skip-e2e"; done
fi

# ── E2E setup ────────────────────────────────────────────────────────────────

e2e_setup() {
    log "building frust.exe on the box ..."
    rcmd build >/dev/null <<EOF || true
@echo off
set "CARGO_TARGET_DIR=C:\\dev\\target-tui"
cd /d $SRC
cargo build -p frust-cli -j 4 > C:\\dev\\wintui-gate-build-$SHORT.log 2>&1
echo WINTUI-GATE-EXIT=%ERRORLEVEL% >> C:\\dev\\wintui-gate-build-$SHORT.log
EOF
    rget "C:/dev/wintui-gate-build-$SHORT.log" "$WORK/build.log.crlf" || true
    tr -d '\r' < "$WORK/build.log.crlf" > "$WORK/build.log" 2>/dev/null || true
    if ! grep -q '^WINTUI-GATE-EXIT=0' "$WORK/build.log"; then
        log "frust.exe build failed:"
        tail -30 "$WORK/build.log" >&2 || true
        return 1
    fi

    # Fresh scratch projects + profiles (ours only; the app target dir is kept).
    rcmd setup >&2 <<'EOF'
@echo off
for %%d in (a wiz empty profile profile0) do if exist C:\dev\wintui-gate-%%d rmdir /s /q C:\dev\wintui-gate-%%d
mkdir C:\dev\wintui-gate-empty
mkdir C:\dev\wintui-gate-profile
mkdir C:\dev\wintui-gate-profile0
if not exist C:\dev\wintui-gate-apptarget mkdir C:\dev\wintui-gate-apptarget
EOF

    # The interactive launcher: HOME cleared, scratch USERPROFILE.
    sed 's/$/\r/' > "$WORK/wintui-gate-tui.cmd" <<EOF
@echo off
set "HOME="
set "XDG_CONFIG_HOME="
if not defined CARGO_HOME set "CARGO_HOME=%USERPROFILE%\\.cargo"
if not defined RUSTUP_HOME set "RUSTUP_HOME=%USERPROFILE%\\.rustup"
set "USERPROFILE=%~2"
set "CARGO_TARGET_DIR=C:\\dev\\wintui-gate-apptarget"
cd /d %~1
$FRUST_EXE
echo WINTUI-GATE-TUI-EXIT=%ERRORLEVEL%
EOF
    rput "$WORK/wintui-gate-tui.cmd" "C:/dev/wintui-gate-tui.cmd"
}

# ── E3 (CLI half) — creates wintui-gate-a, which E2/E5/E7 use ────────────────

E3_CLI_OK=0
E3_NOTES=""
e3_cli_create() {
    local out
    out=$(rcmd create-cli <<EOF 2>&1 || true
@echo off
$FRUST_EXE create $PROJ_A --project-name wintui_gate_a
echo WINTUI-GATE-EXIT=%ERRORLEVEL%
EOF
)
    printf '%s\n' "$out" > "$WORK/e3-cli.txt"
    if printf '%s\n' "$out" | grep -q '^WINTUI-GATE-EXIT=0' && rexists "$PROJ_A\\Cargo.toml"; then
        E3_CLI_OK=1
    else
        E3_NOTES="CLI create failed: $(tail -5 "$WORK/e3-cli.txt" | tr '\n' ' ')"
    fi
}

# e3_manifest_ok <remote project dir> <label>: cargo metadata + path= hygiene.
e3_manifest_ok() {
    local dir=$1 label=$2 out toml bad
    out=$(rcmd "meta-$label" <<EOF 2>&1 || true
@echo off
cd /d $dir
cargo metadata --format-version 1 --no-deps > nul 2> C:\\dev\\wintui-gate-meta-$label.err
echo WINTUI-GATE-EXIT=%ERRORLEVEL%
EOF
)
    if ! printf '%s\n' "$out" | grep -q '^WINTUI-GATE-EXIT=0'; then
        E3_NOTES="$E3_NOTES [$label: cargo metadata failed: $(rsh "type C:\\dev\\wintui-gate-meta-$label.err" | head -5 | tr '\n' ' ')]"
        return 1
    fi
    toml=$(rsh "type $dir\\Cargo.toml")
    printf '%s\n' "$toml" > "$WORK/e3-$label-Cargo.toml"
    bad=$(printf '%s\n' "$toml" | grep -E 'path[[:space:]]*=' | grep -F '\' || true)
    if printf '%s\n' "$toml" | grep -qF '\\?\'; then
        E3_NOTES="$E3_NOTES [$label: Cargo.toml contains a verbatim \\\\?\\ path]"
        return 1
    fi
    if [ -n "$bad" ]; then
        E3_NOTES="$E3_NOTES [$label: backslash in path =: $(printf '%s' "$bad" | tr '\n' ' ')]"
        return 1
    fi
    E3_NOTES="$E3_NOTES [$label: metadata ok, $(printf '%s\n' "$toml" | grep -cE 'path[[:space:]]*=') path= value(s) portable]"
    return 0
}

# ── E1: startup lists projects ───────────────────────────────────────────────

EXAMPLE_NAMES='glyph-catalog|huddle|material3-demo|playground|shadertoy'
check_e1() {
    local ok=1 notes="" where tag n
    for where in root examples; do
        tag="e1-$where"
        if [ "$where" = root ]; then tui_start "$tag" "$SRC" "$PROFILE"; else tui_start "$tag" "$SRC\\examples" "$PROFILE"; fi
        if wait_screen "$tag" 'PROJECTS' 60; then
            cp "$WORK/screen-$tag.txt" "$WORK/rows-screen-$tag.txt"
            project_rows "$WORK/screen-$tag.txt" > "$WORK/rows-$tag.txt"
            n=$(grep -cEx "$EXAMPLE_NAMES" "$WORK/rows-$tag.txt" || true)
            if [ "$n" -ge 3 ]; then
                notes="$notes $where: $n example projects listed;"
            else
                ok=0; notes="$notes $where: only $n example projects listed;"
                show_screen "$WORK/screen-$tag.txt" "E1 $where"
            fi
        else
            ok=0; notes="$notes $where: no PROJECTS heading within 60s;"
            show_screen "$WORK/screen-$tag.txt" "E1 $where"
        fi
        tui_quit "$tag" || { ok=0; notes="$notes $where: q did not quit;"; }
        tui_end "$tag"
    done
    if [ "$ok" = 1 ]; then record E1 PASS "${notes% ;}"; else record E1 FAIL "${notes% ;}"; fi
}

# ── E6 (both halves): keys ───────────────────────────────────────────────────

DOCTOR_RE='r re-run · t toolchain|Toolchain setup'
HELP_RE='(^|[^[:alnum:]])i +Doctor'
E6_WELCOME=""
E6_NOTES=""

# e6_keys <tag> <screen-name>: i -> Doctor, Esc, d -> nothing, ? -> help.
e6_keys() {
    local tag=$1 name=$2 ok=0
    keys "$tag" i
    if wait_screen "$tag" "$DOCTOR_RE" 15; then
        E6_NOTES="$E6_NOTES $name: i opens Doctor;"
    else
        ok=1; E6_NOTES="$E6_NOTES $name: i did NOT open Doctor;"
        show_screen "$WORK/screen-$tag.txt" "E6 $name after i"
    fi
    keys "$tag" Escape
    sleep 1
    if scr "$tag" | grep -Eq "$DOCTOR_RE"; then keys "$tag" Escape; sleep 1; fi
    cp "$WORK/screen-$tag.txt" "$WORK/screen-$tag-before-d.txt"
    keys "$tag" d
    sleep 2
    if scr "$tag" | grep -Eq "$DOCTOR_RE|DevTools|Performance|Inspector"; then
        ok=1; E6_NOTES="$E6_NOTES $name: d without a session opened something;"
        show_screen "$WORK/screen-$tag.txt" "E6 $name after d"
        keys "$tag" Escape
    else
        E6_NOTES="$E6_NOTES $name: d is a no-op;"
    fi
    keys "$tag" '?'
    if wait_screen "$tag" "$HELP_RE" 15; then
        E6_NOTES="$E6_NOTES $name: ? help lists 'i  Doctor';"
    else
        ok=1; E6_NOTES="$E6_NOTES $name: ? help lacks 'i  Doctor';"
        show_screen "$WORK/screen-$tag.txt" "E6 $name help"
    fi
    keys "$tag" Escape
    sleep 1
    return "$ok"
}

e6_welcome() {
    local tag=e6-welcome
    tui_start "$tag" "$EMPTY" "$PROFILE0"
    if ! wait_screen "$tag" 'No Frust project found' 60; then
        E6_WELCOME=fail
        E6_NOTES="$E6_NOTES welcome: welcome screen never showed;"
        show_screen "$WORK/screen-$tag.txt" "E6 welcome"
    elif e6_keys "$tag" welcome; then
        E6_WELCOME=pass
    else
        E6_WELCOME=fail
    fi
    tui_quit "$tag" || true
    tui_end "$tag"
}

# Workbench half — run inside the main session (tag `main`, idle workbench).
e6_workbench_and_record() {
    local tag=main ok=0 status
    [ "$E6_WELCOME" = pass ] || ok=1
    e6_keys "$tag" workbench || ok=1
    status=$(scr "$tag" | grep -v '^[[:space:]]*$' | tail -1)
    if printf '%s' "$status" | grep -qF '^P palette' && ! printf '%s' "$status" | grep -qF '⌘'; then
        E6_NOTES="$E6_NOTES status bar shows '^P palette', no ⌘"
    else
        ok=1; E6_NOTES="$E6_NOTES status bar wrong: '$(printf '%s' "$status" | sed 's/  */ /g')'"
    fi
    if [ "$ok" = 0 ]; then record E6 PASS "$E6_NOTES"; else record E6 FAIL "$E6_NOTES"; fi
}

# ── E2: persistence with HOME cleared ────────────────────────────────────────

check_e2() {
    local tag=e2-a ok=1 notes=""
    if [ "$E3_CLI_OK" != 1 ]; then record E2 FAIL "no scaffold in $PROJ_A (CLI create failed)"; return; fi
    tui_start "$tag" "$PROJ_A" "$PROFILE"
    if ! wait_screen "$tag" 'wintui-gate-a' 60; then
        ok=0; notes="first start in $PROJ_A never listed it;"
        show_screen "$WORK/screen-$tag.txt" "E2 first start"
    fi
    tui_quit "$tag" || { ok=0; notes="$notes q did not quit;"; }
    tui_end "$tag"
    if rexists "$PROFILE\\.config\\frust\\tui.toml"; then
        notes="$notes tui.toml written under the scratch USERPROFILE;"
    else
        ok=0; notes="$notes no $PROFILE\\.config\\frust\\tui.toml after quit;"
    fi
    tag=e2-empty
    tui_start "$tag" "$EMPTY" "$PROFILE"
    if wait_screen "$tag" 'PREVIOUS PROJECTS' 60 \
        && awk '/PREVIOUS PROJECTS/ {p = 1} p && /wintui-gate-a/ {f = 1} END {exit !f}' "$WORK/screen-$tag.txt"; then
        notes="$notes restart from $EMPTY lists wintui-gate-a under PREVIOUS PROJECTS"
    else
        ok=0; notes="$notes restart from $EMPTY: wintui-gate-a not under PREVIOUS PROJECTS"
        show_screen "$WORK/screen-$tag.txt" "E2 restart from empty"
    fi
    tui_quit "$tag" || true
    tui_end "$tag"
    if [ "$ok" = 1 ]; then record E2 PASS "$notes"; else record E2 FAIL "$notes"; fi
}

# ── E3 (TUI half) + E4 ───────────────────────────────────────────────────────

check_e3_e4() {
    local tag=e3-wiz wiz_ok=0 e4_ok=1 e4_notes="" dups i
    if [ "$E3_CLI_OK" != 1 ]; then
        record E3 FAIL "$E3_NOTES"
        record E4 FAIL "not run: no CLI scaffold to start from"
        return
    fi
    tui_start "$tag" "$PROJ_A" "$PROFILE"
    if wait_screen "$tag" 'PROJECTS' 60; then
        keys "$tag" n
        if wait_screen "$tag" 'Step 1 of 3' 15; then
            lit "$tag" wintui_gate_wiz
            keys "$tag" Enter
            if wait_screen "$tag" 'Step 2 of 3' 10; then
                for ((i = 0; i < 24; i++)); do tm send-keys -t "$(sess "$tag")" BSpace; done
                sleep 0.5
                lit "$tag" "$PROJ_WIZ"
                keys "$tag" Enter
                if wait_screen "$tag" 'Step 3 of 3' 10; then
                    keys "$tag" Enter
                    if wait_screen "$tag" 'Created project wintui-gate-wiz' 120 || rexists "$PROJ_WIZ\\Cargo.toml"; then
                        wiz_ok=1
                    fi
                fi
            fi
        fi
    fi
    if [ "$wiz_ok" != 1 ]; then
        E3_NOTES="$E3_NOTES [TUI wizard did not create $PROJ_WIZ]"
        show_screen "$WORK/screen-$tag.txt" "E3 wizard"
    fi
    # E4 on the post-create screen.
    sleep 2
    scr "$tag" > /dev/null
    cp "$WORK/screen-$tag.txt" "$WORK/rows-screen-e4-a.txt"
    project_rows "$WORK/screen-$tag.txt" > "$WORK/rows-e4-a.txt"
    dups=$(tr '[:upper:]' '[:lower:]' < "$WORK/rows-e4-a.txt" | sort | uniq -d | tr '\n' ' ')
    if [ -n "$dups" ]; then
        e4_ok=0; e4_notes="after create: duplicate rows: $dups;"
        show_screen "$WORK/screen-$tag.txt" "E4 after create"
    else
        e4_notes="after create: $(wc -l < "$WORK/rows-e4-a.txt") unique rows ($(tr '\n' ' ' < "$WORK/rows-e4-a.txt"));"
    fi
    tui_quit "$tag" || true
    tui_end "$tag"

    # E4 again with the cwd typed in another case (path identity must fold it).
    tag=e4-case
    tui_start "$tag" 'c:\DEV\WINTUI-GATE-A' "$PROFILE"
    if wait_screen "$tag" 'PREVIOUS PROJECTS|PROJECTS' 60; then
        sleep 1
        scr "$tag" > /dev/null
        cp "$WORK/screen-$tag.txt" "$WORK/rows-screen-e4-b.txt"
        project_rows "$WORK/screen-$tag.txt" > "$WORK/rows-e4-b.txt"
        dups=$(tr '[:upper:]' '[:lower:]' < "$WORK/rows-e4-b.txt" | sort | uniq -d | tr '\n' ' ')
        if [ -n "$dups" ]; then
            e4_ok=0; e4_notes="$e4_notes case-folded cwd: duplicate rows: $dups"
            show_screen "$WORK/screen-$tag.txt" "E4 case-folded cwd"
        else
            e4_notes="$e4_notes case-folded cwd: $(wc -l < "$WORK/rows-e4-b.txt") unique rows ($(tr '\n' ' ' < "$WORK/rows-e4-b.txt"))"
        fi
    else
        e4_ok=0; e4_notes="$e4_notes case-folded cwd: TUI never showed projects"
        show_screen "$WORK/screen-$tag.txt" "E4 case-folded cwd"
    fi
    tui_quit "$tag" || true
    tui_end "$tag"
    if [ "$e4_ok" = 1 ]; then record E4 PASS "$e4_notes"; else record E4 FAIL "$e4_notes"; fi

    local e3_ok=1
    e3_manifest_ok "$PROJ_A" cli || e3_ok=0
    if [ "$wiz_ok" = 1 ]; then e3_manifest_ok "$PROJ_WIZ" wiz || e3_ok=0; else e3_ok=0; fi
    if [ "$e3_ok" = 1 ]; then record E3 PASS "$E3_NOTES"; else record E3 FAIL "$E3_NOTES"; fi
}

# ── Main session: E6 workbench, E8, E7, E5 ───────────────────────────────────

# launch_and_wait <tag> <timeout>: `r` Enter (desktop) until the app runs.
# Returns 0 when the status bar says "1 running" and the app exe is alive.
launch_and_wait() {
    local tag=$1 timeout=$2 end
    keys "$tag" r
    if ! wait_screen "$tag" ' Run · ' 10; then return 1; fi
    keys "$tag" Enter
    end=$((SECONDS + timeout))
    sleep 5
    while [ "$SECONDS" -lt "$end" ]; do
        if scr "$tag" | grep -q ' ready '; then
            # The session already ended on its own.
            return 1
        fi
        if grep -q '1 running' "$WORK/screen-$tag.txt" && rprocs | grep -qx "$APP_EXE"; then
            return 0
        fi
        sleep 10
    done
    return 1
}

# stop_and_wait <tag>: `x`, then wait for the status bar to read "ready".
stop_and_wait() {
    keys "$1" x
    wait_screen "$1" ' ready ' 60
}

# fetch <remote path> <local name>: copy a remote file into $WORK (scp wants
# forward slashes); echoes the local path.
fetch() {
    local f="$WORK/$2" remote=${1//\\//}
    rm -f "$f"
    rget "$remote" "$f" >/dev/null 2>&1 || return 1
    printf '%s' "$f"
}

main_session() {
    local tag=main e8_ok=1 e8_notes="" e7_ok=1 e7_notes="" e5_ok=1 e5_notes="" i left
    local first second
    if [ "$E3_CLI_OK" != 1 ]; then
        for id in E5 E7 E8; do record "$id" FAIL "not run: no CLI scaffold in $PROJ_A"; done
        record E6 FAIL "$E6_NOTES workbench half not run"
        return
    fi
    tui_start "$tag" "$PROJ_A" "$PROFILE"
    if ! wait_screen "$tag" 'wintui-gate-a' 60; then
        show_screen "$WORK/screen-$tag.txt" "main session start"
        for id in E5 E6 E7 E8; do record "$id" FAIL "workbench never opened in $PROJ_A"; done
        tui_end "$tag"
        return
    fi
    sleep 2

    # E7 precondition: startup alone writes no IDE config.
    if rexists "$PROJ_A\\.zed" || rexists "$PROJ_A\\.vscode"; then
        e7_ok=0; e7_notes="startup wrote an IDE config dir (.zed/.vscode) before any launch;"
    else
        e7_notes="startup wrote nothing;"
    fi

    e6_workbench_and_record

    # E8 / MCP: M starts, M stops.
    keys "$tag" M
    if wait_listening 4848 yes 20; then
        e8_notes="MCP 127.0.0.1:4848 LISTENING after M;"
        keys "$tag" M
        if wait_listening 4848 no 20; then
            e8_notes="$e8_notes gone after M again;"
        else
            e8_ok=0; e8_notes="$e8_notes still LISTENING 20s after stop;"
        fi
    else
        e8_ok=0; e8_notes="MCP never listened on 127.0.0.1:4848;"
        show_screen "$WORK/screen-$tag.txt" "E8 MCP"
    fi

    # DAP dialog: IDE override -> Zed, then start the server.
    keys "$tag" D
    if wait_screen "$tag" 'DAP server' 10; then
        for ((i = 0; i < 6; i++)); do
            scr "$tag" | grep -q '‹ Zed ›' && break
            keys "$tag" Right
            sleep 0.3
        done
        if ! scr "$tag" | grep -q '‹ Zed ›'; then
            e7_ok=0; e7_notes="$e7_notes could not select Zed in the DAP dialog;"
            show_screen "$WORK/screen-$tag.txt" "E7 DAP dialog"
        fi
        keys "$tag" s
        if wait_listening 4849 yes 20; then
            e8_notes="$e8_notes DAP 127.0.0.1:4849 LISTENING after s;"
        else
            e8_ok=0; e8_notes="$e8_notes DAP never listened on 127.0.0.1:4849;"
            show_screen "$WORK/screen-$tag.txt" "E8 DAP start"
        fi
        keys "$tag" Escape
        sleep 1
    else
        e7_ok=0; e8_ok=0; e8_notes="$e8_notes D did not open the DAP dialog;"
        show_screen "$WORK/screen-$tag.txt" "E8 DAP dialog"
    fi

    # E7: first launch (also the long first build) writes .zed\debug.json.
    log "E7/E5: first desktop launch of wintui-gate-a (first build can take ~5+ min) ..."
    if launch_and_wait "$tag" "$BUILD_TIMEOUT"; then
        e5_notes="desktop run reached running ($APP_EXE alive);"
        sleep 2
        if first=$(fetch "$PROJ_A/.zed/debug.json" e7-debug-1.json); then
            if grep -Eq '"adapter"[[:space:]]*:[[:space:]]*"Delve"' "$first"; then
                e7_notes="$e7_notes launch wrote .zed\\debug.json with \"adapter\": \"Delve\";"
            else
                e7_ok=0; e7_notes="$e7_notes .zed\\debug.json lacks \"adapter\": \"Delve\";"
                sed 's/^/    | /' "$first"
            fi
        else
            e7_ok=0; e7_notes="$e7_notes no .zed\\debug.json after the launch;"
        fi
        # E5: x stops it with no orphans.
        if stop_and_wait "$tag"; then
            if left=$(wait_no_orphans 30); then
                e5_notes="$e5_notes x stopped it, no orphaned app/cargo/rustc;"
            else
                e5_ok=0; e5_notes="$e5_notes orphans 30s after x: $left;"
            fi
        else
            e5_ok=0; e5_notes="$e5_notes x did not end the session within 60s;"
            show_screen "$WORK/screen-$tag.txt" "E5 after x"
        fi
        # E7: second launch leaves the file byte-identical.
        if launch_and_wait "$tag" 600; then
            sleep 2
            if second=$(fetch "$PROJ_A/.zed/debug.json" e7-debug-2.json) && [ -n "$first" ] \
                && cmp -s "$first" "$second"; then
                e7_notes="$e7_notes second launch left it byte-identical"
            else
                e7_ok=0; e7_notes="$e7_notes second launch changed (or removed) .zed\\debug.json"
            fi
            stop_and_wait "$tag" || true
            wait_no_orphans 30 >/dev/null || true
        else
            e7_ok=0; e7_notes="$e7_notes second launch never reached running"
            show_screen "$WORK/screen-$tag.txt" "E7 second launch"
        fi
    else
        e5_ok=0; e7_ok=0
        e5_notes="desktop run never reached running (status '1 running' + $APP_EXE alive) within ${BUILD_TIMEOUT}s;"
        e7_notes="$e7_notes not judged: the launch never reached running"
        show_screen "$WORK/screen-$tag.txt" "E5 first launch"
        tm capture-pane -p -S -200 -t "$(sess "$tag")" > "$WORK/screen-main-scrollback.txt" 2>/dev/null || true
        stop_and_wait "$tag" || true
    fi
    if [ "$e7_ok" = 1 ]; then record E7 PASS "$e7_notes"; else record E7 FAIL "$e7_notes"; fi

    # E8 / DAP stop.
    keys "$tag" D
    if wait_screen "$tag" 'DAP server' 10; then
        keys "$tag" s
        if wait_listening 4849 no 20; then
            e8_notes="$e8_notes DAP gone after s again"
        else
            e8_ok=0; e8_notes="$e8_notes DAP still LISTENING 20s after stop"
        fi
        keys "$tag" Escape
        sleep 1
    else
        e8_ok=0; e8_notes="$e8_notes could not reopen the DAP dialog to stop it"
    fi
    if [ "$e8_ok" = 1 ]; then record E8 PASS "$e8_notes"; else record E8 FAIL "$e8_notes"; fi

    # E5: quit with a live session -> confirmation -> y -> no orphans.
    if launch_and_wait "$tag" 600; then
        keys "$tag" q
        if wait_screen "$tag" 'Quit frust\?' 10 && grep -q '1 running session will be stopped' "$WORK/screen-$tag.txt"; then
            e5_notes="$e5_notes q with a live session asks ('1 running session will be stopped');"
            keys "$tag" y
            if wait_screen "$tag" 'WINTUI-GATE-SSH-EXIT=' 60; then
                if left=$(wait_no_orphans 30); then
                    e5_notes="$e5_notes y quit, no orphans"
                else
                    e5_ok=0; e5_notes="$e5_notes orphans 30s after quit: $left"
                fi
            else
                e5_ok=0; e5_notes="$e5_notes y did not quit within 60s"
                show_screen "$WORK/screen-$tag.txt" "E5 after y"
            fi
        else
            e5_ok=0; e5_notes="$e5_notes q with a live session showed no confirmation"
            show_screen "$WORK/screen-$tag.txt" "E5 quit confirm"
        fi
    else
        e5_ok=0; e5_notes="$e5_notes relaunch for the quit check never reached running"
        show_screen "$WORK/screen-$tag.txt" "E5 relaunch"
    fi
    if [ "$e5_ok" = 1 ]; then record E5 PASS "$e5_notes"; else record E5 FAIL "$e5_notes"; fi
    tui_end "$tag"
}

if [ "$SKIP_E2E" = 0 ]; then
    if e2e_setup; then
        e3_cli_create
        check_e1
        e6_welcome
        check_e2
        check_e3_e4
        main_session
    else
        for id in E1 E2 E3 E4 E5 E6 E7 E8; do record "$id" FAIL "frust.exe build failed — see $WORK/build.log"; done
    fi
fi

# ── Summary ──────────────────────────────────────────────────────────────────

pass=0; fail=0; blocked=0; skip=0
echo
echo "== wintui-gate summary: $FULL_SHA on $HOST (SAC state ${SAC:-unknown}) =="
for id in "${CHECK_ORDER[@]}"; do
    r=${RESULT[$id]:-FAIL}
    case "$r" in
        PASS) pass=$((pass + 1)) ;;
        BLOCKED) blocked=$((blocked + 1)) ;;
        SKIP) skip=$((skip + 1)) ;;
        *) fail=$((fail + 1)) ;;
    esac
    printf '  %-7s %s\n' "$r" "$id"
done
echo "  $pass passed, $fail failed, $blocked blocked, $skip skipped"
echo "  manual (not scriptable over ssh): mouse input, glyph/color rendering, the app window itself"
echo "  left on the box for inspection: C:\\dev\\wintui-gate-* ; local logs/screens: $WORK"
[ "$fail" -eq 0 ]
