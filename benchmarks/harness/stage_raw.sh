#!/usr/bin/env bash
# benchmarks/harness/stage_raw.sh — stage a matrix.sh out tree's sanitized raws AFTER the fact
# (e.g. a run that was interrupted before its own staging step). Same rules as matrix.sh's
# staging block: copies run-NN.log / run-NN.pss_*.txt / stats.txt / coldstart.txt / cpuinfo.txt
# of every block whose blocks.tsv verdict is `ok` (attempt 1 preferred, else attempt 2) into
# <out>/raw/<device-name>/{frust_profile,flutter}/<sN>/ and runs the .gitignore whitelist checks.
# Usage: stage_raw.sh <out-dir> <device-name> <runs> [scenario ...]   (default: all scenarios seen)
set -uo pipefail
OUT="$(cd "$1" && pwd -P)"; DEV="$2"; RUNS="$3"; shift 3
BLOCKS="$OUT/blocks.tsv"; STAGE="$OUT/raw/$DEV"; ok=1
[ -f "$BLOCKS" ] || { echo "no blocks.tsv in $OUT" >&2; exit 1; }
if [ $# -eq 0 ]; then set -- $(awk -F'\t' 'NR>1{print $3}' "$BLOCKS" | sort -u); fi
pick() { local v1 v2
  v1="$(awk -F'\t' -v a="$1" -v s="$2" '$2==a && $3==s && $4=="1"{print $NF}' "$BLOCKS" | tail -1)"
  v2="$(awk -F'\t' -v a="$1" -v s="$2" '$2==a && $3==s && $4=="2"{print $NF}' "$BLOCKS" | tail -1)"
  if [ "$v1" = ok ]; then echo "$OUT/$1/$2"; elif [ "$v2" = ok ]; then echo "$OUT/$1/$2.retry1"; else echo ""; fi; }
for s in "$@"; do for app in frust flutter; do
  src="$(pick "$app" "$s")"; [ -n "$src" ] && [ -d "$src" ] || { echo "skip $app/$s (no ok block)"; continue; }
  if [ "$app" = frust ]; then dst="$STAGE/frust_profile/$s"; else dst="$STAGE/flutter/$s"; fi
  rm -rf "$dst"; mkdir -p "$dst"
  cp "$src"/run-[0-9][0-9].log "$dst"/ || ok=0
  ls "$src"/run-[0-9][0-9].pss_*.txt >/dev/null 2>&1 && cp "$src"/run-[0-9][0-9].pss_*.txt "$dst"/
  for f in stats.txt coldstart.txt cpuinfo.txt; do [ -f "$src/$f" ] && cp "$src/$f" "$dst/$f"; done
  n="$(ls "$dst"/run-[0-9][0-9].log 2>/dev/null | wc -l | tr -d ' ')"; [ "$n" -eq "$RUNS" ] || { echo "$dst has $n logs, expected $RUNS"; ok=0; }
  echo "staged $app/$s -> $dst ($n logs)"
done; done
if grep -rlvE 'frust-perf|flutter-perf|bench-scenario|^[[:space:]]*$' "$STAGE" --include='*.log' | grep -q .; then echo "UNSANITIZED .log lines"; ok=0; fi
if grep -rlvE '^-- stats\.py( --dclass)? \(discarding first [0-9]+ runs?, per protocol convention\) --$|^== [A-Za-z0-9_. ()-]+ ==$|^frames: [0-9]+ total, [0-9]+ active, [0-9]+ skipped$|^p50=[0-9.]+ms[[:space:]]+p95=[0-9.]+ms[[:space:]]+p99=[0-9.]+ms[[:space:]]+worst=[0-9.]+ms$|^missed_60hz=[0-9]+ \(budget [0-9.]+ms\)[[:space:]]+missed_120hz=[0-9]+ \(budget [0-9.]+ms\)$|^[[:space:]]*$' "$STAGE" --include='stats.txt' | grep -q .; then echo "stats.txt outside whitelist"; ok=0; fi
if [ "$ok" = 1 ]; then touch "$OUT/RAW_OK"; echo "RAW_OK"; else rm -f "$OUT/RAW_OK"; touch "$OUT/RAW_CHECK_FAILED"; echo "RAW_CHECK_FAILED"; exit 1; fi
