#!/bin/sh
# attest-spike: Ethereum sync-committee light-client update + MPT storage proof in a
# RISC Zero 3.0.5 guest. EXECUTOR ONLY: counts cycles, never proves.
#   ./run.sh            full run (all scenarios, breakdown, software-keccak variant, negatives, mainnet)
#   ./run.sh fast       headline scenario + guest B + negatives only
set -e
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$HOME/.risc0/bin:$PATH"
export CARGO_TARGET_DIR="$PWD/target"
export CARGO_BUILD_JOBS=4   # also caps the nested guest builds risc0-build starts
unset PROVE RISC0_PROVER
# Build first and stop on failure (so a stale binary is never run).
if ! nice -n 19 cargo build --release -j 4 -p host > build.log 2>&1; then
  grep -nE "error(\[|:)" -A12 build.log | sed -n 1,60p; echo "build failed, see build.log"; exit 1
fi
grep -E "Finished|was not used" build.log || true
mkdir -p results
nice -n 19 ./target/release/host fixtures "${1:-}" 2>&1 | tee "results/run-$(date -u +%Y%m%dT%H%M%SZ).txt"
