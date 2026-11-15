#!/bin/bash
# lcbench (branch lc-cycles): build under the shared lock, then run. EXECUTOR ONLY, no prover call.
#   ./build-lcbench.sh          zkcrypto guest only
#   BLST=1 ./build-lcbench.sh   with the blst guest (RISC Zero C toolchain on the external drive)
set -u
W=<local path>
export TMPDIR=$W/tmp CARGO_TARGET_DIR=$W/target-lcbench CARGO_BUILD_JOBS=4
. <local path>
if [ "${BLST:-0}" = 1 ]; then
  # Overriding the guest C compiler makes risc0-build skip its own C setup, including the
  # bigint2 flag the accelerated crates assert at compile time, so both are set here.
  TC=<local path>
  export CC_riscv32im_risc0_zkvm_elf=$TC/riscv32-unknown-elf-gcc AR_riscv32im_risc0_zkvm_elf=$TC/riscv32-unknown-elf-ar
  export CFLAGS_riscv32im_risc0_zkvm_elf="-march=rv32im -nostdlib" RISC0_FEATURE_bigint2=
fi
T=$(date -u +%Y%m%dT%H%M%SZ)
cd $W/src/lez-bridge/attest/lcbench
<local path> nice -n 19 cargo build --release -j 4 ${FEATURES:-} > $W/logs/build-$T.log 2>&1
rc=$?
tail -30 $W/logs/build-$T.log
[ $rc = 0 ] || { echo "BUILD FAILED rc=$rc ($W/logs/build-$T.log)"; exit $rc; }
nice -n 10 $CARGO_TARGET_DIR/release/lcbench ${RUNARGS:-} > $W/logs/run-$T.txt 2>&1
rc=$?
cat $W/logs/run-$T.txt
echo "run rc=$rc ($W/logs/run-$T.txt)"
