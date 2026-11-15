#!/bin/sh
# Run one command (e.g. a private-proving lez-bridge call) under the test machine's memory and
# thermal watchdog: pause (SIGSTOP) below PAUSE_PCT free memory and resume above RESUME_PCT;
# kill below KILL_PCT, above MAX_FOOTPRINT_GB, or under a thermal limit. Exit status is the
# command's.
#   scripts/guard.sh <command> [args...]
#
# Memory is the process's physical FOOTPRINT (/usr/bin/footprint), not RSS. RSS under-reads
# once macOS compresses pages: on 2026-11-12 the prover showed 6.5 GB RSS at a 9.4 GB
# footprint, so the old RSS cap never fired. MAX_RSS_GB is no longer used (callers that still
# set it are unaffected). The free-memory rule is what protects the machine; the footprint cap
# only stops a runaway process early.
PAUSE_PCT="${PAUSE_PCT:-15}"; RESUME_PCT="${RESUME_PCT:-25}"; KILL_PCT="${KILL_PCT:-8}"
MAX_FOOTPRINT_GB="${MAX_FOOTPRINT_GB:-11}"
LOG="${GUARD_LOG:-/dev/null}"
"$@" &
PID=$!
max_mb=$((MAX_FOOTPRINT_GB * 1024)); paused=0; peak=0; peak_rss=0; start=$(date +%s)
footprint_mb() {
  /usr/bin/footprint "$1" 2>/dev/null | awk '/Footprint:/ {
    for (i = 1; i <= NF; i++) if ($i == "Footprint:") { v = $(i+1); u = $(i+2) }
    if (u == "KB") v = v / 1024; else if (u == "GB") v = v * 1024; else if (u == "B") v = 0
    printf "%d", v; exit }'
}
while kill -0 $PID 2>/dev/null; do
  rss=$(ps -o rss= -p $PID 2>/dev/null | awk '{print int($1/1024)}')
  fp=$(footprint_mb $PID); fp=${fp:-0}
  [ "$fp" -gt "$peak" ] && peak=$fp
  [ "${rss:-0}" -gt "$peak_rss" ] && peak_rss=$rss
  free=$(memory_pressure -Q 2>/dev/null | awk -F': ' '/percentage/ {gsub("%","",$2); print $2+0}')
  therm=$(pmset -g therm 2>/dev/null | awk -F'= ' '/CPU_Speed_Limit/ {print $2+0}')
  echo "$(date '+%T') footprint=${fp}MB rss=${rss:-0}MB free=${free}% speed_limit=${therm:-100} paused=$paused" >> "$LOG"
  if [ "$fp" -gt "$max_mb" ] || [ "${free:-100}" -lt "$KILL_PCT" ] || [ "${therm:-100}" -lt 100 ]; then
    echo "guard: hard limit crossed (footprint=${fp}MB free=${free}% speed_limit=${therm:-100}), killing $1" >&2; kill -9 $PID
  elif [ $paused = 0 ] && [ "${free:-100}" -lt "$PAUSE_PCT" ]; then
    echo "guard: memory tight, pausing" >&2; kill -STOP $PID; paused=1
  elif [ $paused = 1 ] && [ "${free:-0}" -gt "$RESUME_PCT" ]; then
    echo "guard: memory back, resuming" >&2; kill -CONT $PID; paused=0
  fi
  sleep 2
done
wait $PID; rc=$?
echo "guard: exit=$rc wall=$(( $(date +%s) - start ))s peak_footprint=${peak}MB peak_rss=${peak_rss}MB" >&2
exit $rc
