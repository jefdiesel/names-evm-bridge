#!/bin/bash
# NOTE (2026-11-14): written for the CLI and programs BEFORE the security fixes (`init --zone-id`, init
# signed by --admin, first-come). The security-fixes CLI's `init` is signed by the bridge's header key
# with --payer and takes no --zone-id (see lez-bridge/scripts/e2e-security.sh); update this script
# before using it with that CLI. Kept as is for the live v2 deployment and its logs.
# End to end on throwaway local chains: Ethereum (anvil) and LEZ 0.4 (standalone sequencer).
#   1. start anvil; set up the LEZ wallet and a funded account;
#   2. deploy the token and bridge programs on LEZ (the vault needs the bridge's program id);
#   3. deploy the vault on anvil with LEZ_CHAIN_ID, ZONE_ID and PROGRAM_ID; it prints its CFG_ID,
#      which must equal what `lez-bridge cfg-id` derives, or the script stops;
#   4. create wrapped USDC (minted only by the bridge), init (the bridge derives the same cfg_id)
#      and register; set the sponsor amount and top up the bridge's float (fee sponsoring);
#   5. deposit 100 and 1000 USDC on Ethereum with commitments for our LEZ account;
#   6. record the vault's current deposit root (stand-in attestor), claim both deposits against
#      it, try a double claim;
#   7. burn one denomination with an exit commitment; try a non-denomination burn.
# PRIVATE=1 claims into and burns from a private account V (proved on this machine, 4 threads).
# Each private claim mints to a one-time address of V and the float pays that address SPONSOR
# native LEZ in the same transaction, which pays V's fees: F never funds V. With SPONSOR=0 the
# float is off and V is funded instead by `shield`, which credits a one-time address of V.
# Public claims (the default) mint to F by name and never draw the float; the script checks that.
#
#   R=<run dir> SEQ=http://127.0.0.1:3250 scripts/e2e-local.sh
# A run dir from before the cfg_id change (2026-11-12) can't be resumed: use a fresh R.
set -euo pipefail
: "${R:?set R to a run directory}"
HERE=$(cd "$(dirname "$0")/.." && pwd)
VAULT_DIR=${VAULT_DIR:-$HERE/../vault}
export LEE_WALLET_HOME_DIR=$R/wallet
export LEZ_BRIDGE_SEQUENCER=${SEQ:-http://127.0.0.1:3250}
export RAYON_NUM_THREADS=${RAYON_NUM_THREADS:-4}
# In-process prover only: never fall back to the external r0vm (see cli/Cargo.toml).
export RISC0_PROVER=local
export PATH=$HOME/.foundry/bin:$HOME/.risc0/bin:$PATH
T="$HERE/scripts/guard.sh nice -n 10 $HERE/target/release/lez-bridge"
export GUARD_LOG=$R/guard.log
LOCAL_TEST_KEY=$("$HERE/scripts/lez-test-key.sh") || exit 1   # LEZ published genesis test key, read from LEZ
ANVIL_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
ETH_RPC=http://127.0.0.1:${ANVIL_PORT:-8550}
ZERO32=0x0000000000000000000000000000000000000000000000000000000000000000
# LEZ 0.4 has no native chain id; locally both ids are 32 zero bytes (shared with the vault).
LEZ_CHAIN_ID=${LEZ_CHAIN_ID:-$ZERO32}
ZONE_ID=${ZONE_ID:-$ZERO32}
USDC_LEZ_DENOMS=100000000,1000000000
# Native LEZ the float pays each one-time claim recipient (a claim's fee was ~9.1M in run
# private4; the rest covers a later burn), and what the script tops the float up with.
SPONSOR=${SPONSOR:-20000000}
FLOAT_TOPUP=${FLOAT_TOPUP:-100000000}
say() { echo; echo "## $(date +%T) $*"; }
die() { echo "FAILED: $*" >&2; exit 1; }
lower() { tr 'A-F' 'a-f'; }
id_of() { grep -oE "$1/[1-9A-HJ-NP-Za-km-z]{32,44}" | sed -n 1p | cut -d/ -f2; }
mkdir -p "$R"; touch "$R/ids.env"; . "$R/ids.env"
save() { for v in F V TP BP DEF VAULT USDC CFG_ID; do echo "$v=${!v:-}"; done > "$R/ids.env"; }
if [ -n "${DEF:-}" ] && ! grep -q '^CFG_ID=' "$R/ids.env"; then
  die "$R predates the derived cfg_id (old config layout): use a fresh R"
fi

say "Ethereum: anvil on $ETH_RPC"
if ! cast chain-id --rpc-url $ETH_RPC >/dev/null 2>&1; then
  nohup anvil --port ${ANVIL_PORT:-8550} --silent > "$R/anvil.log" 2>&1 & echo $! > "$R/anvil.pid"
  sleep 2; VAULT=; USDC=; CFG_ID=
fi
EVM_CHAIN_ID=$(cast chain-id --rpc-url $ETH_RPC)

say "LEZ: wallet and funded account"
if [ -z "${F:-}" ]; then
  $T wallet check-health
  F=$($T wallet account import public --private-key $LOCAL_TEST_KEY | tee /dev/stderr | id_of Public)
  save
fi
if [ "${PRIVATE:-0}" = 1 ] && [ -z "${V:-}" ]; then
  V=$($T wallet account new private | tee /dev/stderr | id_of Private)
  save
  if [ "$SPONSOR" = 0 ]; then
    say "no sponsoring: shield native to a one-time address of V for its in-proof fees"
    $T shield --from $F --to $V --amount 50000000
  fi
fi
DEST=$F; [ "${PRIVATE:-0}" = 1 ] && DEST=$V
echo "F=$F V=${V:-} claiming into $DEST"

if [ -z "${TP:-}" ]; then
  say "deploy the token program"
  TP=$($T deploy --which token --payer $F | tee /dev/stderr | sed -n 's/^program //p'); save
fi
if [ -z "${BP:-}" ]; then
  say "deploy the bridge program"
  BP=$($T deploy --which bridge --payer $F | tee /dev/stderr | sed -n 's/^program //p')
  # A new bridge program means a new cfg_id: a new vault, wrapped token and init.
  VAULT=; USDC=; CFG_ID=; DEF=; save
fi
PROGRAM_ID=$($T program-id --bridge $BP | sed -n 's/^PROGRAM_ID=//p')
[ -n "$PROGRAM_ID" ] || die "no program id for $BP"

if [ -z "${VAULT:-}" ]; then
  say "Ethereum: deploy the vault for bridge $BP ($PROGRAM_ID)"
  out=$(cd "$VAULT_DIR" && LEZ_CHAIN_ID=$LEZ_CHAIN_ID ZONE_ID=$ZONE_ID PROGRAM_ID=$PROGRAM_ID \
    forge script script/LocalDeploy.s.sol --rpc-url $ETH_RPC --private-key $ANVIL_KEY --broadcast 2>&1) \
    || { echo "$out"; die "vault deploy"; }
  VAULT=$(echo "$out" | sed -n 's/.*VAULT=\(0x[0-9a-fA-F]*\).*/\1/p' | sed -n 1p)
  USDC=$(echo "$out" | sed -n 's/.*USDC=\(0x[0-9a-fA-F]*\).*/\1/p' | sed -n 1p)
  CFG_ID=$(echo "$out" | sed -n 's/.*CFG_ID=\(0x[0-9a-fA-F]*\).*/\1/p' | sed -n 1p)
  [ -n "$VAULT" ] && [ -n "$USDC" ] && [ -n "$CFG_ID" ] || { echo "$out"; die "the vault deploy did not print VAULT, USDC and CFG_ID"; }
  save
fi
# The vault's CFG_ID and the one the bridge derives must be the same, byte for byte.
DERIVED=$($T cfg-id --vault $VAULT --chain-id $EVM_CHAIN_ID --lez-chain-id $LEZ_CHAIN_ID --zone-id $ZONE_ID --bridge $BP \
  | sed -n 's/^CFG_ID=//p')
if [ "$(echo "$DERIVED" | lower)" != "$(echo "$CFG_ID" | lower)" ]; then
  die "cfg_id mismatch: vault $CFG_ID, lez-bridge cfg-id $DERIVED (vault $VAULT chain $EVM_CHAIN_ID lez $LEZ_CHAIN_ID zone $ZONE_ID program $PROGRAM_ID)"
fi
echo "vault $VAULT usdc $USDC cfg $CFG_ID (matches the bridge's derivation)"

if [ -z "${DEF:-}" ]; then
  say "create wrapped USDC, mint authority = the bridge program"
  DEF=$($T create-wrapped --token-program $TP --bridge $BP --name "Wrapped USDC" --payer $F | tee /dev/stderr | sed -n 's/^token //p'); save
  say "init the bridge and register USDC"
  $T init --bridge $BP --token-program $TP --admin $F --vault $VAULT --chain-id $EVM_CHAIN_ID \
    --lez-chain-id $LEZ_CHAIN_ID --zone-id $ZONE_ID --freezer $F --attestor $F
  $T register --bridge $BP --admin $F --eth-token $USDC --definition $DEF --decimals 6 --denoms $USDC_LEZ_DENOMS
  say "sponsor $SPONSOR per one-time claim; top up the float with $FLOAT_TOPUP"
  $T set-sponsor --bridge $BP --admin $F --amount $SPONSOR
  $T fund-float --bridge $BP --from $F --amount $FLOAT_TOPUP
fi
ONCHAIN=$($T show --bridge $BP | sed -n 's/^cfg_id //p')
if [ "$(echo "$ONCHAIN" | lower)" != "$(echo "$CFG_ID" | lower)" ]; then
  die "the bridge config holds cfg_id $ONCHAIN but the vault has $CFG_ID (stale run dir? use a fresh R)"
fi
$T definition --token-program $TP --definition $DEF

say "Ethereum: deposit 100 and 1000 USDC with commitments for $DEST"
K1=$(cast keccak "secret-1-$DEST"); K2=$(cast keccak "secret-2-$DEST")
C1=$($T commitment --secret $K1 --dest $DEST --eth-token $USDC --denom 100000000 --cfg-id $CFG_ID)
C2=$($T commitment --secret $K2 --dest $DEST --eth-token $USDC --denom 1000000000 --cfg-id $CFG_ID)
start=$(cast call $VAULT "depositCount()(uint64)" --rpc-url $ETH_RPC)
cast send $VAULT "deposit(address,uint32,bytes32)" $USDC 0 $C1 --private-key $ANVIL_KEY --rpc-url $ETH_RPC >/dev/null
cast send $VAULT "deposit(address,uint32,bytes32)" $USDC 1 $C2 --private-key $ANVIL_KEY --rpc-url $ETH_RPC >/dev/null
ROOT=$(cast call $VAULT "depositRoot()(bytes32)" --rpc-url $ETH_RPC)
echo "vault deposit root $ROOT (deposits $start..$((start+1)))"
: > "$R/leaves.txt"
n=$(cast call $VAULT "depositCount()(uint64)" --rpc-url $ETH_RPC)
# Rebuild the leaves from the vault's own Deposit events.
cast logs --from-block 0 --address $VAULT "Deposit(address indexed,uint32,bytes32,uint64,bytes32)" --rpc-url $ETH_RPC --json \
  | python3 -c '
import json,sys
logs=json.load(sys.stdin)
for l in logs:
    d=l["data"][2:]
    print(l["topics"][1][-40:], int(d[0:64],16), "0x"+d[64:128])' > "$R/deposits.txt"
denom_of() { [ "$1" = 0 ] && echo 100000000 || echo 1000000000; }
while read -r tok di c; do
  cast call $VAULT "depositLeaf(address,uint256,bytes32)(bytes32)" 0x$tok $(denom_of $di) $c --rpc-url $ETH_RPC >> "$R/leaves.txt"
done < "$R/deposits.txt"
echo "$(wc -l < "$R/leaves.txt") leaves rebuilt from events (vault count $n)"

say "LEZ: record the vault root (stand-in attestor)"
# Clients must always claim against the newest attested root: an older root holds fewer
# deposits, so it narrows the set a claim could have come from. ROOT is the vault's current
# depositRoot(), and the claims below rebuild their path from every leaf the vault holds, so
# they prove against this same root.
$T attest --bridge $BP --attestor $F --root $ROOT

float_now() { $T show --bridge $BP | sed -n 's/^float //p'; }
FLOAT_BEFORE=$(float_now)
echo "float before the claims: $FLOAT_BEFORE"
say "LEZ: claim the 100 deposit into $DEST"
$T claim --bridge $BP --token-program $TP --from $DEST --eth-token $USDC --denom-index 0 --denom 100000000 \
  --secret $K1 --cfg-id $CFG_ID --leaves "$R/leaves.txt" --index $start
say "LEZ: claim the 1000 deposit"
$T claim --bridge $BP --token-program $TP --from $DEST --eth-token $USDC --denom-index 1 --denom 1000000000 \
  --secret $K2 --cfg-id $CFG_ID --leaves "$R/leaves.txt" --index $((start+1))
$T balances --token-program $TP
$T show --bridge $BP
FLOAT_AFTER=$(float_now)
EXPECT=$FLOAT_BEFORE
if [ "${PRIVATE:-0}" = 1 ] && [ "$($T show --bridge $BP | sed -n 's/^sponsor_amount //p')" != 0 ]; then
  EXPECT=$((FLOAT_BEFORE - 2 * $($T show --bridge $BP | sed -n 's/^sponsor_amount //p')))
fi
[ "$FLOAT_AFTER" = "$EXPECT" ] || die "float $FLOAT_AFTER after the claims, expected $EXPECT"
echo "float after the claims: $FLOAT_AFTER (as expected)"

say "LEZ: the same deposit again is refused"
if $T claim --bridge $BP --token-program $TP --from $DEST --eth-token $USDC --denom-index 0 --denom 100000000 \
  --secret $K1 --cfg-id $CFG_ID --leaves "$R/leaves.txt" --index $start; then echo "UNEXPECTED: double claim applied"; exit 1; fi
[ "$(float_now)" = "$FLOAT_AFTER" ] || die "the refused claim moved the float"

say "LEZ: burn 100 for exit to an Ethereum address"
ETH_DEST=0x000000000000000000000000000000000000bEEF
KX=$(cast keccak "exit-1-$DEST")
E=$($T exit-commitment --secret $KX --eth-dest $ETH_DEST --eth-token $USDC --denom 100000000 --max-relay-fee 2000000 --cfg-id $CFG_ID | sed -n 1p)
$T burn --bridge $BP --token-program $TP --from $DEST --definition $DEF --amount 100000000 --exit $E
say "LEZ: a burn of 50 (not a denomination) is refused"
if $T burn --bridge $BP --token-program $TP --from $DEST --definition $DEF --amount 50000000 --exit $E; then echo "UNEXPECTED: odd burn applied"; exit 1; fi
$T balances --token-program $TP
$T show --bridge $BP
$T definition --token-program $TP --definition $DEF
say "done: exit commitment $E"
