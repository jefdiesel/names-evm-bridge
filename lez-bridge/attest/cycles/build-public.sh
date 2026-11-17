#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Branch public-update, build host only: build attest_cycles with the public guest
# (methods/attest_guest_public: blst needs RISC Zero's C toolchain for riscv32im) under the shared
# build lock, then run it. EXECUTOR ONLY: no prover call exists in attest_cycles.
#   build-public.sh                    build, then `attest_cycles public` (the recorded fixtures)
#   build-public.sh test               build, then `cargo test --release` (pins the cycle count)
#   build-public.sh workspace          lez-bridge: eth_attest with public-update + the panic-text test
#   build-public.sh run ARGS...        no build: run attest_cycles ARGS
#   build-public.sh cli                the lez-bridge CLI (release, RISC0_SKIP_BUILD) into target-cli
# W: the work directory (src/ = rsync of the worktree's lez-bridge/, logs/, tmp/, target dirs).
set -u
W=${W:-<local path>}
LOCK=<local path>
export TMPDIR=$W/tmp CARGO_BUILD_JOBS=4
mkdir -p "$W/logs" "$W/tmp"
. <local path>
free_gb() { df -g / | awk 'NR==2{print $4}'; }
[ "$(free_gb)" -ge 16 ] || { echo "system disk: $(free_gb) GB free, under 16 GB: not building"; exit 1; }
T=$(date -u +%Y%m%dT%H%M%SZ)
case "${1:-public}" in
  workspace)
    export CARGO_TARGET_DIR=$W/target RISC0_SKIP_BUILD=1
    cd "$W/src/lez-bridge"
    $LOCK nice -n 19 cargo test -j 4 -p eth_attest -p bridge_core --features eth_attest/public-update \
      --test public_update --test consumer --test error_codes --test finality_split --test instance --test audit --test panic_texts > "$W/logs/workspace-$T.log" 2>&1
    rc=$?; tail -40 "$W/logs/workspace-$T.log"; echo "workspace rc=$rc ($W/logs/workspace-$T.log)"; echo "disk free $(free_gb) GB"; exit $rc ;;
  run) shift; exec nice -n 10 "$W/target-cycles/release/attest_cycles" "$@" ;;
  cli)
    # The lez-bridge CLI with the public-path commands (release, guests not embedded: deploy
    # takes --elf). target-cli is an APFS clone of an earlier release CLI build.
    export CARGO_TARGET_DIR=$W/target-cli RISC0_SKIP_BUILD=1
    cd "$W/src/lez-bridge"
    $LOCK nice -n 19 cargo build --release -j 4 -p lez_bridge_cli > "$W/logs/cli-$T.log" 2>&1
    rc=$?; grep -E "^(error|warning: unused)" -A6 "$W/logs/cli-$T.log" | sed -n 1,40p; tail -3 "$W/logs/cli-$T.log"
    echo "cli rc=$rc ($W/logs/cli-$T.log)"; echo "disk free $(free_gb) GB"; exit $rc ;;
esac
export CARGO_TARGET_DIR=$W/target-cycles
# As attest/lcbench/build-lcbench.sh: overriding the guest C compiler makes risc0-build skip its
# own C setup, including the bigint2 flag the accelerated crates assert at compile time.
TC=<local path>
export CC_riscv32im_risc0_zkvm_elf=$TC/riscv32-unknown-elf-gcc AR_riscv32im_risc0_zkvm_elf=$TC/riscv32-unknown-elf-ar
export CFLAGS_riscv32im_risc0_zkvm_elf="-march=rv32im -nostdlib" RISC0_FEATURE_bigint2=
cd "$W/src/lez-bridge/attest/cycles"
if [ "${1:-public}" = test ]; then
  $LOCK nice -n 19 cargo test --release -j 4 > "$W/logs/test-$T.log" 2>&1
  rc=$?; tail -40 "$W/logs/test-$T.log"; echo "test rc=$rc ($W/logs/test-$T.log)"; echo "disk free $(free_gb) GB"; exit $rc
fi
$LOCK nice -n 19 cargo build --release -j 4 > "$W/logs/build-$T.log" 2>&1
rc=$?
tail -15 "$W/logs/build-$T.log"
echo "disk free $(free_gb) GB"
[ $rc = 0 ] || { echo "BUILD FAILED rc=$rc ($W/logs/build-$T.log)"; exit $rc; }
ls -la "$CARGO_TARGET_DIR"/riscv-guest/attest_cycles/*/riscv32im-risc0-zkvm-elf/release/*.bin 2>/dev/null
nice -n 10 "$CARGO_TARGET_DIR/release/attest_cycles" public 2>&1 | tee "$W/logs/public-$T.txt"
echo "run rc=${PIPESTATUS[0]} ($W/logs/public-$T.txt)"
