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
#        examples/hotpatch-spike/measure.sh --prepare-app <dir>
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
ROW=""

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

if [ -n "$PREPARE_APP" ]; then
  prepare_app "$PREPARE_APP"
  exit $?
fi

case "$RUNS" in
  ''|*[!0-9]*|0) echo "error: --runs must be a positive integer, got '$RUNS'" >&2; exit 2 ;;
esac

FRUST_MODE=0
case "$MODE" in frust-run|frust-restart) FRUST_MODE=1 ;; esac
if [ "$FRUST_MODE" = 1 ]; then
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
  set -m
  if [ "$MODE" = "hotpatch" ]; then
    (cd "$RUNNER_DIR" && exec "$DX" serve --hot-patch --platform desktop --interactive false \
      --verbose) >> "$LOG" 2>&1 < /dev/null &
  elif [ "$MODE" = "restart" ]; then
    (cd "$RUNNER_DIR" && exec cargo run -p hotpatch-spike) >> "$LOG" 2>&1 < /dev/null &
  else
    local -a cmd=("$FRUST" run --watch)
    [ "$MODE" = "frust-restart" ] && cmd+=(--no-hot --features frust/hotpatch)
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
  if [ -n "$APP_PID" ] && kill -0 "$APP_PID" 2>/dev/null; then
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
  if [ "$FRUST_MODE" = 1 ]; then
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

cleanup() {
  stop_runner
  local file copy
  while IFS=$'\t' read -r file copy; do
    write_in_place "$file" "$copy"
  done < "$RESTORE_LIST"
  echo
  if [ "$FRUST_MODE" = 1 ]; then
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
    if [ -n "$APP_PID" ] && ! kill -0 "$APP_PID" 2>/dev/null; then return 1; fi
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
  kb="$(ps -o rss= -p "$1" 2>/dev/null | tr -d ' ')"
  if [ -n "$kb" ]; then awk -v k="$kb" 'BEGIN { printf "%.1f", k / 1024 }'; else echo "?"; fi
}

# --frust-run: the session dir's newest `stub-N.o` / `patch-N.<ext>` written after the save at `$1`
# (unix ms): `host: stub-N.o at save+<ms>, patch-N.dylib at save+<ms> (<bytes> bytes, mode <octal>)`.
host_artifacts() {
  python3 - "${APP_TARGET_DIR%/}/frust-hotpatch/session-${APP_BIN}" "$1" <<'PY'
import os, re, sys
d, stamp = sys.argv[1], int(sys.argv[2])
best = {}
try:
    names = os.listdir(d)
except OSError:
    names = []
for n in names:
    m = re.fullmatch(r"(stub|patch)-(\d+)\.(o|dylib|so|dll)", n)
    if not m:
        continue
    st = os.stat(os.path.join(d, n))
    t = int(st.st_mtime * 1000)
    if t > stamp and (m.group(1) not in best or int(m.group(2)) > best[m.group(1)][0]):
        best[m.group(1)] = (int(m.group(2)), n, t, st.st_size, st.st_mode & 0o777)
parts = []
if "stub" in best:
    parts.append(f"{best['stub'][1]} at save+{best['stub'][2] - stamp}")
if "patch" in best:
    _, n, t, size, mode = best["patch"]
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
[ "$FRUST_MODE" = 1 ] && echo "frust: ${FRUST} ($("$FRUST" --version 2>/dev/null)); app: ${APP} (${APP_BIN})"
echo "Logs: ${LOG_DIR}"
echo "If this script dies without its EXIT trap, restore the sources in place with:"
while IFS=$'\t' read -r file copy; do
  echo "  cat '${copy}' > '${file}'"
done < "$RESTORE_LIST"

START_MS="$(now_ms)"
start_runner
echo "Waiting up to ${STARTUP_TIMEOUT}s for the first frame (a cold build compiles frust and wgpu)..."
if ! FIRST_FRAME="$(wait_for frame 0 0 "$STARTUP_TIMEOUT")"; then
  echo "error: no first 'frust-hotpatch: frame' line; see ${LOG}" >&2
  exit 1
fi
find_app_pid
echo "First frame after $((FIRST_FRAME - START_MS)) ms (app pid ${APP_PID:-?})."
if [ "$PRE_RUN_PAUSE" != "0" ]; then
  echo "Pausing ${PRE_RUN_PAUSE}s before the first edit (PRE_RUN_PAUSE)..."
  sleep "$PRE_RUN_PAUSE"
fi

APPLIED_DELTAS=""
FRAME_DELTAS=""
RESULTS=""
OUTCOMES=""
run=1
while [ "$run" -le "$RUNS" ]; do
  sleep 1
  from="$(log_lines)"
  if ! stamp="$(apply_edit "$EDIT_FILE" "$TARGET" "$run" "$EDIT_ORIG")"; then
    RESULTS="${RESULTS}run ${run}: edit failed"$'\n'
    break
  fi
  applied="" frame="" status="ok" outcome="" old_pid="$APP_PID" detail=""
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
  elif [ "$MODE" = "frust-run" ]; then
    outcome="$(wait_outcome "$from" 180)" || status="timeout"
    o_ms="${outcome%% *}" o_text="${outcome#* }"
    case "$o_text" in
      "patched in "*)
        status="patched"
        applied="$(wait_for applied "$from" "$stamp" "$EDIT_TIMEOUT")" || status="patched, no applied line"
        frame="$(wait_for frame "$from" "$stamp" "$EDIT_TIMEOUT")" || status="patched, no frame line"
        ;;
      "restart required: "*)
        status="restart"
        # The CLI kills the app and starts a fresh fat session: wait for the new process's frame.
        APP_PID=""
        frame="$(wait_for frame "$from" "$stamp" "$RESTART_TIMEOUT")" || status="restart, no relaunch frame"
        find_app_pid
        ;;
      "") ;;
      *) status="other" ;;
    esac
    [ -n "$outcome" ] && echo "    cli: ${o_text} [line at save+$((o_ms - stamp)) ms]"
    if [ "${status%%,*}" = "patched" ]; then
      echo "    $(host_artifacts "$stamp")"
      echo "    $(handoff_line "$from")"
    fi
    [ -n "$outcome" ] && OUTCOMES="${OUTCOMES}${status}"$'\t'"$((o_ms - stamp))"$'\n'
    trouble_lines "$from"
    if [ -n "$APP_PID" ] && ! kill -0 "$APP_PID" 2>/dev/null; then status="crashed"; fi
    if [ "$status" = "patched" ] && [ "$old_pid" != "$APP_PID" ]; then
      status="patched, but the app pid changed"
    fi
    detail="; pid ${old_pid:-?} -> ${APP_PID:-?}; rss $(rss_mb "${APP_PID:-0}") MB"
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
  line="run ${run}: save->applied ${a_delta} ms, save->frame ${f_delta} ms (${status})${detail}"
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
fi
