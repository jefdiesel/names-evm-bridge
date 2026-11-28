#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# e2e-ci.sh: end-to-end integration test driver for CI (.github/workflows/e2e.yml, RFP-021 Sup2).
#
#   bash scripts/e2e-ci.sh [--keep] [--purge] [--work DIR] [--build-only kit|bridge|outbound] [--help]
#
# What it runs, with no secrets and no network beyond GitHub/crates/apt (for initial builds):
#   1. Local LEZ v0.4.0-rc3 standalone sequencer (from the pinned testnet kit in VERSIONS.toml,
#      running with RISC0_DEV_MODE=1 so no real zkVM proving happens) + local Ethereum (anvil).
#   2. Deploy the RFP-021 attestation program (eth_attest) and its reference consumer
#      (eth_attest_probe), immutable, then exercise the light-client path from recorded Sepolia
#      fixtures (lez-bridge/attest/fixtures/sepolia/, served over a local loopback HTTP mock beacon):
#        - same-period finality update (period 1411 -> slot 11563424);
#        - committee handoff across period 1410 -> 1411 (period update + rotating finality update);
#        - storage-slot statement verified on chain through eth_attest_probe (public tx);
#        - wrong-chain update refused + stale weak-subjectivity checkpoint refused (B1.3).
#      Note: on main, eth_attest uses unmodified Helios (37-46M cycles, over LEZ's 10M public cap),
#      so `attest init` and `attest prove-slot` run as ordinary public transactions while `attest
#      update` runs from a private account under RISC0_DEV_MODE=1 (dev receipts, real execution).
#   3. Deploy the wrapped-token and bridge programs on LEZ and the BridgeVault (RoundTripDeploy.s.sol)
#      + DepositDesk (DepositDesk.sol) on anvil.
#   4. Complete round trip reusing lez-bridge/scripts/e2e-roundtrip.sh and
#      public-demo/relayer/relayer.py: deposits on anvil (direct + through DepositDesk) -> claims
#      on LEZ (direct + relayed into a never-used zero-LEZ account) -> burns on LEZ -> G2-lite
#      outbound updates -> G3 releases on anvil, asserting balances at every step and reconciling
#      solvency and privacy checks at the end.
#   5. Local security e2e cases reusing lez-bridge/scripts/e2e-security-enc.py:
#        - double-claim refused (CLI pre-check + on-chain revert, supply unchanged);
#        - wrong-chain refused (wrong LEZ channel id on init + wrong EVM chain id / cfg_id on claim,
#          CLI pre-check and on-chain revert);
#        - frozen token refused (LEZ per-token mint freeze, unfreeze + recovery proving nullifier
#          not spent, LEZ per-token burn freeze, Ethereum vault deposit freeze);
#        - fixed security regressions from e2e-security.sh (C1 header-key mint, H1 re-init, M2
#          sponsor cap).
#
# Options:
#   --keep      leave anvil, the LEZ chain, the relay and the run directory after finishing
#   --purge     delete the build cache directory (WORK/build) at the end
#   --work DIR  where builds and run state go (default: run/e2e-ci in this checkout)
#   --build-only kit|bridge|outbound
#               only build that part (the kit's sequencer and wallet, the bridge CLI, or the
#               outbound host) and stop: e2e.yml builds the three in parallel jobs, then runs the
#               test with the binaries given as SEQ_BIN/WALLET_BIN/LEZ_BRIDGE_BIN/OUTBOUND_HOST_BIN
#
# Environment (all optional):
#   LEZ_PORT (6460; the kit also takes +1, +5, +6), ANVIL_PORT (6470), RELAY_PORT (6472),
#   BEACON_PORT (6474), BLOCK_TIME (5 s), RELEASE_DELAY (20 s), CARGO_BUILD_JOBS (4),
#   BUILD_WRAP (default "nice -n 10"), RUN_WRAP (default "env"),
#   KIT_URL, KIT_COMMIT (pinned to match VERSIONS.toml [judge_demo]),
#   SEQ_BIN, WALLET_BIN, LEZ_BRIDGE_BIN, OUTBOUND_HOST_BIN (prebuilt binary overrides).
set -uo pipefail
export LC_ALL=C

REPO=$(cd "$(dirname "$0")/.." && pwd)
KEEP=0; PURGE=0; WORK=${WORK:-}; BUILD_ONLY=""
usage() { sed -n '3,51p' "$0" | sed 's/^# \{0,1\}//'; }
while [ $# -gt 0 ]; do
  case $1 in
    --keep) KEEP=1; shift ;;
    --purge) PURGE=1; shift ;;
    --work) WORK=${2:?--work needs a directory}; shift 2 ;;
    --build-only) BUILD_ONLY=${2:?--build-only needs kit, bridge or outbound}; shift 2
      case $BUILD_ONLY in kit|bridge|outbound) ;; *) echo "--build-only: kit, bridge or outbound" >&2; exit 2 ;; esac ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1 (see --help)" >&2; exit 2 ;;
  esac
done
WORK=${WORK:-$REPO/run/e2e-ci}
mkdir -p "$WORK"; WORK=$(cd "$WORK" && pwd)

# Pinned LEZ v0.4.0-rc3 testnet kit commit (matches VERSIONS.toml [judge_demo]).
KIT_URL=${KIT_URL:-https://github.com/jefdiesel/logos-execution-zone.git}
KIT_COMMIT=${KIT_COMMIT:-96a0a50232203ceae0cb8e69ef564d2d4a5d34a1}
LEZ_PORT=${LEZ_PORT:-6460}; ANVIL_PORT=${ANVIL_PORT:-6470}
RELAY_PORT=${RELAY_PORT:-6472}; BEACON_PORT=${BEACON_PORT:-6474}
BLOCK_TIME=${BLOCK_TIME:-5}
RELEASE_DELAY=${RELEASE_DELAY:-20}
JOBS=${CARGO_BUILD_JOBS:-4}
BUILD_WRAP=${BUILD_WRAP:-nice -n 10}
RUN_WRAP=${RUN_WRAP:-env}
STAMP=$(date +%Y%m%d-%H%M%S)
B=$WORK/build
RUN=$WORK/run-$STAMP
R=$RUN/roundtrip
TRANSCRIPT=$WORK/e2e-ci-$STAMP.log

export PATH="$HOME/.foundry/bin:$HOME/.risc0/bin:$HOME/.cargo/bin:$PATH"
export LEE_WALLET_PASSWORD=
# Dev mode for private LEZ transactions: never allow RISC0_PROVER=local to override it here.
unset RISC0_PROVER RISC0_SERVER_PATH BONSAI_API_URL BONSAI_API_KEY
export RISC0_DEV_MODE=1

SEQ_URL=http://127.0.0.1:$LEZ_PORT
ETH_RPC=http://127.0.0.1:$ANVIL_PORT
BEACON_URL=http://127.0.0.1:$BEACON_PORT
ETH=0x0000000000000000000000000000000000000000
D_ETH=10000000000000000         # 0.01 ETH (RoundTripDeploy.s.sol denom 0)
DF_ETH=100000000000000          # 1% vault deposit fee
WF_ETH=50000000000000           # 0.5% vault withdraw fee
XMAX_ETH=100000000000000        # max relay fee in X's exit commitment
D_USDC=100000000                # 100 USDC (RoundTripDeploy.s.sol denom 0)
DESK_FEE_BPS=30                 # 0.3% demo desk fee
FX=$REPO/lez-bridge/attest/fixtures/sepolia
FX_MAIN=$REPO/lez-bridge/attest/fixtures/mainnet
T_START=$(date +%s)

mkdir -p "$RUN" "$B"
chmod 700 "$RUN"

# Mirror output to the full transcript and truncate overlong lines on stdout (CI log limit).
exec > >(python3 -I -c '
import os, signal, sys
for s in (signal.SIGINT, signal.SIGTERM):
    signal.signal(s, signal.SIG_IGN)
log, out, lim = open(sys.argv[1], "ab", buffering=0), sys.stdout.buffer, 2000
for line in iter(sys.stdin.buffer.readline, b""):
    log.write(line)
    if out is None:
        continue
    if len(line) > lim:
        line = line[:lim] + b" ... (%d more characters in the transcript)\n" % (len(line) - lim)
    try:
        out.write(line); out.flush()
    except OSError:
        out = None
        os.dup2(os.open(os.devnull, os.O_WRONLY), 1)
' "$TRANSCRIPT") 2>&1

# ------------------------------------------------------------------------------------- labels
L_CHAIN="local LEZ v0.4.0-rc3 standalone sequencer (RISC0_DEV_MODE=1: dev receipts, no real proving)"
L_ETH="local Ethereum node (anvil) with BridgeVault (MockVerifier) + DepositDesk"
L_LC="recorded Sepolia fixtures via local loopback mock beacon; Init & prove-slot are public txs, Update runs via private account under RISC0_DEV_MODE=1 (main eth_attest is 37-46M cycles)"
L_RT="e2e-roundtrip.sh + DepositDesk + relayer.py: proofs executed, not proved; LEZ finality trusted; MockVerifier"
L_SEC="local security checks: double-claim, wrong-chain, frozen token (LEZ + vault), C1/H1/M2 regressions"

# ------------------------------------------------------------------------------------- steps
N=0; CUR=""; CURLABEL=""; T0=0; FAILED=0; LAST_OUT=""
S_NAME=(); S_RES=(); S_LABEL=(); S_SECS=()
hr() { echo "------------------------------------------------------------------------------------------"; }
step() { N=$((N + 1)); CUR=$1; CURLABEL=${2:-}; T0=$(date +%s); LAST_OUT=""; echo; hr; echo "STEP $N. $CUR"; [ -z "$CURLABEL" ] || echo "  label: $CURLABEL"; hr; }
record() { S_NAME+=("$N. $CUR"); S_RES+=("$1"); S_LABEL+=("${CURLABEL:--}"); S_SECS+=("$(( $(date +%s) - T0 ))"); }
pass() { echo "PASS: step $N. $CUR ($(( $(date +%s) - T0 ))s)"; record PASS; }
fail() {
  echo "FAIL: step $N. $CUR: $*"
  if [ -n "$LAST_OUT" ] && [ -s "$LAST_OUT" ]; then
    echo "  last lines of $LAST_OUT:"; tail -n 25 "$LAST_OUT" | cut -c1-300 | sed 's/^/    /'
  fi
  record FAIL; FAILED=1; finish
}
show() { echo "\$ $*"; }
run() { show "$@"; "$@"; }
note() { echo "  $*"; }

summary() {
  echo; hr; echo "SUMMARY ($(date '+%F %T'), wall clock $(( ($(date +%s) - T_START) / 60 )) min $(( ($(date +%s) - T_START) % 60 )) s)"; hr
  local i
  for i in "${!S_NAME[@]}"; do
    printf '%-4s  %5ss  %s\n' "${S_RES[$i]}" "${S_SECS[$i]}" "${S_NAME[$i]}"
    printf '               label: %s\n' "${S_LABEL[$i]}"
  done
  [ "$FAILED" = 0 ] && echo "ALL STEPS PASSED" || echo "A STEP FAILED (see FAIL above); later steps did not run"
  echo "Transcript: $TRANSCRIPT"
}

ANVIL_PID=""; RELAY_PID=""; BEACON_PID=""
cleanup() {
  if [ "$KEEP" = 1 ]; then
    echo; echo "--keep: still running: chain (kit home $RUN/kit), anvil ${ANVIL_PID:+pid $ANVIL_PID}, relay ${RELAY_PID:+pid $RELAY_PID}, mock beacon ${BEACON_PID:+pid $BEACON_PID}"
    return
  fi
  echo; echo "cleanup: stopping mock beacon, relay, anvil and the chain, deleting $RUN"
  [ -n "$BEACON_PID" ] && kill "$BEACON_PID" 2>/dev/null || true
  [ -n "$RELAY_PID" ] && kill "$RELAY_PID" 2>/dev/null || true
  [ -n "$ANVIL_PID" ] && kill "$ANVIL_PID" 2>/dev/null || true
  [ -d "$RUN/kit" ] && "$B/kit/testnet/down.sh" --home "$RUN/kit" >/dev/null 2>&1 || true
  sleep 1
  rm -rf "$RUN"
  [ "$PURGE" = 1 ] && { echo "cleanup: --purge, deleting $B"; rm -rf "$B"; }
}
DONE_FINISH=0
finish() {
  [ "$DONE_FINISH" = 1 ] && return; DONE_FINISH=1
  summary; cleanup; scrub_transcript
  exit "$FAILED"
}
on_signal() {
  echo "interrupted"
  if [ "$N" -gt "${#S_NAME[@]}" ]; then fail "interrupted (INT or TERM)"; else FAILED=1; finish; fi
}
trap on_signal INT TERM
on_exit() {
  local rc=$?
  [ "$DONE_FINISH" = 1 ] && return
  [ "$N" -gt "${#S_NAME[@]}" ] || { N=$((N + 1)); CUR="(after the last step)"; }
  fail "the script stopped unexpectedly (exit status $rc)"
}
trap on_exit EXIT

scrub_transcript() {
  sleep 1
  JD_BUILD_WRAP=$BUILD_WRAP JD_RUN_WRAP=$RUN_WRAP python3 -I - "$TRANSCRIPT" "${TRANSCRIPT%.log}.public.log" "$REPO" "$WORK" "$HOME" "$(id -un)" "$(hostname -s 2>/dev/null || hostname)" <<'PY'
import os, re, sys
src, dst, repo, work, home, user, host = sys.argv[1:]
t = open(src, errors="replace").read()
for w in (os.environ.get("JD_BUILD_WRAP", ""), os.environ.get("JD_RUN_WRAP", "")):
    if "/" in w:
        t = t.replace(w, "<build wrapper>")
for a, b in ((work, "<work>"), (repo, "<repo>"), (home, "~")):
    if a and len(a) > 1:
        t = t.replace(a, b)
for w, b in ((host, "<host>"), (user, "<user>")):
    if w and len(w) > 2:
        t = re.sub(r"\b%s\b" % re.escape(w), b, t)
t = re.sub(r"\b(pk|With pk) [0-9a-fA-F]{64}\b", r"\1 <public key>", t)
keyword = re.compile(r"priv(?:ate)?.?key|secret|seed|mnemonic|signing.?key|\bsk\b|\bpk\b", re.I)
t = "\n".join(re.sub(r"(?:0x)?[0-9a-fA-F]{64}(?![0-9a-fA-F])", "<hex>", l) if keyword.search(l) else l for l in t.split("\n"))
t = re.sub(r"\x1b\[[0-9;]*m", "", t)
t = re.sub(r"(?m)^(Transaction data is .{0,160}).*$", r"\1 ...", t)
open(dst, "w").write(t)
PY
  echo "public copy of the transcript: ${TRANSCRIPT%.log}.public.log"
}

port_free() { python3 -I -c 'import socket,sys; s=socket.socket(); s.settimeout(0.5); sys.exit(0 if s.connect_ex(("127.0.0.1", int(sys.argv[1]))) else 1)' "$1"; }
idof() { grep -oE "$1/[1-9A-HJ-NP-Za-km-z]{32,44}" | sed -n 1p | cut -d/ -f2; }
pkof() { sed -n 's/^With pk \([0-9a-fA-F]\{64\}\).*/\1/p' | sed -n 1p; }
lower() { tr 'A-F' 'a-f'; }
rpc() { curl -s -m 10 -X POST "$SEQ_URL/" -H 'Content-Type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":$2}"; }
acct() { rpc getAccount "[\"$1\"]" | python3 -I -c '
import json,sys
r=json.load(sys.stdin).get("result")
if r is None: print("ABSENT (never used)")
else:
    raw=((r.get("data") or {}).get("actor_states") or {}).get("11111111111111111111111111111111") or []
    print("PRESENT nonce=%s native=%d" % (r.get("nonce"), int.from_bytes(bytes(raw),"little")))'; }
native() { acct "$1" | sed -n 's/.*native=\([0-9]*\).*/\1/p'; }
newsecret() { ( umask 077; python3 -I -c 'import secrets; print(secrets.token_hex(32))' > "$1" ); }
ebal() { cast balance "$1" --rpc-url "$ETH_RPC"; }
tbal() { cast call "$1" "balanceOf(address)(uint256)" "$2" --rpc-url "$ETH_RPC" | cut -d' ' -f1; }
supply_of() { "$@" 2>&1 | sed -n 's/^supply //p' | sed -n 1p; }
enc() { python3 -I "$REPO/lez-bridge/scripts/e2e-security-enc.py" "$@"; }

# Assert a CLI call fails with expected substring $2 (output written to $1).
assert_cli_refuses() {
  local outf=$1 want=$2; shift 2
  LAST_OUT=$outf
  if "$@" > "$outf" 2>&1; then
    fail "expected refusal ($want), but command succeeded"
  fi
  grep -qiF "$want" "$outf" || fail "refused, but expected text '$want' not found"
  note "refused as expected ($want): $(tail -n 1 "$outf" | cut -c1-160)"
}

# Assert a CLI call sent with --allow-revert is included and reverted ON CHAIN.
assert_reverts_onchain() {
  local outf=$1; shift
  LAST_OUT=$outf
  if "$@" > "$outf" 2>&1; then
    fail "expected on-chain revert, but transaction applied"
  fi
  grep -q "but reverted" "$outf" || fail "failed before inclusion instead of reverting on chain"
  note "reverted on chain: $(grep -oE '[0-9a-f]{64} was included in block [0-9]+ but reverted' "$outf" | sed -n 1p)"
}

# Assert an Ethereum cast send reverts with Solidity custom error $1 (e.g. IsFrozen).
assert_eth_reverts() {
  local want=$1 key=$2; shift 2
  local outf=$RUN/sec/eth-revert-$want.out
  LAST_OUT=$outf
  if cast send "$@" --private-key "$key" --rpc-url "$ETH_RPC" > "$outf" 2>&1; then
    fail "expected Ethereum revert $want, but transaction succeeded"
  fi
  local sel; sel=$(cast sig "$want()")
  grep -qiE "${sel#0x}|$want" "$outf" || fail "expected Ethereum revert $want ($sel)"
  note "Ethereum reverted as expected: $want"
}

echo "e2e-ci $(date '+%F %T')  repo $(git -C "$REPO" rev-parse --short HEAD 2>/dev/null || echo '?')  $(uname -sm)"
echo "work $WORK  run $RUN"

# ===================================================================================== 1
# want PART: true when this run builds or uses PART (all of them, or only --build-only's).
want() { [ -z "$BUILD_ONLY" ] || [ "$BUILD_ONLY" = "$1" ]; }
step "Prerequisites${BUILD_ONLY:+ (build only: $BUILD_ONLY)}"
missing=0
need() { command -v "$1" >/dev/null 2>&1 || { echo "  missing: $1"; missing=1; }; }
OS=$(uname -s); ARCH=$(uname -m)
case "$OS/$ARCH" in Linux/x86_64|Linux/aarch64|Darwin/arm64) note "platform $OS/$ARCH" ;; *) echo "  unsupported platform $OS/$ARCH"; missing=1 ;; esac
if [ "$OS/$ARCH" = Linux/x86_64 ]; then
  for f in adx bmi2; do
    grep -qw "$f" /proc/cpuinfo 2>/dev/null || { echo "  CPU lacks $f (required by LEZ prebuilt rapidsnark)"; missing=1; }
  done
fi
for t in git curl python3 shasum; do need "$t"; done
[ -n "$BUILD_ONLY" ] || for t in forge cast anvil; do need "$t"; done
BUILDING=0
if [ -n "$BUILD_ONLY" ]; then BUILDING=1
else
  [ -n "${SEQ_BIN:-}" ] && [ -n "${WALLET_BIN:-}" ] && [ -n "${LEZ_BRIDGE_BIN:-}" ] && [ -n "${OUTBOUND_HOST_BIN:-}" ] || BUILDING=1
fi
if [ "$BUILDING" = 1 ]; then
  for t in cargo rustup clang pkg-config rzup; do need "$t"; done
  if [ "$OS" = Linux ]; then
    pkg-config --exists libpcsclite 2>/dev/null || { echo "  missing libpcsclite-dev"; missing=1; }
    pkg-config --exists openssl 2>/dev/null || { echo "  missing libssl-dev"; missing=1; }
  fi
  ls -d "$HOME"/.risc0/toolchains/*rust* >/dev/null 2>&1 || { echo "  missing RISC Zero Rust toolchain"; missing=1; }
  ls -d "$HOME"/.risc0/extensions/v3.0.5-cargo-risczero* >/dev/null 2>&1 || { echo "  missing cargo-risczero 3.0.5"; missing=1; }
fi
if [ -z "$BUILD_ONLY" ]; then
  for p in $LEZ_PORT $((LEZ_PORT + 1)) $((LEZ_PORT + 5)) $((LEZ_PORT + 6)) $ANVIL_PORT $RELAY_PORT $BEACON_PORT; do
    port_free "$p" || { echo "  port $p is in use"; missing=1; }
  done
fi
for t in git cargo forge anvil python3 rzup; do command -v "$t" >/dev/null 2>&1 && note "$t: $($t --version 2>&1 | sed -n 1p)"; done
[ "$missing" = 0 ] || fail "missing prerequisites above"
pass

# ===================================================================================== 2
if want kit || [ -z "$BUILD_ONLY" ]; then
step "Fetch the pinned LEZ v0.4.0-rc3 testnet kit (${KIT_COMMIT:0:8})"
mkdir -p "$B"
if [ ! -d "$B/kit/.git" ]; then
  run git init -q "$B/kit" && run git -C "$B/kit" remote add origin "$KIT_URL" || fail "git init"
fi
if [ "$(git -C "$B/kit" rev-parse HEAD 2>/dev/null)" != "$KIT_COMMIT" ]; then
  run git -C "$B/kit" fetch -q --depth 1 origin "$KIT_COMMIT" || fail "fetch $KIT_URL $KIT_COMMIT"
  run git -C "$B/kit" -c advice.detachedHead=false checkout -q FETCH_HEAD || fail "checkout"
fi
got=$(git -C "$B/kit" rev-parse HEAD)
[ "$got" = "$KIT_COMMIT" ] || fail "kit is at $got, want $KIT_COMMIT"
if [ -f "$B/kit/lez/testnet_initial_state/src/lib.rs" ]; then
  export LEZ_INITIAL_STATE_RS="$B/kit/lez/testnet_initial_state/src/lib.rs"
  note "LEZ_INITIAL_STATE_RS=$LEZ_INITIAL_STATE_RS (offline genesis test key read)"
fi
pass
fi

# risc0-circuit-recursion 4.0.4 (LEZ rc3's prover, in the kit and the bridge CLI) downloads its
# recursion_zkr.zip at build time from RISC Zero's S3 bucket, which answers 403 since 2026-11
# (seen 2026-11-28). The same file is in RISC Zero's own repository (Git LFS); its build script
# takes it from RECURSION_SRC_PATH and checks the SHA-256 it pins, as this does first.
ZKR_SHA256=744b999f0a35b3c86753311c7efb2a0054be21727095cf105af6ee7d3f4d8849
ZKR_URL=${ZKR_URL:-https://media.githubusercontent.com/media/risc0/risc0/v3.0.5/risc0/circuit/recursion/src/recursion_zkr.zip}
fetch_zkr() {
  [ -n "${RECURSION_SRC_PATH:-}" ] && return 0
  local z=$B/recursion_zkr.zip
  if [ ! -f "$z" ] || [ "$(shasum -a 256 "$z" | cut -d' ' -f1)" != "$ZKR_SHA256" ]; then
    run curl -fsSL --retry 3 -o "$z.part" "$ZKR_URL" || fail "download recursion_zkr.zip"
    [ "$(shasum -a 256 "$z.part" | cut -d' ' -f1)" = "$ZKR_SHA256" ] || fail "recursion_zkr.zip: wrong SHA-256"
    mv "$z.part" "$z"
  fi
  export RECURSION_SRC_PATH=$z
  note "RECURSION_SRC_PATH=$z (sha256 ${ZKR_SHA256:0:16}..., risc0-circuit-recursion 4.0.4)"
}

# ===================================================================================== 3
if want kit; then
step "Build (or reuse cached) LEZ v0.4.0-rc3 sequencer and wallet"
if [ -n "${SEQ_BIN:-}" ] && [ -n "${WALLET_BIN:-}" ] && [ -z "$BUILD_ONLY" ]; then
  CURLABEL="prebuilt: $SEQ_BIN, $WALLET_BIN"
else
  fetch_zkr
  show "P=$B/kit-bin CARGO_TARGET_DIR=$B/target-kit $B/kit/testnet/sequencer/build.sh"
  P=$B/kit-bin CARGO_TARGET_DIR=$B/target-kit LEZ_BUILD_WRAP="$BUILD_WRAP" CARGO_BUILD_JOBS=$JOBS \
    "$B/kit/testnet/sequencer/build.sh" || fail "kit build (logs in $B/kit-bin/logs)"
  SEQ_BIN=$B/kit-bin/bin/sequencer_service
  WALLET_BIN=$B/kit-bin/bin/wallet
fi
[ -x "$SEQ_BIN" ] && [ -x "$WALLET_BIN" ] || fail "missing sequencer/wallet binary"
for b in "$SEQ_BIN" "$WALLET_BIN"; do
  "$b" --help >/dev/null 2>&1
  [ $? = 132 ] && fail "$b stopped with Illegal instruction"
done
pass
fi

# ===================================================================================== 4
if want bridge || want outbound; then
step "Build (or reuse cached) ${BUILD_ONLY:-bridge CLI (with token/bridge/attest guests), outbound host, and vault}"
if want bridge && { [ -z "${LEZ_BRIDGE_BIN:-}" ] || [ -n "$BUILD_ONLY" ]; }; then
  fetch_zkr
  show "(cd lez-bridge && CARGO_TARGET_DIR=$B/target-bridge $BUILD_WRAP cargo build --release --locked -j $JOBS -p lez_bridge_cli)"
  LAST_OUT=$B/build-cli.log
  # shellcheck disable=SC2086
  (cd "$REPO/lez-bridge" && CARGO_TARGET_DIR=$B/target-bridge $BUILD_WRAP cargo build --release --locked -j "$JOBS" -p lez_bridge_cli) > "$LAST_OUT" 2>&1 \
    || fail "CLI build"
  LEZ_BRIDGE_BIN=$B/target-bridge/release/lez-bridge
fi
if want outbound && { [ -z "${OUTBOUND_HOST_BIN:-}" ] || [ -n "$BUILD_ONLY" ]; }; then
  show "(cd lez-bridge/outbound && CARGO_TARGET_DIR=$B/target-outbound $BUILD_WRAP cargo build --release --locked -j $JOBS -p outbound_host)"
  LAST_OUT=$B/build-outbound.log
  # shellcheck disable=SC2086
  (cd "$REPO/lez-bridge/outbound" && CARGO_TARGET_DIR=$B/target-outbound $BUILD_WRAP cargo build --release --locked -j "$JOBS" -p outbound_host) > "$LAST_OUT" 2>&1 \
    || fail "outbound_host build"
  OUTBOUND_HOST_BIN=$B/target-outbound/release/outbound_host
fi
if [ -z "$BUILD_ONLY" ]; then
  if [ ! -f "$REPO/vault/lib/forge-std/src/Test.sol" ] || [ ! -d "$REPO/vault/lib/openzeppelin-contracts/contracts" ]; then
    run git -C "$REPO" submodule update --init --recursive vault/lib/forge-std vault/lib/openzeppelin-contracts || fail "vault submodules"
  fi
  LAST_OUT=$B/build-vault.log
  (cd "$REPO/vault" && forge build) > "$LAST_OUT" 2>&1 || fail "forge build"
fi
CLI=${LEZ_BRIDGE_BIN:-}; HOST=${OUTBOUND_HOST_BIN:-}
want bridge && { "$CLI" --help >/dev/null 2>&1 || fail "$CLI --help"; note "CLI $(shasum -a 256 "$CLI" | cut -c1-16)..."; }
want outbound && { note "outbound_host $(shasum -a 256 "$HOST" | cut -c1-16)..."; run "$HOST" ids || fail "outbound_host ids"; }
pass
fi
[ -z "$BUILD_ONLY" ] || finish

# ===================================================================================== 5
step "Start local LEZ v0.4.0-rc3 standalone sequencer (RISC0_DEV_MODE=1) and anvil" "$L_CHAIN; $L_ETH"
show "RISC0_DEV_MODE=1 SEQ_BIN=... WALLET_BIN=... $B/kit/testnet/up.sh --genesis-key default --port $LEZ_PORT --block-time $BLOCK_TIME --home $RUN/kit"
LAST_OUT=$RUN/kit-up.log
RISC0_DEV_MODE=1 SEQ_BIN=$SEQ_BIN WALLET_BIN=$WALLET_BIN "$B/kit/testnet/up.sh" \
  --genesis-key default --port "$LEZ_PORT" --block-time "$BLOCK_TIME" --home "$RUN/kit" > "$LAST_OUT" 2>&1 \
  || fail "kit up.sh"
tail -n 5 "$LAST_OUT" | sed 's/^/  /'
# Best-effort check that the running sequencer inherited RISC0_DEV_MODE=1.
seq_pid=$(pgrep -f "sequencer_service.*$RUN/kit" | sed -n 1p || true)
if [ -n "$seq_pid" ] && [ -r "/proc/$seq_pid/environ" ]; then
  env_lines=$(tr '\0' '\n' < "/proc/$seq_pid/environ")
  if grep -qx "RISC0_DEV_MODE=1" <<<"$env_lines"; then
    note "confirmed sequencer pid $seq_pid runs with RISC0_DEV_MODE=1"
  else
    fail "sequencer pid $seq_pid did NOT inherit RISC0_DEV_MODE=1 (kit up.sh may clear env)"
  fi
else
  note "sequencer pid env check skipped (not readable on this host); RISC0_DEV_MODE=1 was exported to up.sh"
fi

show "anvil --host 127.0.0.1 --port $ANVIL_PORT --silent &"
anvil --host 127.0.0.1 --port "$ANVIL_PORT" --silent > "$RUN/anvil.log" 2>&1 < /dev/null &
ANVIL_PID=$!
for _ in $(seq 1 30); do cast chain-id --rpc-url "$ETH_RPC" >/dev/null 2>&1 && break; sleep 1; done
EVM_CHAIN_ID=$(cast chain-id --rpc-url "$ETH_RPC" 2>/dev/null) || fail "anvil did not answer"
ANVIL0=$("$REPO/lez-bridge/scripts/anvil-dev-key.sh" 0) || fail "anvil-dev-key 0"
ANVIL3=$("$REPO/lez-bridge/scripts/anvil-dev-key.sh" 3) || fail "anvil-dev-key 3"
ANVIL0_ADDR=$(cast wallet address --private-key "$ANVIL0")
LOCAL_TEST_KEY=$("$REPO/lez-bridge/scripts/lez-test-key.sh") || fail "lez-test-key"
note "anvil pid $ANVIL_PID (chain id $EVM_CHAIN_ID), deployer0 $ANVIL0_ADDR"
pass

# ===================================================================================== 6
step "RFP-021: deploy eth_attest & eth_attest_probe (immutable) and start local mock beacon" "$L_LC"
LCW=$RUN/wallet-lc; mkdir -p "$LCW" "$RUN/lc"; chmod 700 "$LCW"
lccli() { LEE_WALLET_HOME_DIR=$LCW LEZ_BRIDGE_SEQUENCER=$SEQ_URL "$CLI" "$@"; }
LAST_OUT=$LCW/first-run.txt
lccli wallet check-health > "$LAST_OUT" 2>&1 || fail "lc wallet check-health"
chmod 600 "$LAST_OUT"
LAST_OUT=$RUN/lc/import-f.txt
lccli wallet account import public --private-key "$LOCAL_TEST_KEY" > "$LAST_OUT" 2>&1 || fail "import genesis key"
LC_F=$(idof Public < "$LAST_OUT")
LAST_OUT=$RUN/lc/new-lp.txt
lccli wallet account new public > "$LAST_OUT" 2>&1 || fail "new public account LP"
LP=$(idof Public < "$LAST_OUT")
LAST_OUT=$RUN/lc/fund-lp.txt
lccli wallet auth-transfer send --from "Public/$LC_F" --to "Public/$LP" --amount 200000000000 > "$LAST_OUT" 2>&1 || fail "fund LP"
LAST_OUT=$RUN/lc/new-v.txt
lccli wallet account new private > "$LAST_OUT" 2>&1 || fail "new private account V"
LC_V=$(idof Private < "$LAST_OUT")
[ -n "$LP" ] && [ -n "$LC_V" ] || fail "could not create LP or LC_V"
note "LC public payer $LP, private updater $LC_V (RISC0_DEV_MODE=1)"

LAST_OUT=$RUN/lc/shield-v.txt
show "lez-bridge shield --from $LP --to $LC_V --amount 500000000   (RISC0_DEV_MODE=1)"
lccli shield --from "$LP" --to "$LC_V" --amount 500000000 > "$LAST_OUT" 2>&1 || fail "shield LC_V"

LAST_OUT=$RUN/lc/deploy-attest.txt
show "lez-bridge deploy --which attest --payer $LP"
lccli deploy --which attest --payer "$LP" > "$LAST_OUT" 2>&1 || fail "deploy attest"
AP=$(sed -n 's/^program //p' "$LAST_OUT" | sed -n 1p)
grep -qx "immutable" "$LAST_OUT" || fail "eth_attest was not deployed immutable"

LAST_OUT=$RUN/lc/deploy-probe.txt
show "lez-bridge deploy --which attest-probe --payer $LP"
lccli deploy --which attest-probe --payer "$LP" > "$LAST_OUT" 2>&1 || fail "deploy attest-probe"
PP=$(sed -n 's/^program //p' "$LAST_OUT" | sed -n 1p)
grep -qx "immutable" "$LAST_OUT" || fail "eth_attest_probe was not deployed immutable"
note "deployed eth_attest=$AP (immutable), eth_attest_probe=$PP (immutable)"

# Start loopback mock Beacon API serving ONLY local files from lez-bridge/attest/fixtures/sepolia.
python3 -I - "$FX" "$BEACON_PORT" > "$RUN/lc/beacon.log" 2>&1 <<'PY' &
import http.server, json, os, struct, sys, urllib.parse

fx, port = sys.argv[1], int(sys.argv[2])
meta = json.load(open(os.path.join(fx, "meta.json")))
fork_sched = open(os.path.join(fx, "fork_schedule.json"), "rb").read()
boots = {}
for name in ("bootstrap_same_period", "bootstrap_prev_period"):
    entry = meta[name]
    root = entry["block_root"].lower()
    boots[root] = (entry["slot"], open(os.path.join(fx, name + ".ssz"), "rb").read())
updates = {}
for u in meta["updates"]:
    p = int(u["period"])
    digest = bytes.fromhex(u["fork_digest"])
    payload = open(os.path.join(fx, f"update_period_{p}.ssz"), "rb").read()
    updates[p] = struct.pack("<Q", 4 + len(payload)) + digest + payload
fin_ssz = open(os.path.join(fx, "finality_update.ssz"), "rb").read()

class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, fmt, *a):
        sys.stdout.write("%s - %s\n" % (self.address_string(), fmt % a)); sys.stdout.flush()
    def send_raw(self, code, ctype, body, extra=None):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        for k, v in (extra or {}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body)
    def do_GET(self):
        u = urllib.parse.urlparse(self.path)
        p = u.path
        if p == "/eth/v1/beacon/genesis":
            body = json.dumps({"data": {
                "genesis_validators_root": meta["genesis_validators_root"],
                "genesis_time": str(meta["genesis_time"]),
            }}).encode()
            return self.send_raw(200, "application/json", body)
        if p == "/eth/v1/config/fork_schedule":
            return self.send_raw(200, "application/json", fork_sched)
        if p.startswith("/eth/v1/beacon/headers/"):
            root = p.rsplit("/", 1)[-1].lower()
            if root in boots:
                slot = boots[root][0]
                body = json.dumps({"data": {"root": root, "header": {"message": {"slot": str(slot)}}}}).encode()
                return self.send_raw(200, "application/json", body)
            return self.send_raw(404, "application/json", b'{"message":"unknown header"}')
        if p.startswith("/eth/v1/beacon/light_client/bootstrap/"):
            root = p.rsplit("/", 1)[-1].lower()
            if root in boots:
                return self.send_raw(200, "application/octet-stream", boots[root][1], {"eth-consensus-version": "gloas"})
            return self.send_raw(404, "application/json", b'{"message":"unknown bootstrap"}')
        if p == "/eth/v1/beacon/light_client/finality_update":
            return self.send_raw(200, "application/octet-stream", fin_ssz, {"eth-consensus-version": "gloas"})
        if p == "/eth/v1/beacon/light_client/updates":
            qs = urllib.parse.parse_qs(u.query)
            sp = int(qs.get("start_period", ["-1"])[0])
            if sp in updates:
                return self.send_raw(200, "application/octet-stream", updates[sp], {"eth-consensus-version": "gloas"})
            return self.send_raw(404, "application/json", b'{"message":"unknown period"}')
        return self.send_raw(404, "application/json", b'{"message":"not found"}')

http.server.HTTPServer(("127.0.0.1", port), H).serve_forever()
PY
BEACON_PID=$!
for _ in $(seq 1 20); do curl -s -m 2 "$BEACON_URL/eth/v1/beacon/genesis" >/dev/null 2>&1 && break; sleep 0.5; done
curl -fsS -m 5 "$BEACON_URL/eth/v1/beacon/genesis" >/dev/null || fail "mock beacon did not start"
note "mock beacon listening on $BEACON_URL (pid $BEACON_PID)"
pass

# ===================================================================================== 7
step "RFP-021: Sepolia light-client fixtures (finality update, committee handoff, storage proof, refusals)" "$L_LC"
# Read fixture metadata from meta.json so no hex constants are duplicated here.
eval "$(python3 -I - "$FX/meta.json" <<'PY'
import json, sys
m = json.load(open(sys.argv[1]))
fu = m["finality_update"]
st = m["storage"]
print("CP_SAME=" + m["bootstrap_same_period"]["block_root"])
print("CP_PREV=" + m["bootstrap_prev_period"]["block_root"])
print("FIN_SLOT=" + str(fu["finalized_slot"]))
print("FIN_SIG_SLOT=" + str(fu["signature_slot"]))
print("FIN_ANCHOR=" + fu["finalized_execution_block_hash"])
print("ST_CONTRACT=" + st["contract"])
print("ST_MSLOT=" + str(st["mapping_slot_index"]))
print("ST_MKEY=" + st["mapping_key"])
PY
)"
# Shift genesis_time for historical fixture replay so signature_slot is ~1h ago and ws_deadline is ~4-5 days ahead.
NOW_S=$(date +%s)
TEST_GENESIS_TIME=$(( NOW_S - FIN_SIG_SLOT * 12 - 3600 ))
SALT1=0x0000000000000000000000000000000000000000000000000000000000000001
SALT2=0x0000000000000000000000000000000000000000000000000000000000000002
SALT3=0x0000000000000000000000000000000000000000000000000000000000000003

# 7a. Same-period finality update (period 1411 -> slot 11563424)
LAST_OUT=$RUN/lc/init-same.txt
show "lez-bridge attest init (same-period Sepolia bootstrap, public tx)"
lccli attest init --program "$AP" --from "$LP" --network sepolia --checkpoint "$CP_SAME" \
  --beacon "$BEACON_URL" --bootstrap-file "$FX/bootstrap_same_period.ssz" \
  --salt "$SALT1" --test-genesis-time "$TEST_GENESIS_TIME" > "$LAST_OUT" 2>&1 || fail "attest init (same period)"
INST_SAME=$(sed -n 's/^INSTANCE=//p' "$LAST_OUT" | sed -n 1p)
[ -n "$INST_SAME" ] || fail "no INSTANCE from attest init (same period)"

LAST_OUT=$RUN/lc/update-same.txt
show "lez-bridge attest update --kind finality (same period, RISC0_DEV_MODE=1)"
lccli attest update --program "$AP" --instance "$INST_SAME" --from "$LC_V" \
  --beacon "$BEACON_URL" --kind finality --update-file "$FX/finality_update.ssz" > "$LAST_OUT" 2>&1 \
  || fail "attest update (same-period finality)"

LAST_OUT=$RUN/lc/show-same.json
lccli --json attest show --program "$AP" --instance "$INST_SAME" --anchor-block-hash "$FIN_ANCHOR" > "$LAST_OUT" 2> "$RUN/lc/show-same.err" \
  || fail "attest show (same period)"
python3 -I - "$LAST_OUT" "$FIN_SLOT" "$FIN_ANCHOR" <<'PY' || fail "same-period finality assertions"
import json, sys
doc = json.load(open(sys.argv[1]))
want_slot, want_anchor = int(sys.argv[2]), sys.argv[3].lower()
assert doc["finalized_slot"] == want_slot, f"slot {doc['finalized_slot']} != {want_slot}"
assert doc["updates"] == 1, f"updates {doc['updates']} != 1"
assert doc["asked_anchor"]["recorded"] is True, "finalized anchor not recorded"
assert doc["asked_anchor"]["value"].lower() == want_anchor, "anchor value mismatch"
print(f"  same-period instance: slot={doc['finalized_slot']}, updates={doc['updates']}, anchor recorded=True")
PY

# 7b. Committee handoff (period 1410 -> period 1411 -> finalized slot 11563424)
LAST_OUT=$RUN/lc/init-prev.txt
show "lez-bridge attest init (prev-period Sepolia bootstrap, period 1410, public tx)"
lccli attest init --program "$AP" --from "$LP" --network sepolia --checkpoint "$CP_PREV" \
  --beacon "$BEACON_URL" --bootstrap-file "$FX/bootstrap_prev_period.ssz" \
  --salt "$SALT2" --test-genesis-time "$TEST_GENESIS_TIME" > "$LAST_OUT" 2>&1 || fail "attest init (prev period)"
INST_HO=$(sed -n 's/^INSTANCE=//p' "$LAST_OUT" | sed -n 1p)
[ -n "$INST_HO" ] || fail "no INSTANCE from attest init (prev period)"

LAST_OUT=$RUN/lc/show-ho-0.txt
lccli attest show --program "$AP" --instance "$INST_HO" > "$LAST_OUT" 2>&1 || fail "attest show (handoff 0)"
C_BEFORE=$(sed -n 's/.*current committee \(0x[0-9a-f]*\).*/\1/p' "$LAST_OUT" | sed -n 1p)
[ -n "$C_BEFORE" ] || fail "could not read initial current committee"
note "handoff instance $INST_HO initial committee $C_BEFORE (period 1410)"

# Before the period-1410 handoff, the period-1411 finality update must be refused.
assert_cli_refuses "$RUN/lc/refuse-early-fin.txt" "invalid sync committee period" \
  lccli attest update --program "$AP" --instance "$INST_HO" --from "$LC_V" \
  --beacon "$BEACON_URL" --kind finality --update-file "$FX/finality_update.ssz"

LAST_OUT=$RUN/lc/update-p1410.txt
show "lez-bridge attest update --kind period (period 1410 -> learns period 1411 committee, RISC0_DEV_MODE=1)"
lccli attest update --program "$AP" --instance "$INST_HO" --from "$LC_V" \
  --beacon "$BEACON_URL" --kind period --update-file "$FX/update_period_1410.ssz" > "$LAST_OUT" 2>&1 \
  || fail "attest update (period 1410)"

LAST_OUT=$RUN/lc/show-ho-1.txt
lccli attest show --program "$AP" --instance "$INST_HO" > "$LAST_OUT" 2>&1 || fail "attest show (handoff 1)"
C_NEXT=$(sed -n 's/.*next committee \(0x[0-9a-f]*\).*/\1/p' "$LAST_OUT" | sed -n 1p)
[ -n "$C_NEXT" ] || fail "next committee not learned after period-1410 update"
note "period-1410 update applied: next committee learned ($C_NEXT)"

LAST_OUT=$RUN/lc/update-ho-fin.txt
show "lez-bridge attest update --kind finality (period 1411 committee rotates in, RISC0_DEV_MODE=1)"
lccli attest update --program "$AP" --instance "$INST_HO" --from "$LC_V" \
  --beacon "$BEACON_URL" --kind finality --update-file "$FX/finality_update.ssz" > "$LAST_OUT" 2>&1 \
  || fail "attest update (handoff finality)"

LAST_OUT=$RUN/lc/show-ho-2.txt
lccli attest show --program "$AP" --instance "$INST_HO" --anchor-block-hash "$FIN_ANCHOR" > "$LAST_OUT" 2>&1 \
  || fail "attest show (handoff 2)"
C_AFTER=$(sed -n 's/.*current committee \(0x[0-9a-f]*\).*/\1/p' "$LAST_OUT" | sed -n 1p)
[ "$C_AFTER" = "$C_NEXT" ] && [ "$C_AFTER" != "$C_BEFORE" ] || fail "committee did not rotate ($C_BEFORE -> $C_AFTER, want $C_NEXT)"
grep -q "RECORDED by beacon slot $FIN_SLOT" "$LAST_OUT" || fail "handoff finalized anchor not recorded"
note "committee handoff confirmed on chain: $C_BEFORE -> $C_AFTER, finalized slot $FIN_SLOT"

# 7c. Reference consumer storage proof (ordinary public tx against the handoff anchor)
mkdir -p "$RUN/lc-slot"
cp "$FX/eth_getProof.json" "$RUN/lc-slot/eth_getProof.json"
cp "$FX/el_block_header.rlp.hex" "$RUN/lc-slot/header.rlp.hex"
LAST_OUT=$RUN/lc/prove-slot.txt
show "lez-bridge attest prove-slot (Sepolia fixture storage proof, public tx)"
lccli attest prove-slot --probe "$PP" --program "$AP" --instance "$INST_HO" --from "$LP" \
  --contract "$ST_CONTRACT" --mapping-slot "$ST_MSLOT" --key "$ST_MKEY" \
  --eq 1 --load-dir "$RUN/lc-slot" > "$LAST_OUT" 2>&1 || fail "attest prove-slot"
STMT_ID=$(sed -n 's/^STATEMENT_ID=//p' "$LAST_OUT" | sed -n 1p)
[ -n "$STMT_ID" ] || fail "no STATEMENT_ID from attest prove-slot"
note "storage statement verified on chain by eth_attest_probe: $STMT_ID"

# 7d. Negative light-client checks: wrong-chain update + stale weak-subjectivity checkpoint (B1.3)
if [ -f "$FX_MAIN/finality_update.ssz" ]; then
  # Refused natively before anything is sent. Its slots lie past this Sepolia store's bound, so the
  # first rule to refuse it is RFP021-E04; the chain mismatch itself is covered by the native tests.
  assert_cli_refuses "$RUN/lc/refuse-mainnet-on-sepolia.txt" "does not verify natively" \
    lccli attest update --program "$AP" --instance "$INST_HO" --from "$LC_V" \
    --beacon "$BEACON_URL" --kind finality --update-file "$FX_MAIN/finality_update.ssz"
fi
# The stale check is a differential test: the same finality update, from the same private account,
# that 7a applied to an instance whose genesis_time is shifted, sent to an instance with Sepolia's
# real genesis_time, whose window (it ends at the store's weak-subjectivity deadline, B1.3) closed
# on 2026-11-17. Natively the update verifies (slot rule passes); LEZ must refuse it on block time.
GENESIS_REAL=$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["genesis_time"])' "$FX/meta.json")
SAME_SLOT=$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["bootstrap_same_period"]["slot"])' "$FX/meta.json")
WS_DEADLINE=$(( GENESIS_REAL + (SAME_SLOT + 1348 * 32) * 12 ))   # ws_deadline_ms / 1000, ws_epochs(Sepolia) = 1348
[ "$(date +%s)" -gt "$WS_DEADLINE" ] || fail "precondition: the fixture's weak-subjectivity deadline ($WS_DEADLINE) has not passed yet"
note "unshifted instance: weak-subjectivity deadline $(date -u -d "@$WS_DEADLINE" '+%F %T' 2>/dev/null || echo "$WS_DEADLINE") UTC, passed"
LAST_OUT=$RUN/lc/init-stale.txt
lccli attest init --program "$AP" --from "$LP" --network sepolia --checkpoint "$CP_SAME" \
  --beacon "$BEACON_URL" --bootstrap-file "$FX/bootstrap_same_period.ssz" \
  --salt "$SALT3" > "$LAST_OUT" 2>&1 || fail "attest init (unshifted stale check)"
INST_STALE=$(sed -n 's/^INSTANCE=//p' "$LAST_OUT" | sed -n 1p)
[ -n "$INST_STALE" ] || fail "no INSTANCE from attest init (unshifted stale check)"
LAST_OUT=$RUN/lc/refuse-stale.txt
stale_rc=0
timeout 300 env LEE_WALLET_HOME_DIR="$LCW" LEZ_BRIDGE_SEQUENCER="$SEQ_URL" "$CLI" \
  attest update --program "$AP" --instance "$INST_STALE" --from "$LC_V" \
  --beacon "$BEACON_URL" --kind finality --update-file "$FX/finality_update.ssz" > "$LAST_OUT" 2>&1 || stale_rc=$?
echo "  stale update, exit $stale_rc; its last lines:"; grep -vE '^Transaction data|WARNING: [Pp]roving in dev mode' "$LAST_OUT" | tail -n 4 | cut -c1-300 | sed 's/^/    /'
[ "$stale_rc" != 0 ] || fail "stale 2026-11-12 fixture update unexpectedly applied past its weak-subjectivity deadline"
[ "$stale_rc" != 124 ] || fail "stale fixture update: the CLI did not return within 300 s"
grep -q "verified natively" "$LAST_OUT" || fail "stale update: expected it to verify natively and be refused by LEZ, not before"
# LEZ rc3 drops a private transaction whose window excludes the block time: the CLI reports it never
# included (seen 2026-11-28); a reverted inclusion or the window error text are accepted too.
grep -qiE "not included|outside of the validity window|but reverted" "$LAST_OUT" \
  || fail "stale update refused, but not by LEZ at inclusion (no 'not included' / validity window / revert)"
LAST_OUT=$RUN/lc/show-stale.json
lccli --json attest show --program "$AP" --instance "$INST_STALE" > "$LAST_OUT" 2> "$RUN/lc/show-stale.err" || fail "attest show (stale)"
python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); assert d["updates"] == 0, d["updates"]' "$LAST_OUT" \
  || fail "the stale instance's store advanced"
note "stale instance: the update 7a applied was dropped by LEZ (exit $stale_rc, past the weak-subjectivity deadline); its store has 0 updates"

kill "$BEACON_PID" 2>/dev/null || true; BEACON_PID=""
pass

# ------------------------------------------------------------------------------------- roundtrip helper
roundtrip() {
  show "R=$R RESUME=1 STOP_AFTER=$1 RELEASE_DELAY=$RELEASE_DELAY SEQ=$SEQ_URL ETH_RPC=$ETH_RPC lez-bridge/scripts/e2e-roundtrip.sh"
  LAST_OUT=$R/roundtrip.log
  R=$R RESUME=1 STOP_AFTER=$1 RELEASE_DELAY=$RELEASE_DELAY VAULT_DIR=$REPO/vault TGT=$B/target-outbound \
    HOST=$HOST CLI_SRC=$CLI SEQ=$SEQ_URL ETH_RPC=$ETH_RPC LOCK=$RUN_WRAP TMPDIR_OUTBOUND=$RUN/tmp \
    bash "$REPO/lez-bridge/scripts/e2e-roundtrip.sh"
}
rt_done() { grep -qx "$1" "$R/steps.done" 2>/dev/null; }
ids() { . "$R/ids.env"; }

# ===================================================================================== 8
step "Deploy bridge + token on LEZ, BridgeVault (RoundTripDeploy.s.sol) + DepositDesk on anvil" "$L_ETH"
mkdir -p "$R/wallet" "$RUN/tmp"; chmod 700 "$R/wallet"
LEE_WALLET_HOME_DIR=$R/wallet LEZ_BRIDGE_SEQUENCER=$SEQ_URL "$CLI" wallet check-health > "$R/wallet/first-run.txt" 2>&1
chmod 600 "$R/wallet/first-run.txt"
roundtrip programs && rt_done programs || fail "round trip: programs"
roundtrip vault && rt_done vault || fail "round trip: vault"
ids

# Deploy DepositDesk in front of the round-trip vault (feeBps = 30 = 0.3%).
LAST_OUT=$RUN/deploy-desk.json
show "forge create src/desk/DepositDesk.sol:DepositDesk --constructor-args $VAULT $ANVIL0_ADDR $DESK_FEE_BPS"
(cd "$REPO/vault" && forge create src/desk/DepositDesk.sol:DepositDesk \
  --rpc-url "$ETH_RPC" --private-key "$ANVIL0" --broadcast --json \
  --constructor-args "$VAULT" "$ANVIL0_ADDR" "$DESK_FEE_BPS") > "$LAST_OUT" 2> "$RUN/deploy-desk.err" || { cat "$RUN/deploy-desk.err"; fail "deploy DepositDesk"; }
# the JSON object is the last line forge prints that starts with "{"
DESK=$(python3 -I -c '
import json, sys
lines = [l for l in open(sys.argv[1]) if l.lstrip().startswith("{")]
print(json.loads(lines[-1])["deployedTo"])' "$LAST_OUT") || fail "parse DepositDesk address"
[ "$(cast call "$DESK" 'vault()(address)' --rpc-url "$ETH_RPC" | lower)" = "$(echo "$VAULT" | lower)" ] || fail "desk vault mismatch"
[ "$(cast call "$DESK" 'feeBps()(uint256)' --rpc-url "$ETH_RPC" | cut -d' ' -f1)" = "$DESK_FEE_BPS" ] || fail "desk feeBps mismatch"
note "DepositDesk deployed at $DESK (vault=$VAULT, feeBps=$DESK_FEE_BPS)"

roundtrip setup && rt_done setup || fail "round trip: setup"
ids
pass

# ===================================================================================== 9
step "Round trip: anvil deposits (direct + DepositDesk) -> LEZ claims (direct + relayed) -> burns -> G2 updates -> G3 releases" "$L_RT"
v_usdc_0=$(tbal "$USDC" "$VAULT"); v_eth_0=$(ebal "$VAULT"); d_eth_0=$(ebal "$DESK")
[ "$v_usdc_0" = 0 ] && [ "$v_eth_0" = 0 ] && [ "$d_eth_0" = 0 ] || fail "vault/desk non-zero before deposits"

# 9a. Deposits: P's 6 direct deposits, then X's 0.01 ETH deposit through DepositDesk.
roundtrip deposits && rt_done deposits || fail "round trip: deposits"
XW=$R/wallet-x; mkdir -p "$XW" "$RUN/x"; chmod 700 "$XW" "$RUN/x"
xcli() { LEE_WALLET_HOME_DIR=$XW LEZ_BRIDGE_SEQUENCER=$SEQ_URL "$CLI" "$@"; }
pcli() { LEE_WALLET_HOME_DIR=$R/wallet LEZ_BRIDGE_SEQUENCER=$SEQ_URL "$CLI" "$@"; }
xcli wallet check-health > "$XW/first-run.txt" 2>&1; chmod 600 "$XW/first-run.txt"
LAST_OUT=$RUN/x/new.out; xcli wallet account new public > "$LAST_OUT" 2>&1 || fail "new account X"
X=$(idof Public < "$LAST_OUT"); XPK=$(pkof < "$LAST_OUT")
[ -n "$X" ] && [ ${#XPK} = 64 ] || fail "could not create X"
newsecret "$RUN/x/deposit.secret"
XC=$("$CLI" commitment --secret-file "$RUN/x/deposit.secret" --dest "$X" --eth-token "$ETH" --denom "$D_ETH" --cfg-id "$CFG_ID" | sed -n 1p)
grep -qE '^0x[0-9a-f]{64}$' <<<"$XC" || fail "X commitment"

# Quote and send X's deposit through DepositDesk.depositETHWithFee.
DESK_QUOTE=$(cast call "$DESK" "quote(address,uint32)(uint256,uint256,uint256,uint256)" "$ETH" 0 --rpc-url "$ETH_RPC")
DESK_FEE=$(echo "$DESK_QUOTE" | sed -n 3p | cut -d' ' -f1)
DESK_TOTAL=$(echo "$DESK_QUOTE" | sed -n 4p | cut -d' ' -f1)
ZERO32=0x0000000000000000000000000000000000000000000000000000000000000000
LAST_OUT=$RUN/x/deposit.json
show "cast send $DESK 'depositETHWithFee(uint32,bytes32,bytes32)' 0 $XC $ZERO32 --value $DESK_TOTAL"
cast send "$DESK" "depositETHWithFee(uint32,bytes32,bytes32)" 0 "$XC" "$ZERO32" --value "$DESK_TOTAL" \
  --private-key "$ANVIL0" --rpc-url "$ETH_RPC" --json > "$LAST_OUT" 2>&1 || fail "X DepositDesk deposit"

# Balance assertions after deposits (6 direct for P + 1 via DepositDesk for X).
v_usdc_1=$(tbal "$USDC" "$VAULT"); v_eth_1=$(ebal "$VAULT"); d_eth_1=$(ebal "$DESK")
[ "$v_usdc_1" = 1212000000 ] || fail "vault USDC after deposits: $v_usdc_1, want 1212000000"
[ "$v_eth_1" = 131300000000000000 ] || fail "vault ETH after deposits: $v_eth_1, want 131300000000000000"
[ "$d_eth_1" = "$DESK_FEE" ] || fail "desk ETH after deposit: $d_eth_1, want $DESK_FEE"
note "balances after deposits: vault USDC=$v_usdc_1, vault ETH=$v_eth_1, desk ETH=$d_eth_1"

# 9b. Claims: P's 6 direct claims + X's relayed claim into zero-LEZ account.
roundtrip claims && rt_done claims || fail "round trip: claims"
p_bal_after_claim=$(pcli balances --token-program "$TP" 2>/dev/null)
p_usdc_c=$(awk -v p="Public/$P" -v d="$DEF_USDC" '$1 == p && $2 == d {print $3}' <<<"$p_bal_after_claim")
p_eth_c=$(awk -v p="Public/$P" -v d="$DEF_ETH" '$1 == p && $2 == d {print $3}' <<<"$p_bal_after_claim")
[ "${p_usdc_c:-0}" = 1200000000 ] || fail "P wrapped USDC after claims: ${p_usdc_c:-0}, want 1200000000"
[ "${p_eth_c:-0}" = 120000000000000000 ] || fail "P wrapped ETH after claims: ${p_eth_c:-0}, want 120000000000000000"
note "P wrapped balances after claims: wUSDC=$p_usdc_c, wETH=$p_eth_c"

XLEAF=$(cast call "$VAULT" "depositLeaf(address,uint256,bytes32)(bytes32)" "$ETH" "$D_ETH" "$XC" --rpc-url "$ETH_RPC")
XIDX=$(( $(grep -nix "$XLEAF" "$R/leaves.txt" | cut -d: -f1) - 1 ))
[ "$XIDX" -ge 0 ] || fail "X leaf not under attested root"

RW=$R/wallet-relay; mkdir -p "$RW" "$RUN/relay"; chmod 700 "$RW"
rcli() { LEE_WALLET_HOME_DIR=$RW LEZ_BRIDGE_SEQUENCER=$SEQ_URL "$CLI" "$@"; }
rcli wallet check-health > "$RW/first-run.txt" 2>&1; chmod 600 "$RW/first-run.txt"
LAST_OUT=$RUN/relay/new.out; rcli wallet account new public > "$LAST_OUT" 2>&1 || fail "relay payer account"
RP=$(idof Public < "$LAST_OUT"); RPK=$(pkof < "$LAST_OUT")
pcli wallet auth-transfer send --from "Public/$F" --to-pk "$RPK" --amount 100000000 > "$RUN/relay/fund.out" 2>&1 || fail "fund relay payer"
P=$RUN/relay CLI=$CLI RELAY_WALLET=$RW SEQUENCER=$SEQ_URL BRIDGE=$BP TOKEN_PROGRAM=$TP PAYER=$RP \
  RELAY_STATE=$RUN/relay/state.json RELAY_LOG=$RUN/relay/relayer.log python3 -I "$REPO/public-demo/relayer/relayer.py" --bind-payer \
  > "$RUN/relay/bind.out" 2>&1 || fail "bind relay payer"
P=$RUN/relay CLI=$CLI RELAY_WALLET=$RW SEQUENCER=$SEQ_URL RELAY_LISTEN=127.0.0.1:$RELAY_PORT BRIDGE=$BP \
  TOKEN_PROGRAM=$TP PAYER=$RP RELAY_STATE=$RUN/relay/state.json RELAY_LOG=$RUN/relay/relayer.log \
  nohup python3 -I "$REPO/public-demo/relayer/relayer.py" > "$RUN/relay/relayer.out" 2>&1 < /dev/null &
RELAY_PID=$!
RELAY=http://127.0.0.1:$RELAY_PORT/relay
for _ in $(seq 1 20); do curl -s -m 2 "$RELAY/info" >/dev/null 2>&1 && break; sleep 1; done
curl -fsS -m 5 "$RELAY/info" >/dev/null || fail "relayer did not answer"

XCLAIM=(claim --bridge "$BP" --token-program "$TP" --from "$X" --eth-token "$ETH" --denom-index 0 --denom "$D_ETH"
        --secret-file "$RUN/x/deposit.secret" --cfg-id "$CFG_ID" --leaves "$R/leaves.txt" --index "$XIDX")
p0=$(native "$RP")
LAST_OUT=$RUN/x/claim.out
xcli "${XCLAIM[@]}" --relay "$RELAY" > "$LAST_OUT" 2>&1 || fail "X relayed claim"
sleep 2
p1=$(native "$RP")
[ "$p1" -lt "$p0" ] || fail "relay payer balance did not decrease ($p0 -> $p1)"
xn=$(native "$X")
[ "${xn:-0}" = 0 ] || fail "X native LEZ after relayed claim: $xn, want 0"
x_weth=$(xcli balances --token-program "$TP" 2>/dev/null | awk -v x="Public/$X" -v d="$DEF_ETH" '$1 == x && $2 == d {print $3}')
[ "${x_weth:-0}" = "$D_ETH" ] || fail "X wrapped ETH after claim: ${x_weth:-0}, want $D_ETH"
note "X after relayed claim: native=0, wETH=$x_weth (relay paid $((p0 - p1)))"

# 9c. Burns: X burns 0.01 wETH, P burns batch 1.
XR=$(cast keccak "e2e-ci-x-recipient-$STAMP" | cut -c1-42)
newsecret "$RUN/x/exit.secret"
XE=$("$CLI" exit-commitment --secret-file "$RUN/x/exit.secret" --eth-dest "$XR" --eth-token "$ETH" --denom "$D_ETH" \
  --max-relay-fee "$XMAX_ETH" --cfg-id "$CFG_ID" | sed -n 1p)
grep -qE '^0x[0-9a-f]{64}$' <<<"$XE" || fail "X exit commitment"
pcli wallet auth-transfer send --from "Public/$F" --to-pk "$XPK" --amount 50000000 > "$RUN/x/topup.out" 2>&1 || fail "top up X"
LAST_OUT=$RUN/x/burn.out
xcli burn --bridge "$BP" --token-program "$TP" --from "$X" --definition "$DEF_ETH" --amount "$D_ETH" --exit "$XE" > "$LAST_OUT" 2>&1 || fail "X burn"
x_weth_after=$(xcli balances --token-program "$TP" 2>/dev/null | awk -v x="Public/$X" -v d="$DEF_ETH" '$1 == x && $2 == d {print $3}')
[ "${x_weth_after:-0}" = 0 ] || fail "X wrapped ETH after burn: ${x_weth_after:-0}, want 0"
note "X burned $D_ETH wETH -> balance now 0"

roundtrip burns1 && rt_done burns1 || fail "round trip: burns1"
echo "x $XE" >> "$R/exit-commitments.txt"

# 9d. G2-lite updates and G3 releases.
roundtrip submit1 && rt_done submit1 || fail "round trip: submit1"
grep -qi "$(echo "${XE#0x}" | lower)" "$R/update1/report.json" || fail "X exit not in G2 update 1"
roundtrip releases2 && rt_done releases2 || fail "round trip: releases2"

XIDX_E=$(python3 -I - "$XE" "$R/exit-leaves.txt" "$CFG_ID" "$ETH" "$D_ETH" <<'PY'
import hashlib, sys
E, lf, cfg, tok, d = sys.argv[1:]
tag = bytes.fromhex("22018742ad6028fda6cb0726afaae90ea20dff3cef6b1ec5b3c14f856d76c59c")
leaf = "0x" + hashlib.sha256(tag + bytes.fromhex(cfg[2:]) + bytes.fromhex(tok[2:]) + int(d).to_bytes(32, "big") + bytes.fromhex(E[2:])).hexdigest()
print([l.strip() for l in open(lf) if l.strip()].index(leaf))
PY
) || fail "X exit leaf not in exit-leaves.txt"
LAST_OUT=$RUN/x/release.txt
# shellcheck disable=SC2086
$RUN_WRAP "$HOST" release --secret "$(cat "$RUN/x/exit.secret")" --eth-dest "$XR" --token "$ETH" --denom "$D_ETH" \
  --max-relay-fee "$XMAX_ETH" --cfg-id "$CFG_ID" --leaves "$R/exit-leaves.txt" --index "$XIDX_E" --relayer "$ETH" --relay-fee 0 \
  > "$LAST_OUT" 2>&1 || fail "X G3 release (executor)"
XROOT=$(sed -n 's/^ROOT=//p' "$LAST_OUT"); XNF=$(sed -n 's/^NULLIFIER=//p' "$LAST_OUT"); XSEAL=$(sed -n 's/^SEAL=//p' "$LAST_OUT")
at=$(cast call "$VAULT" "releasableAt(address,bytes32)(uint256)" "$ETH" "$XROOT" --rpc-url "$ETH_RPC" | cut -d' ' -f1)
while [ "$(date +%s)" -le "$((at + 1))" ]; do sleep 2; done
cast send "$ANVIL0_ADDR" --value 0 --private-key "$ANVIL0" --rpc-url "$ETH_RPC" >/dev/null
b0=$(ebal "$XR")
LAST_OUT=$RUN/x/release-tx.json
cast send "$VAULT" "release(bytes,bytes32,bytes32,address,uint32,address,address,uint256)" \
  "$XSEAL" "$XROOT" "$XNF" "$ETH" 0 "$XR" "$ETH" 0 --private-key "$ANVIL0" --rpc-url "$ETH_RPC" --json > "$LAST_OUT" 2>&1 \
  || fail "X release tx"
XTX=$(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1]))["transactionHash"])' "$LAST_OUT")
b1=$(ebal "$XR")
[ "$((b1 - b0))" = "$((D_ETH - WF_ETH))" ] || fail "X recipient got $((b1 - b0)), want $((D_ETH - WF_ETH))"
[ "$(cast call "$VAULT" 'releaseNullifierSpent(bytes32)(bool)' "$XNF" --rpc-url "$ETH_RPC")" = true ] || fail "X release nullifier not spent"
echo "x $XTX" >> "$R/released.txt"
note "X released on anvil: recipient $XR +$((b1 - b0)) wei, nullifier spent"

# 9e. Closing solvency + privacy reconciliation across every deposit and release.
roundtrip "" || fail "round trip: closing checks"
kill "$RELAY_PID" 2>/dev/null || true; RELAY_PID=""
pass

# ===================================================================================== 10
step "Local security e2e cases: double-claim, wrong-chain, frozen token, C1/H1/M2" "$L_SEC"
mkdir -p "$RUN/sec"
LEZ_CHAIN_ID=$(pcli chain-id | sed -n 's/^LEZ_CHAIN_ID=//p')
ZONE_ID=$("$CLI" source-id | sed -n 's/^ZONE_ID=//p')
CONFIG_PDA=$("$CLI" program-id --bridge "$BP" | sed -n 's/^CONFIG=//p')

# 10a. Double-claim refused (CLI pre-check AND on-chain revert, supply unchanged). X signs and pays
# the repeat itself, and an included revert costs its whole gas limit: top X up first.
pcli wallet auth-transfer send --from "Public/$F" --to-pk "$XPK" --amount 50000000 > "$RUN/sec/topup-x.txt" 2>&1 || fail "top up X for the repeat claim"
s_eth_0=$(supply_of pcli definition --token-program "$TP" --definition "$DEF_ETH")
assert_cli_refuses "$RUN/sec/double-claim-cli.txt" "already claimed" xcli "${XCLAIM[@]}"
assert_reverts_onchain "$RUN/sec/double-claim-chain.txt" xcli --no-precheck --allow-revert "${XCLAIM[@]}"
s_eth_1=$(supply_of pcli definition --token-program "$TP" --definition "$DEF_ETH")
[ "$s_eth_0" = "$s_eth_1" ] || fail "double claim changed wrapped ETH supply ($s_eth_0 -> $s_eth_1)"

# 10b. Wrong-chain refused (wrong LEZ channel id on init, AND wrong EVM chain id / cfg_id on claim)
BAD_LEZ_CHAIN=0x1111111111111111111111111111111111111111111111111111111111111111
assert_cli_refuses "$RUN/sec/wrong-lez-chain.txt" "is not this chain's channel id" \
  pcli init --bridge "$BP" --token-program "$TP" --admin "$P" --vault "$VAULT" --chain-id "$EVM_CHAIN_ID" \
  --lez-chain-id "$BAD_LEZ_CHAIN" --freezer "$P" --attestor "$P" --payer "$P"

WRONG_CFG=$("$CLI" cfg-id --vault "$VAULT" --chain-id 1 --lez-chain-id "$LEZ_CHAIN_ID" --zone-id "$ZONE_ID" --bridge "$BP" \
  | sed -n 's/^CFG_ID=//p')
[ -n "$WRONG_CFG" ] && [ "$(echo "$WRONG_CFG" | lower)" != "$(echo "$CFG_ID" | lower)" ] || fail "bad WRONG_CFG"
newsecret "$RUN/sec/wrong-chain.secret"
WC=$("$CLI" commitment --secret-file "$RUN/sec/wrong-chain.secret" --dest "$P" --eth-token "$USDC" --denom "$D_USDC" --cfg-id "$WRONG_CFG" | sed -n 1p)
python3 -I - "$WRONG_CFG" "$USDC" "$D_USDC" "$WC" > "$RUN/sec/wrong-chain-leaves.txt" 2> "$RUN/sec/wrong-chain-root.txt" <<'PY'
import hashlib, sys
h = lambda *p: hashlib.sha256(b"".join(p)).digest()
tag = bytes.fromhex("9e8d098bfe4f2807258ba6b200534afcacae50702d4735cb40a3f31e9bea950c")
cfg, tok, d, c = bytes.fromhex(sys.argv[1][2:]), bytes.fromhex(sys.argv[2][2:]), int(sys.argv[3]).to_bytes(32, "big"), bytes.fromhex(sys.argv[4][2:])
leaf = h(tag, cfg, tok, d, c)
print("0x" + leaf.hex())
zero = b"\0" * 32; node = leaf
for _ in range(32):
    node = h(node, zero); zero = h(zero, zero)
print("0x" + node.hex(), file=sys.stderr)
PY
pcli attest --bridge "$BP" --attestor "$P" --root "$(cat "$RUN/sec/wrong-chain-root.txt")" > "$RUN/sec/wrong-chain-attest.txt" 2>&1 \
  || fail "attest wrong-chain synthetic root"
s_usdc_0=$(supply_of pcli definition --token-program "$TP" --definition "$DEF_USDC")
WCLAIM=(claim --bridge "$BP" --token-program "$TP" --from "$P" --eth-token "$USDC" --denom-index 0 --denom "$D_USDC"
        --secret-file "$RUN/sec/wrong-chain.secret" --cfg-id "$WRONG_CFG" --leaves "$RUN/sec/wrong-chain-leaves.txt" --index 0)
assert_cli_refuses "$RUN/sec/wrong-chain-cli.txt" "claim for another deployment" pcli "${WCLAIM[@]}"
assert_reverts_onchain "$RUN/sec/wrong-chain-chain.txt" pcli --no-precheck --allow-revert "${WCLAIM[@]}"
s_usdc_1=$(supply_of pcli definition --token-program "$TP" --definition "$DEF_USDC")
[ "$s_usdc_0" = "$s_usdc_1" ] || fail "wrong-chain claim changed wrapped USDC supply ($s_usdc_0 -> $s_usdc_1)"

# 10c. Frozen token refused (LEZ per-token mint freeze, unfreeze + recovery, LEZ burn freeze, vault deposit freeze)
newsecret "$RUN/sec/freeze.secret"
FC=$("$CLI" commitment --secret-file "$RUN/sec/freeze.secret" --dest "$P" --eth-token "$USDC" --denom "$D_USDC" --cfg-id "$CFG_ID" | sed -n 1p)
LAST_OUT=$RUN/sec/freeze-dep.json
cast send "$VAULT" "deposit(address,uint32,bytes32)" "$USDC" 0 "$FC" --private-key "$ANVIL3" --rpc-url "$ETH_RPC" --json > "$LAST_OUT" 2>&1 \
  || fail "deposit for freeze test"
FLEAF=$(cast call "$VAULT" "depositLeaf(address,uint256,bytes32)(bytes32)" "$USDC" "$D_USDC" "$FC" --rpc-url "$ETH_RPC")
cp "$R/leaves.txt" "$RUN/sec/freeze-leaves.txt"
echo "$FLEAF" >> "$RUN/sec/freeze-leaves.txt"
FIDX=$(( $(wc -l < "$RUN/sec/freeze-leaves.txt" | tr -d ' ') - 1 ))
FROOT=$(cast call "$VAULT" "depositRoot()(bytes32)" --rpc-url "$ETH_RPC")
pcli attest --bridge "$BP" --attestor "$P" --root "$FROOT" > "$RUN/sec/freeze-attest.txt" 2>&1 || fail "attest freeze root"

LAST_OUT=$RUN/sec/freeze-mint.txt
pcli freeze --bridge "$BP" --by "$P" --eth-token "$USDC" --mint > "$LAST_OUT" 2>&1 || fail "freeze USDC mint"
FCLAIM=(claim --bridge "$BP" --token-program "$TP" --from "$P" --eth-token "$USDC" --denom-index 0 --denom "$D_USDC"
        --secret-file "$RUN/sec/freeze.secret" --cfg-id "$CFG_ID" --leaves "$RUN/sec/freeze-leaves.txt" --index "$FIDX")
assert_cli_refuses "$RUN/sec/frozen-claim-cli.txt" "minting is frozen" pcli "${FCLAIM[@]}"
assert_reverts_onchain "$RUN/sec/frozen-claim-chain.txt" pcli --no-precheck --allow-revert "${FCLAIM[@]}"
s_usdc_2=$(supply_of pcli definition --token-program "$TP" --definition "$DEF_USDC")
[ "$s_usdc_1" = "$s_usdc_2" ] || fail "frozen claim changed wrapped USDC supply"

# Unfreeze minting -> same deposit must now claim cleanly (frozen refusal did not burn the nullifier).
LAST_OUT=$RUN/sec/unfreeze-mint.txt
pcli unfreeze --bridge "$BP" --admin "$P" --eth-token "$USDC" --mint > "$LAST_OUT" 2>&1 || fail "unfreeze USDC mint"
LAST_OUT=$RUN/sec/unfrozen-claim.txt
pcli "${FCLAIM[@]}" > "$LAST_OUT" 2>&1 || fail "claim after unfreeze failed"
s_usdc_3=$(supply_of pcli definition --token-program "$TP" --definition "$DEF_USDC")
[ "$s_usdc_3" = "$((s_usdc_2 + D_USDC))" ] || fail "supply after unfrozen claim: $s_usdc_3, want $((s_usdc_2 + D_USDC))"
note "LEZ mint freeze refused claim; unfreeze allowed the same deposit to claim (supply $s_usdc_2 -> $s_usdc_3)"

# Per-token burn freeze on LEZ: P now holds D_USDC wrapped USDC.
LAST_OUT=$RUN/sec/freeze-burn.txt
pcli freeze --bridge "$BP" --by "$P" --eth-token "$USDC" --burn > "$LAST_OUT" 2>&1 || fail "freeze USDC burn"
FE=$(cast keccak "e2e-ci-frozen-burn-$STAMP")
assert_reverts_onchain "$RUN/sec/frozen-burn-chain.txt" \
  pcli --allow-revert burn --bridge "$BP" --token-program "$TP" --from "$P" --definition "$DEF_USDC" --amount "$D_USDC" --exit "$FE"
s_usdc_4=$(supply_of pcli definition --token-program "$TP" --definition "$DEF_USDC")
[ "$s_usdc_4" = "$s_usdc_3" ] || fail "frozen burn changed wrapped USDC supply"
pcli unfreeze --bridge "$BP" --admin "$P" --eth-token "$USDC" --burn > "$RUN/sec/unfreeze-burn.txt" 2>&1 || fail "unfreeze USDC burn"
note "LEZ per-token burn freeze refused burn (supply unchanged at $s_usdc_4)"

# Ethereum vault per-token deposit freeze: must revert with IsFrozen. Lifting it goes through the
# admin timelock (M4): schedule(Unfreeze), execute refused before adminDelay, then accepted after
# it. The last Ethereum step of the run, so moving anvil's clock forward here delays nothing.
LAST_OUT=$RUN/sec/vault-freeze.txt
cast send "$VAULT" "freeze(address,bool,bool)" "$USDC" true false --private-key "$ANVIL3" --rpc-url "$ETH_RPC" > "$LAST_OUT" 2>&1 || fail "vault freeze USDC"
assert_eth_reverts IsFrozen "$ANVIL3" "$VAULT" "deposit(address,uint32,bytes32)" "$USDC" 0 "$(cast keccak "e2e-ci-vault-frozen-$STAMP")"
UNFREEZE_PARAMS=$(cast abi-encode "f(address,bool,bool)" "$USDC" true false)
ADMIN_DELAY=$(cast call "$VAULT" "adminDelay()(uint64)" --rpc-url "$ETH_RPC" | cut -d' ' -f1)
[ "${ADMIN_DELAY:-0}" -ge 3600 ] || fail "vault adminDelay ${ADMIN_DELAY:-?} below MIN_ADMIN_DELAY (3600)"
LAST_OUT=$RUN/sec/vault-schedule-unfreeze.txt
cast send "$VAULT" "schedule(uint8,bytes)" 0 "$UNFREEZE_PARAMS" --private-key "$ANVIL3" --rpc-url "$ETH_RPC" > "$LAST_OUT" 2>&1 || fail "schedule vault unfreeze"
assert_eth_reverts ActionTooEarly "$ANVIL3" "$VAULT" "execute(uint8,bytes)" 0 "$UNFREEZE_PARAMS"
assert_eth_reverts IsFrozen "$ANVIL3" "$VAULT" "deposit(address,uint32,bytes32)" "$USDC" 0 "$(cast keccak "e2e-ci-vault-frozen2-$STAMP")"
cast rpc evm_increaseTime "$((ADMIN_DELAY + 1))" --rpc-url "$ETH_RPC" >/dev/null || fail "anvil evm_increaseTime"
cast rpc evm_mine --rpc-url "$ETH_RPC" >/dev/null || fail "anvil evm_mine"
LAST_OUT=$RUN/sec/vault-execute-unfreeze.txt
cast send "$VAULT" "execute(uint8,bytes)" 0 "$UNFREEZE_PARAMS" --private-key "$ANVIL3" --rpc-url "$ETH_RPC" > "$LAST_OUT" 2>&1 || fail "execute vault unfreeze after adminDelay"
dc0=$(cast call "$VAULT" "depositCount()(uint64)" --rpc-url "$ETH_RPC" | cut -d' ' -f1)
LAST_OUT=$RUN/sec/vault-deposit-after-unfreeze.txt
cast send "$VAULT" "deposit(address,uint32,bytes32)" "$USDC" 0 "$(cast keccak "e2e-ci-vault-unfrozen-$STAMP")" --private-key "$ANVIL3" --rpc-url "$ETH_RPC" > "$LAST_OUT" 2>&1 \
  || fail "deposit after the timelocked unfreeze"
dc1=$(cast call "$VAULT" "depositCount()(uint64)" --rpc-url "$ETH_RPC" | cut -d' ' -f1)
[ "$dc1" = "$((dc0 + 1))" ] || fail "depositCount after unfreeze: $dc1, want $((dc0 + 1))"
note "vault deposit freeze: IsFrozen; unfreeze only through the timelock (ActionTooEarly before adminDelay $ADMIN_DELAY s, accepted after)"

# 10d. Fixed security regressions (from e2e-security.sh: C1 header mint, H1 re-init, M2 sponsor cap)
pcli wallet auth-transfer send --from "Public/$F" --to "Public/$BP" --amount 50000000 > "$RUN/sec/fund-bp.txt" 2>&1 || fail "fund bridge header"
assert_reverts_onchain "$RUN/sec/c1-header-mint.txt" \
  pcli --allow-revert raw-call --program "$TP" --signer "$BP" \
  --message-hex "$(enc mint "$DEF_USDC" "$BP" 1000000000000)" --also "$DEF_USDC@$TP" --payer "$P"
s_usdc_5=$(supply_of pcli definition --token-program "$TP" --definition "$DEF_USDC")
[ "$s_usdc_5" = "$s_usdc_4" ] || fail "C1 header mint changed supply"
assert_reverts_onchain "$RUN/sec/h1-reinit.txt" \
  pcli --allow-revert raw-call --program "$BP" --signer "$BP" \
  --message-hex "$(enc initialize "$P" "$VAULT" "$EVM_CHAIN_ID" "$LEZ_CHAIN_ID" "$TP" "$P" "$P")" --also "$CONFIG_PDA@$BP" --payer "$P"
assert_reverts_onchain "$RUN/sec/m2-sponsor-cap.txt" \
  pcli --allow-revert set-sponsor --bridge "$BP" --admin "$P" --amount 50000001
note "C1 header mint, H1 re-init, and M2 over-cap sponsor all reverted on chain"
pass

finish
