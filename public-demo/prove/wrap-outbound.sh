#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# The Groth16 wrap of a succinct receipt from prove-outbound-rc3.sh (or `outbound_host release
# --prove`), on an x86-64 Linux box with Docker. It compresses an existing proof; it proves no guest.
#
#   wrap-outbound.sh --check --receipt FILE --image-id H --out DIR   every gate; nothing pulled or run
#   wrap-outbound.sh         --receipt FILE --image-id H --out DIR   pulls the prover image if missing, wraps, checks
#   wrap-outbound.sh --status --out DIR                              one line
#
# Needs: x86-64 Linux; Docker usable by this user; `outbound_wrap` (lez-bridge/outbound/wrap,
# built on this box: `cargo build --release --locked`, rust 1.98.1 from lez-bridge/rust-toolchain.toml);
# MIN_MEM_GB available memory (default 16; the prover's real need is measured here, see mem.log);
# MIN_DISK_GB free on the out dir and on Docker's root (default 10; the image is 2.3 GB compressed).
# The prover image is the one risc0-groth16 3.0.6 names: risczero/risc0-groth16-prover:v2026-10-20.1
# (gnark; built from risc0 v3.1.0 groth16_proof/docker/prover.Dockerfile). Whether it needs ADX is
# not stated by RISC Zero; the script reports the CPU flags and the run shows it (an illegal
# instruction would end the Docker step with an error).
# Outputs in DIR: groth16.bin, seal.bin (selector 0xe71e6620 + 256 bytes), seal.hex, journal.hex,
# wrap.json, verify-call.txt (the read-only `cast call` on Sepolia's RiscZeroGroth16Verifier 3.1.0),
# out-wrap.log, time.log (/usr/bin/time -v), mem.log (free memory and the container's memory, every 5 s), status.
set -uo pipefail
IMAGE=risczero/risc0-groth16-prover:v2026-10-20.1
WRAP=${OUTBOUND_WRAP:-$HOME/work/outbound-wrap/src/lez-bridge/outbound/wrap/target/release/outbound_wrap}
MIN_MEM_GB=${MIN_MEM_GB:-16}
MIN_DISK_GB=${MIN_DISK_GB:-10}
LOCK=${WRAP_LOCK:-/tmp/wrap-outbound.lock}

MODE=run RECEIPT="" IMAGE_ID="" OUT=""
while [ $# -gt 0 ]; do
  case "$1" in
    --check) MODE=check; shift ;;
    --status) MODE=status; shift ;;
    --receipt) RECEIPT=${2:-}; shift 2 ;;
    --image-id) IMAGE_ID=${2:-}; shift 2 ;;
    --out) OUT=${2:-}; shift 2 ;;
    *) echo "usage: $0 [--check|--status] --receipt FILE --image-id H --out DIR"; exit 2 ;;
  esac
done
[ -n "$OUT" ] || { echo "--out DIR is required"; exit 2; }
if [ $MODE = status ]; then cat "$OUT/status" 2>/dev/null || echo "never started"; exit 0; fi
[ -n "$RECEIPT" ] && [ -n "$IMAGE_ID" ] || { echo "--receipt and --image-id are required"; exit 2; }
mkdir -p "$OUT"
say() { echo "## $(date '+%F %T') $*"; }
status() { [ $MODE = run ] && echo "$*" > "$OUT/status"; return 0; }
stop() { local rc=$1; shift; echo "STOP: $*"; status "stopped $(date +%H:%M) (exit $rc): $*"; exit "$rc"; }

say "Groth16 wrap $([ $MODE = check ] && echo "CHECK ONLY" || echo RUN): $RECEIPT (image $IMAGE_ID) -> $OUT"
status "checks since $(date +%H:%M)"
[ "$(uname -s)/$(uname -m)" = Linux/x86_64 ] || stop 3 "needs x86-64 Linux (RISC Zero's Groth16 prover is x86 only); this is $(uname -s)/$(uname -m)"
flags=$(grep -o -w -E 'adx|bmi2|avx2|avx512f' /proc/cpuinfo | sort -u | tr '\n' ' ')
echo "CPU: $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2- | sed 's/^ //'), $(nproc) threads; flags: ${flags:-none of adx/bmi2/avx2/avx512f}"
case " $flags " in *" adx "*) ;; *) echo "note: no ADX on this CPU; RISC Zero does not say whether the prover needs it, so this run finds out" ;; esac
command -v docker >/dev/null || stop 3 "docker is not installed"
docker info >/dev/null 2>&1 || stop 3 "docker is not usable by $(id -un) (daemon down or not in the docker group)"
[ -x "$WRAP" ] || stop 3 "no outbound_wrap at $WRAP (OUTBOUND_WRAP=…)"
avail=$(awk '/MemAvailable/ {print int($2/1048576)}' /proc/meminfo)
[ "$avail" -ge "$MIN_MEM_GB" ] || stop 8 "only ${avail} GB memory available (need $MIN_MEM_GB)"
root=$(docker info -f '{{.DockerRootDir}}' 2>/dev/null)
for d in "$OUT" "${root:-/var/lib/docker}"; do
  f=$(df -P -BG "$d" 2>/dev/null | awk 'NR==2 {gsub("G","",$4); print $4}')
  [ "${f:-0}" -ge "$MIN_DISK_GB" ] || stop 8 "only ${f:-?} GB free on $d (need $MIN_DISK_GB)"
done
echo "memory available ${avail} GB, disk OK, docker $(docker version -f '{{.Server.Version}}' 2>/dev/null)"
"$WRAP" check --receipt "$RECEIPT" --image-id "$IMAGE_ID" || stop 4 "the input receipt does not verify natively against $IMAGE_ID"
if docker image inspect "$IMAGE" >/dev/null 2>&1; then echo "image $IMAGE present"; else echo "image $IMAGE not pulled yet (a run pulls it, 2.3 GB compressed)"; fi
if [ -d "$LOCK" ] && kill -0 "$(cat "$LOCK/pid" 2>/dev/null)" 2>/dev/null; then
  stop 9 "another wrap is running (pid $(cat "$LOCK/pid"))"
fi
if [ $MODE = check ]; then say "CHECK ONLY: gates passed; nothing pulled or wrapped"; exit 0; fi

mkdir "$LOCK" 2>/dev/null || { rm -rf "$LOCK"; mkdir "$LOCK"; }
echo $$ > "$LOCK/pid"
SAMPLER=""
cleanup() { local rc=$?; [ -n "$SAMPLER" ] && kill "$SAMPLER" 2>/dev/null; rm -rf "$LOCK"; echo "## $(date '+%F %T') exit $rc"; }
trap cleanup EXIT
if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
  say "pulling $IMAGE"; status "pulling the prover image since $(date +%H:%M)"
  docker pull "$IMAGE" || stop 5 "docker pull failed"
fi
echo "image $(docker image inspect -f '{{.Id}} {{.Size}} bytes' "$IMAGE")"
( while :; do
    echo "$(date +%T) avail=$(awk '/MemAvailable/ {print int($2/1024)}' /proc/meminfo)MB $(c=$(docker ps -q --filter ancestor="$IMAGE"); [ -n "$c" ] && docker stats --no-stream --format '{{.Name}}={{.MemUsage}} cpu={{.CPUPerc}}' $c 2>/dev/null | tr '\n' ' ')"
    sleep 5
  done ) > "$OUT/mem.log" 2>&1 &
SAMPLER=$!
say "WRAP"; status "wrapping since $(date +%H:%M); log $OUT/out-wrap.log"
/usr/bin/time -v -o "$OUT/time.log" "$WRAP" wrap --receipt "$RECEIPT" --image-id "$IMAGE_ID" --out "$OUT" 2>&1 | tee "$OUT/out-wrap.log"
grep -a -q "^WRAPPED:" "$OUT/out-wrap.log" || stop 2 "the wrap did not finish (see $OUT/out-wrap.log, $OUT/mem.log)"
"$WRAP" check --receipt "$OUT/groth16.bin" --image-id "$IMAGE_ID" | tee -a "$OUT/out-wrap.log" | grep -a "^VERIFIED: groth16" >/dev/null || stop 6 "the Groth16 receipt did not verify natively"
peak=$(awk '{for (i=1;i<=NF;i++) if ($i ~ /=/ && $i ~ /iB/) {split($i,a,"="); split(a[2],b,"/"); v=b[1]; u=v; gsub(/[0-9.]/,"",u); gsub(/[A-Za-z]/,"",v); if (u=="GiB") v*=1024; if (u=="KiB") v/=1024; if (v>m) m=v}} END {printf "%d", m}' "$OUT/mem.log")
min_avail=$(sed -n 's/.*avail=\([0-9]*\)MB.*/\1/p' "$OUT/mem.log" | sort -n | sed -n 1p)
{
  echo "=== GROTH16 WRAP (send this) ==="
  grep -a -E "^WRAPPED:|^JOURNAL_DIGEST=" "$OUT/out-wrap.log"
  grep -E "Elapsed \(wall clock\)|Maximum resident set size" "$OUT/time.log" | sed 's/^\t*/outbound_wrap (host process) /'
  echo "prover container peak memory about ${peak:-?} MB (docker stats every 5 s); lowest available memory on the box ${min_avail:-?} MB"
  echo "CPU flags: ${flags:-none}"
  echo "read-only check on Sepolia: $(cat "$OUT/verify-call.txt" | cut -c1-120)… (full command in $OUT/verify-call.txt)"
  echo "==="
} | tee "$OUT/summary.txt"
status "done $(date +%H:%M): Groth16 receipt verified natively; seal $OUT/seal.bin; on-chain check: $OUT/verify-call.txt"
exit 0
