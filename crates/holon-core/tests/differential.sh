#!/bin/sh
# The evaluator against an earlier version of itself, case by case (tests/differential.rs).
#
#   sh crates/holon-core/tests/differential.sh [BEFORE_REF]
#
# Runs the generated cases with the working tree's eval.rs and primitives.rs ("after"), then with
# BEFORE_REF's (default main, for a change to the evaluator before it lands), puts the working
# tree's files back (even when a run fails), and lists every case whose outputs or observer calls
# differ, with both sides' full text of each. HOLON_DIFF_WORLDS (default 150) and HOLON_DIFF_CASES
# (default 2000) size the run; HOLON_DIFF_DIR (default target/differential) holds what it writes.
set -eu

BEFORE=${1:-main}
ROOT=$(git rev-parse --show-toplevel)
cd "$ROOT"
OUT=${HOLON_DIFF_DIR:-target/differential}
mkdir -p "$OUT"
export HOLON_DIFF_WORLDS=${HOLON_DIFF_WORLDS:-150}
export HOLON_DIFF_CASES=${HOLON_DIFF_CASES:-2000}
EVAL=crates/holon-core/src/eval.rs
PRIM=crates/holon-core/src/primitives.rs

cp "$EVAL" "$OUT/eval.rs.after"
cp "$PRIM" "$OUT/primitives.rs.after"
restore() {
  cp "$OUT/eval.rs.after" "$EVAL"
  cp "$OUT/primitives.rs.after" "$PRIM"
}
trap restore EXIT

side() {
  # $1: after | before; any further variables come from the caller's environment.
  cargo test -p holon-core --release --features eager-oracle --test differential -- --exact differential_record --nocapture
}

echo "== after (working tree)"
HOLON_DIFF_RECORD="$OUT/after.txt" HOLON_DIFF_SUMMARY="$OUT/after-summary.txt" side after

echo "== before ($BEFORE)"
git show "$BEFORE:$EVAL" > "$EVAL"
git show "$BEFORE:$PRIM" > "$PRIM"
HOLON_DIFF_RECORD="$OUT/before.txt" HOLON_DIFF_SUMMARY="$OUT/before-summary.txt" side before

awk 'NR == FNR { h[$1] = $2; next } !($1 in h) || h[$1] != $2 { print $1 }' "$OUT/before.txt" "$OUT/after.txt" > "$OUT/diverged.txt"
N=$(wc -l < "$OUT/diverged.txt" | tr -d ' ')
echo "== cases: $(wc -l < "$OUT/after.txt" | tr -d ' ') after, $(wc -l < "$OUT/before.txt" | tr -d ' ') before; diverged: $N"
if [ "$N" -gt 0 ]; then
  head -n 200 "$OUT/diverged.txt" > "$OUT/dump-ids.txt"
  HOLON_DIFF_DUMP="@$OUT/dump-ids.txt" HOLON_DIFF_DUMP_OUT="$OUT/before-dump.txt" side before
  restore
  HOLON_DIFF_DUMP="@$OUT/dump-ids.txt" HOLON_DIFF_DUMP_OUT="$OUT/after-dump.txt" side after
  echo "== the first 200 diverging cases, in full: $OUT/before-dump.txt and $OUT/after-dump.txt"
fi
grep -E "^(cases|eager|self)" "$OUT/after-summary.txt" | sed 's/^/after:  /'
grep -E "^(cases|eager|self)" "$OUT/before-summary.txt" | sed 's/^/before: /'
