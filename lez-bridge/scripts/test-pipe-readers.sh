#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# test-pipe-readers.sh: the early-exit reader trap, and a check that it does not come back.
#
#   lez-bridge/scripts/test-pipe-readers.sh      (from anywhere; shell and python3 only, no Rust, ~10 s)
#
# The trap (the judge demo's CI run of 2026-11-14 failed on it): `E=$($CLI exit-commitment … | head -1)`
# under `set -euo pipefail`. head exits after the first line; the CLI prints a second line, Rust's
# println! meets the closed pipe and panics ("failed printing to stdout: Broken pipe"), pipefail
# turns that into a failure and set -e ends the script without a word. Whether the CLI's second
# write comes after head has gone is down to scheduling: never seen on an idle machine, seen on a
# loaded 2-vCPU CI runner. Readers that read to the end (`sed -n 1p`, `grep` without -q/-m, a file)
# have no such race.
#
# Part 1 runs a stand-in for the CLI (two lines, each its own write; a write to a closed pipe is an
# error, as in Rust) with a pause between the lines, so the race is lost every time: `| head -1`
# must fail, `| sed -n 1p` and e2e-roundtrip.sh's own `to` (copied out of that file) must not.
# Part 2 refuses the pattern in the repository's shell scripts: a pipe into `head`, `grep -m`,
# `sed … q` or `awk … exit`; and in the judge demo's two scripts also a pipe into `grep -q`
# (use a here-string or a file). A line that must keep one says why in a `# pipe-ok: …` comment
# (public-demo/prove/ keeps a few `| head` behind sed, grep or pgrep, which read to the end).
set -uo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
fails=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=$((fails + 1)); }

# ------------------------------------------------------------------------------- 1. behaviour
TMP=$(mktemp -d "${TMPDIR:-/tmp}/pipe-readers.XXXXXX"); trap 'rm -rf "$TMP"' EXIT
cat > "$TMP/two.py" <<'PY'
import sys, time
sys.stdout.write("0xabc\n"); sys.stdout.flush()
time.sleep(0.05)
sys.stdout.write("release nullifier 0xdef\n"); sys.stdout.flush()
PY
TO=$(sed -n '/^to() {/,/return "\$rc"; }$/p' "$ROOT/lez-bridge/scripts/e2e-roundtrip.sh")
[ -n "$TO" ] || bad "could not find to() in lez-bridge/scripts/e2e-roundtrip.sh"
case_run() { # $1: reader; runs under set -euo pipefail like the e2e scripts; exit 0 = got the first line
  bash -c 'set -euo pipefail; LABEL=test
    two() { python3 -I "$1/two.py"; }
    '"$TO"'
    case $2 in
      head) E=$(two "$1" | head -1) ;;  # pipe-ok: the trap itself, kept here on purpose
      sed)  E=$(two "$1" | sed -n 1p) ;;
      to)   to "$1/out.txt" two "$1" > /dev/null; E=$(sed -n 1p "$1/out.txt") ;;
    esac
    [ "$E" = 0xabc ]' _ "$TMP" "$1" > /dev/null 2>&1
}
if case_run head; then bad "| head -1 did not fail (the stand-in no longer shows the trap)"; else ok "| head -1 fails when the writer prints after the reader left"; fi
case_run sed && ok "| sed -n 1p reads to the end: no failure" || bad "| sed -n 1p failed"
case_run to && ok "to (e2e-roundtrip.sh) writes to a file first: no failure" || bad "to failed"

# ------------------------------------------------------------------------------- 2. no early readers
cd "$ROOT" || exit 1
EARLY='\|[[:space:]]*(head([[:space:]]|$)|grep[^|]*[[:space:]]-m[[:space:]]*[0-9]|sed[^|]*;[[:space:]]*q|awk[^|]*exit)'
QUIET='\|[[:space:]]*grep[[:space:]]+-[a-zA-Z]*q'
scan() { # $1 pattern, $2 label, files...
  local pat=$1 label=$2 f hits; shift 2
  for f in "$@"; do
    hits=$(grep -nE "$pat" "$f" | grep -vE '^[0-9]+:[[:space:]]*#|# pipe-ok' || true)
    [ -z "$hits" ] || { bad "$f: $label"; echo "$hits" | sed 's/^/       /'; }
  done
}
FILES=$(git ls-files '*.sh' | grep -vx 'lez-bridge/scripts/test-pipe-readers.sh')
# shellcheck disable=SC2086  # one path per line, none with spaces
scan "$EARLY" "a pipe into a reader that exits early (head, grep -m, sed …q, awk …exit)" $FILES
scan "$QUIET" "a pipe into grep -q (use a here-string or a file)" scripts/judge-demo.sh lez-bridge/scripts/e2e-roundtrip.sh
[ "$fails" = 0 ] && ok "no early-exit readers in $(echo "$FILES" | wc -l | tr -d ' ') shell scripts"

echo; [ "$fails" = 0 ] && echo "PASS" || echo "FAILED: $fails"
exit $((fails > 0))
