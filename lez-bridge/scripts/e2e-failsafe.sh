#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# The bridge's FAILSAFE (RFP-022 Security #9) end to end on throwaway local chains, judge-demo style:
#   three seed-derived deposits; one claimed on LEZ; the vault wound down; LEZ minting stopped for
#   good by the bridge's wind-down; the final claim accumulator fixed by the G2 update that holds it; the two unclaimed deposits reclaimed on
#   Ethereum to fresh addresses by a relayer that is not the depositor (one of the two depositors
#   first recovered from the seed phrase alone, wallet directory wiped); the claimed one refused.
#
#   mock-verified: proofs executed, not proved; LEZ finality trusted for the test.
#
# Build host only. A fresh rc3 sequencer (127.0.0.1:3500/3501) and anvil (127.0.0.1:8600) of its own,
# fresh keys only (anvil's default accounts; new LEZ wallets). Stops and deletes both at the end
# unless KEEP=1. Nothing here touches any other chain.
#
#   R=<fresh dir> CLI=<lez-bridge with deposit-secret> RECLAIM_HOST=<reclaim_host> \
#     OUTBOUND_HOST=<canonical outbound_host> GUESTS=<dir: bridge_guest.bin token_guest.bin> \
#     SEQ_BIN=<rc3 sequencer_service> NETCFG=<rc3 networks/local.json> CHAIN_SH=<rc3 test-chain script> \
#     lez-bridge/scripts/e2e-failsafe.sh
#
# M6 order (2026-11-28; not run yet: it needs a host that runs the rc3 sequencer): the vault winds
# down; an update after that fixes nothing; the attestor's `wind-down --stand-in` stops LEZ minting
# for good (an admin unfreeze does not reopen it); the next update, which holds that block, fixes
# the final. Every update uses the failsafe guest (`host update --failsafe`, V2 journals). The
# pre-M6 run is public-demo/prove/failsafe-20261117/.
set -euo pipefail
: "${R:?set R to a fresh run directory}" "${CLI:?}" "${RECLAIM_HOST:?}" "${OUTBOUND_HOST:?}" "${GUESTS:?}" "${SEQ_BIN:?}" "${NETCFG:?}"
# CHAIN_SH: a LEZ rc3 test-chain script taking R, SEQ_BIN, NETCFG, CLI, RPC, METRICS and the verbs
# setup | start | stop, which leaves the genesis operator's wallet in $R/wallets/operator and its
# account in $R/operator.env (public-demo/rc3/chain-rc3.sh (not published))
: "${CHAIN_SH:?set CHAIN_SH to the rc3 test-chain script}"
HERE=$(cd "$(dirname "$0")/.." && pwd)
VAULT_DIR=${VAULT_DIR:-$HERE/../vault}
LOCK=${LOCK:-}
LABEL="mock-verified: proofs executed, not proved; LEZ finality trusted for the test"
RPC=127.0.0.1:3500 METRICS=127.0.0.1:3501 SEQ=http://127.0.0.1:3500
ANVIL_PORT=8600 ETH_RPC=http://127.0.0.1:8600
export PATH=$HOME/.foundry/bin:$PATH
export LEZ_BRIDGE_SEQUENCER=$SEQ LEZ_BRIDGE_ETH_RPC=$ETH_RPC LEE_WALLET_PASSWORD= RISC0_PROVER=local RAYON_NUM_THREADS=4
# anvil's default accounts #0..#2 (public test keys of anvil's default test phrase, this anvil only;
# the run checks their addresses): #0 deployer, admin and freezer of the vault; #1 pays the
# deposits; #2 the relayer that submits every reclaim.
K0=$("$HERE/scripts/anvil-dev-key.sh" 0) || exit 1
K1=$("$HERE/scripts/anvil-dev-key.sh" 1) || exit 1
K2=$("$HERE/scripts/anvil-dev-key.sh" 2) || exit 1
A0=$(cast wallet address --private-key $K0); A1=$(cast wallet address --private-key $K1); RELAYER=$(cast wallet address --private-key $K2)
[ "$A0 $A1 $RELAYER" = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266 0x70997970C51812dc3A010C7d01b50e0d17dc79C8 0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC" ] \
  || { echo "FAILED: not anvil's default accounts #0..#2" >&2; exit 1; }
DENOM=100000000; RELAY_FEE=1000000

[ -e "$R" ] && { echo "FAILED: $R exists (use a fresh run dir)" >&2; exit 1; }
mkdir -p "$R/out" "$R/blocks"; chmod 700 "$R"
exec > >(tee -a "$R/failsafe.log") 2>&1
say() { echo; echo "## $(date +%T) $*"; }
die() { echo "FAILED: $*  [$LABEL]" >&2; exit 1; }
ok() { echo "OK: $*"; }
set -E; trap 'echo "FAILED: line $LINENO: $BASH_COMMAND (exit status $?)  [$LABEL]" >&2' ERR
to() { local f=$1; shift; local rc=0; "$@" > "$f" 2>&1 || rc=$?; cat "$f"; [ "$rc" = 0 ] || echo "FAILED: exit $rc from ${*:1:2} ($f)" >&2; return "$rc"; }
host() { ${LOCK:+$LOCK nice -n 19} "$OUTBOUND_HOST" "$@"; }
rhost() { ${LOCK:+$LOCK nice -n 19} "$RECLAIM_HOST" "$@"; }
lower() { tr 'A-F' 'a-f'; }
id_of() { grep -oE "$1/[1-9A-HJ-NP-Za-km-z]{32,44}" | sed -n 1p | cut -d/ -f2; }
kv() { sed -n "s/^$1=//p" | tail -1; }
block_of() { sed -nE 's/.*(applied|included) in block ([0-9]+).*/\2/p' | tail -1; }
ecall() { cast call "$@" --rpc-url $ETH_RPC; }
esend() { local k=$1; shift; cast send "$@" --private-key $k --rpc-url $ETH_RPC --json; }
gas_of() { python3 -c 'import json,sys;j=json.load(open(sys.argv[1]));print(int(j["gasUsed"],16))' "$1"; }
# (the subshell drops the ERR trap: an expected revert is not a failure)
erevert() { local want=$1 k=$2; shift 2; local out; if out=$(trap - ERR; cast send "$@" --private-key $k --rpc-url $ETH_RPC 2>&1); then die "expected $want, the transaction went through"; fi
  local sel; sel=$(cast sig "$want()"); grep -qi -- "${sel#0x}\|$want" <<<"$out" || die "expected $want ($sel), got: $(echo "$out" | tail -3)"; echo "refused as expected: $want"; }
# Every CLI call names its wallet home (the build host's default wallet home is not ours): T uses
# the throwaway chain's operator wallet, `as` the one given.
T() { LEE_WALLET_HOME_DIR=${OPW:?} "$CLI" "$@"; }
as() { local w=$1; shift; LEE_WALLET_HOME_DIR=$w "$CLI" "$@"; }
usdc_bal() { ecall $USDC "balanceOf(address)(uint256)" $1 | cut -d' ' -f1; }
cleanup() {
  [ "${KEEP:-0}" = 1 ] && { echo "KEEP=1: chain and anvil left running in $R"; return; }
  R=$R/chain SEQ_BIN=$SEQ_BIN NETCFG=$NETCFG CLI=$CLI RPC=$RPC METRICS=$METRICS "$CHAIN_SH" stop || true
  [ -f "$R/anvil.pid" ] && kill "$(cat "$R/anvil.pid")" 2>/dev/null || true
}
trap cleanup EXIT
echo "$LABEL"; echo "run $R; CLI $(shasum -a 256 "$CLI" | cut -c1-16)…; reclaim host $(shasum -a 256 "$RECLAIM_HOST" | cut -c1-16)…; outbound host $(shasum -a 256 "$OUTBOUND_HOST" | cut -c1-16)…"

# ------------------------------------------------------------------------------- chains
say "chains: a fresh LEZ rc3 sequencer on $RPC and anvil on $ANVIL_PORT"
lsof -nP -iTCP:3500 -iTCP:3501 -iTCP:$ANVIL_PORT -sTCP:LISTEN >/dev/null 2>&1 && die "a port of 3500/3501/$ANVIL_PORT is busy"
( nohup anvil --port $ANVIL_PORT --host 127.0.0.1 --silent > "$R/anvil.log" 2>&1 < /dev/null & echo $! > "$R/anvil.pid" )
for _ in $(seq 1 30); do cast chain-id --rpc-url $ETH_RPC >/dev/null 2>&1 && break; sleep 1; done
export R_CHAIN=$R/chain
R=$R_CHAIN SEQ_BIN=$SEQ_BIN NETCFG=$NETCFG CLI=$CLI RPC=$RPC METRICS=$METRICS "$CHAIN_SH" setup
R=$R_CHAIN SEQ_BIN=$SEQ_BIN NETCFG=$NETCFG CLI=$CLI RPC=$RPC METRICS=$METRICS "$CHAIN_SH" start
OPW=$R_CHAIN/wallets/operator; OP=$(sed -n 's/^OP=//p' $R_CHAIN/operator.env)
echo "operator (genesis funds, bridge admin, freezer, stand-in attestor) $OP"

# ------------------------------------------------------------------------------- LEZ programs, G2 genesis
say "LEZ: deploy the token and bridge programs from the canonical ELFs (image ids printed)"
START=$(host last --rpc $SEQ)
to $R/out/deploy-token.txt as $OPW deploy --which token --payer $OP --elf $GUESTS/token_guest.bin; TP=$(sed -n 's/^program //p' $R/out/deploy-token.txt)
to $R/out/deploy-bridge.txt as $OPW deploy --which bridge --payer $OP --elf $GUESTS/bridge_guest.bin; BP=$(sed -n 's/^program //p' $R/out/deploy-bridge.txt)
PROGRAM_ID=$(T program-id --bridge $BP | kv PROGRAM_ID)
LEZ_CHAIN_ID=$(as $OPW chain-id | kv LEZ_CHAIN_ID); ZONE_ID=$(T source-id | kv ZONE_ID)
to $R/genesis.txt host genesis --rpc $SEQ --dir $R/blocks --block $START --bridge $BP --out $R/state0.bin
G2_STATE0=$(kv G2_STATE < $R/genesis.txt)
to $R/image-ids.txt host ids
UPDATE_IMAGE_ID=$(kv UPDATE_IMAGE_ID_FAILSAFE < $R/image-ids.txt); RELEASE_IMAGE_ID=$(kv RELEASE_IMAGE_ID < $R/image-ids.txt)
RECLAIM_IMAGE_ID=$(rhost ids | kv RECLAIM_IMAGE_ID)
echo "update image $UPDATE_IMAGE_ID; release image $RELEASE_IMAGE_ID; reclaim image $RECLAIM_IMAGE_ID"

# ------------------------------------------------------------------------------- vault
say "Ethereum: the failsafe vault (MockVerifier; G2 failsafe guest (TrustedTip, V2 journal), G3 and G4 image ids from their hosts)"
out=$(cd "$VAULT_DIR" && PROGRAM_ID=$PROGRAM_ID LEZ_CHAIN_ID=$LEZ_CHAIN_ID ZONE_ID=$ZONE_ID UPDATE_IMAGE_ID=$UPDATE_IMAGE_ID \
  RELEASE_IMAGE_ID=$RELEASE_IMAGE_ID RECLAIM_IMAGE_ID=$RECLAIM_IMAGE_ID G2_STATE=$G2_STATE0 \
  forge script script/FailsafeDeploy.s.sol --rpc-url $ETH_RPC --private-key $K0 --broadcast 2>&1) || { echo "$out"; die "vault deploy"; }
VAULT=$(echo "$out" | sed -n 's/.*VAULT=\(0x[0-9a-fA-F]*\).*/\1/p' | sed -n 1p)
USDC=$(echo "$out" | sed -n 's/.*USDC=\(0x[0-9a-fA-F]*\).*/\1/p' | sed -n 1p)
CFG_ID=$(echo "$out" | sed -n 's/.*CFG_ID=\(0x[0-9a-fA-F]*\).*/\1/p' | sed -n 1p)
EVM_CHAIN_ID=$(cast chain-id --rpc-url $ETH_RPC)
[ "$(ecall $VAULT 'reclaimImageId()(bytes32)' | lower)" = "$(echo $RECLAIM_IMAGE_ID | lower)" ] || die "reclaimImageId"
echo "vault $VAULT usdc $USDC cfg $CFG_ID; runtime $(cast code $VAULT --rpc-url $ETH_RPC | python3 -c 'import sys;print((len(sys.stdin.read().strip())-2)//2)') bytes (EIP-170: 24576)"
esend $K0 $USDC "transfer(address,uint256)" $A1 100000000000 > /dev/null
esend $K1 $USDC "approve(address,uint256)" $VAULT 100000000000 > /dev/null

# ------------------------------------------------------------------------------- LEZ setup
say "LEZ: wrapped USDC (mint authority = the bridge), init (operator = admin, freezer, stand-in attestor), register"
to $R/out/wrapped.txt as $OPW create-wrapped --token-program $TP --bridge $BP --name "Wrapped USDC failsafe" --payer $OP; DEF=$(sed -n 's/^token //p' $R/out/wrapped.txt)
to $R/out/init.txt as $OPW init --bridge $BP --token-program $TP --admin $OP --vault $VAULT --chain-id $EVM_CHAIN_ID --freezer $OP --attestor $OP --payer $OP
to $R/out/register.txt as $OPW register --bridge $BP --admin $OP --eth-token $USDC --definition $DEF --decimals 6 --denoms 100000000,1000000000
DERIVED=$(T cfg-id --vault $VAULT --chain-id $EVM_CHAIN_ID --lez-chain-id $LEZ_CHAIN_ID --zone-id $ZONE_ID --bridge $BP | kv CFG_ID)
[ "$(echo $DERIVED | lower)" = "$(echo $CFG_ID | lower)" ] || die "cfg_id: vault $CFG_ID, bridge $DERIVED"

# ------------------------------------------------------------------------------- users, deposits
say "three users: u1 a fresh LEZ wallet of its own with its own recovery phrase (tests only: the phrase goes to a mode-600 file); u2 and u3 accounts in the operator's wallet (on LEZ rc3 a public transfer needs the recipient's key in the sending wallet, so they can be funded for fees)"
# (bash 3 on macOS: no associative arrays) a user's wallet directory and account
wd() { if [ "$1" = u1 ]; then echo $R/wallet-u1; else echo $OPW; fi; }
acct() { cat $R/account-$1; }
mkdir -p $(wd u1); chmod 700 $(wd u1)
LEZ_BRIDGE_TEST_MNEMONIC_FILE=$R/u1-phrase.txt as $(wd u1) wallet check-health > $R/out/u1-health.txt 2>&1
chmod 600 $R/u1-phrase.txt
for u in u1 u2 u3; do
  as $(wd $u) wallet account new public | id_of Public > $R/account-$u
  echo "$u: LEZ account $(acct $u)"
done
say "Ethereum: one 100 USDC deposit each, through the seed-derived path (deposit-request); anvil account #1 pays"
for u in u1 u2 u3; do
  as $(wd $u) deposit-request --bridge $BP --from $(acct $u) --eth-token $USDC --denom-index 0 > $R/out/$u-request.txt
  C=$(kv COMMITMENT < $R/out/$u-request.txt); J=$(kv KEY_INDEX < $R/out/$u-request.txt)
  esend $K1 $VAULT "deposit(address,uint32,bytes32)" $USDC 0 $C > $R/out/$u-deposit.json
  echo "$u: deposit index $J commitment $C"
done
VAULT_START=$(usdc_bal $VAULT); echo "vault USDC after deposits: $VAULT_START (3 x 100 + 3 x 1 deposit fee)"

# ------------------------------------------------------------------------------- u3 claims
say "LEZ: the operator records the vault root; u3 (funded for fees) claims its deposit by key index"
to $R/out/attest.txt as $OPW attest --bridge $BP --attestor $OP --root $(ecall $VAULT "depositRoot()(bytes32)")
to $R/out/fund-u3.txt as $OPW wallet auth-transfer send --from Public/$OP --to Public/$(acct u3) --amount 200000000000
to $R/out/u3-claim.txt as $(wd u3) claim --bridge $BP --token-program $TP --from $(acct u3) --eth-token $USDC --denom-index 0 --denom $DENOM --key-index 0
L1=$(block_of < $R/out/u3-claim.txt)

say "G2 update 1 (LEZ blocks $((START + 1))..$L1, executor), submitted: the vault counts u3's claim"
to $R/update1.txt host update --failsafe --rpc $SEQ --dir $R/blocks --state $R/state0.bin --to $L1 --out $R/update1
esend $K0 $VAULT "update(bytes,bytes)" $(cat $R/update1/journal.hex) $(cat $R/update1/seal.hex) > $R/out/update1-tx.json
echo "counters (deposits, claims, burns, releases): $(ecall $VAULT 'counters(address,uint256)(uint64,uint64,uint64,uint64)' $USDC 0 | tr '\n' ' ')"

# ------------------------------------------------------------------------------- wind-down
say "WIND-DOWN 1/3, Ethereum: the vault admin calls windDown(); deposits are refused from now on"
esend $K0 $VAULT "windDown()" > $R/out/winddown-tx.json
echo "windDownAt $(ecall $VAULT 'windDownAt()(uint64)'); wind-down slot $(cast storage $VAULT $(ecall $VAULT 'WIND_DOWN_SLOT()(bytes32)') --rpc-url $ETH_RPC); finalState $(ecall $VAULT 'finalState()(bytes32)') (not yet)"
erevert IsWoundDown $K1 $VAULT "deposit(address,uint32,bytes32)" $USDC 0 $(cast keccak "late-deposit-$R")
erevert NotFinal $K2 $VAULT "reclaim(bytes,bytes32,bytes32,address,uint32,address,address,uint256)" 0x $(ecall $VAULT "depositRoot()(bytes32)") $(cast keccak x) $USDC 0 $A1 0x0000000000000000000000000000000000000000 0
say "G2 update 2 (one more LEZ block), after windDown but before LEZ stopped minting: counted, fixes nothing"
to $R/out/tick.txt as $OPW wallet auth-transfer send --from Public/$OP --to Public/$(acct u2) --amount 1
L2=$(block_of < $R/out/tick.txt)
to $R/update2.txt host update --failsafe --rpc $SEQ --dir $R/blocks --state $R/update1/next-state.bin --to $L2 --out $R/update2
esend $K0 $VAULT "update(bytes,bytes)" $(cat $R/update2/journal.hex) $(cat $R/update2/seal.hex) > $R/out/update2-tx.json
[ "$(ecall $VAULT 'finalState()(bytes32)')" = 0x0000000000000000000000000000000000000000000000000000000000000000 ] || die "update 2 fixed the final before LEZ stopped minting"
echo "finalState still unset"
say "WIND-DOWN 2/3, LEZ: the stand-in attestor stops minting for good (wind-down --stand-in)"
to $R/out/wind-down.txt as $OPW wind-down --bridge $BP --from $OP --stand-in
LW=$(block_of < $R/out/wind-down.txt)
to $R/out/show-wound.txt T show --bridge $BP
grep -q "mint_wound_down true" $R/out/show-wound.txt || die "the bridge is not wound down"
say "an admin unfreeze does not reopen minting; u2's LEZ claim is refused (minting is wound down)"
to $R/out/unfreeze.txt as $OPW unfreeze --bridge $BP --admin $OP --mint
to $R/out/fund-u2.txt as $OPW wallet auth-transfer send --from Public/$OP --to Public/$(acct u2) --amount 200000000000
if as $(wd u2) claim --bridge $BP --token-program $TP --from $(acct u2) --eth-token $USDC --denom-index 0 --denom $DENOM --key-index 0 > $R/out/u2-claim-after-wind-down.txt 2>&1; then
  cat $R/out/u2-claim-after-wind-down.txt; die "a claim went through after the wind-down"; fi
grep -q "wound down" $R/out/u2-claim-after-wind-down.txt || { cat $R/out/u2-claim-after-wind-down.txt; die "claim refused for another reason"; }
echo "u2's claim refused: minting is wound down"
say "WIND-DOWN 3/3: G2 update 3 holds the wind-down (LEZ block $LW): its journal carries GLOBAL and the vault fixes the final"
to $R/out/tick3.txt as $OPW wallet auth-transfer send --from Public/$OP --to Public/$(acct u2) --amount 1
L3=$(block_of < $R/out/tick3.txt)
to $R/update3.txt host update --failsafe --rpc $SEQ --dir $R/blocks --state $R/update2/next-state.bin --to $L3 --out $R/update3
esend $K0 $VAULT "update(bytes,bytes)" $(cat $R/update3/journal.hex) $(cat $R/update3/seal.hex) > $R/out/update3-tx.json
FINAL=$(ecall $VAULT "finalState()(bytes32)")
NEW3=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["newState"])' $R/update3/report.json)
[ "$(echo $FINAL | lower)" = "$(echo $NEW3 | lower)" ] || die "finalState $FINAL is not update 3's newState $NEW3"
[ "$(ecall $VAULT 'mintStoppedBlock()(uint64)')" = "$LW" ] || die "mintStoppedBlock is not the wind-down block $LW"
python3 -c 'import json,sys;s=json.load(open(sys.argv[1]));print("final accumulator: root", s["claimAccRoot"], "count", s["claimAccCount"])' $R/update3/next-state.json
ok "final state fixed: $FINAL"

# ------------------------------------------------------------------------------- recovery of u1 from the phrase alone
say "u1 loses everything but the recovery phrase: its wallet directory (keys and the bridge client cache) is deleted"
rm -rf $(wd u1); [ ! -e $(wd u1) ] || die "wallet still there"
as $(wd u1) wallet restore-keys --depth 4 --yes < $R/u1-phrase.txt > $R/out/u1-restore.txt 2>&1 || { tail -20 $R/out/u1-restore.txt; die "restore-keys"; }
chmod 700 $(wd u1)
as $(wd u1) wallet account list > $R/out/u1-accounts.txt 2>&1 || true
# u1's account never touched LEZ; if the restore did not regenerate it, derive the phrase's next
# public accounts (the same key tree) until it appears
for _ in 1 2 3; do
  grep -q "$(acct u1)" $R/out/u1-accounts.txt && break
  as $(wd u1) wallet account new public >> $R/out/u1-accounts-new.txt 2>&1
  as $(wd u1) wallet account list > $R/out/u1-accounts.txt 2>&1 || true
done
grep -q "$(acct u1)" $R/out/u1-accounts.txt || die "the restored wallet does not hold u1's account"
echo "the restored wallet (from the phrase alone) holds u1's account $(acct u1)"
to $R/out/u1-recover.txt as $(wd u1) recover --bridge $BP
say "u1: the deposit's master secret, re-derived from the restored wallet (written to a mode-600 file, never printed)"
mkdir -p $R/secrets; chmod 700 $R/secrets
for u in u1 u2 u3; do
  to $R/out/$u-deposit-secret.txt as $(wd $u) deposit-secret --bridge $BP --from $(acct $u) --eth-token $USDC --denom $DENOM --key-index 0 --out $R/secrets/$u.master
done

# ------------------------------------------------------------------------------- reclaims
cast logs --from-block 0 --address $VAULT "Deposit(address indexed,uint32,bytes32,uint64,bytes32)" --rpc-url $ETH_RPC --json > $R/deposit-logs.json
python3 - "$R/deposit-logs.json" > $R/deposits.txt <<'EOF'
import json, sys
for l in json.load(open(sys.argv[1])):
    d = l["data"][2:]
    print("0x" + l["topics"][1][-40:], int(d[0:64], 16), "0x" + d[64:128], int(d[128:192], 16))
EOF
: > $R/leaves.txt
while read -r tok di c idx; do ecall $VAULT "depositLeaf(address,uint256,bytes32)(bytes32)" $tok $DENOM $c >> $R/leaves.txt; done < $R/deposits.txt
F1=$(cast keccak "failsafe-fresh-1-$R" | cut -c1-42); F2=$(cast keccak "failsafe-fresh-2-$R" | cut -c1-42)
SIG="reclaim(bytes,bytes32,bytes32,address,uint32,address,address,uint256)"
reclaim_proof() { # $1 user $2 recipient $3 relay fee -> $R/reclaim-$1.txt (executor run)
  rhost reclaim --master-file $R/secrets/$1.master --dest $(acct $1) --token $USDC --denom $DENOM --chain-id $EVM_CHAIN_ID --vault $VAULT \
    --lez-chain-id $LEZ_CHAIN_ID --zone-id $ZONE_ID --program-id $PROGRAM_ID --leaves $R/leaves.txt --state $R/update3/next-state.bin \
    --recipient $2 --relayer $( [ "$3" = 0 ] && echo 0x0000000000000000000000000000000000000000 || echo $RELAYER ) --relay-fee $3 > $R/reclaim-$1.txt 2>&1
}
args_of() { # $1 user, [$2 recipient override] -> calldata args
  local f=$R/reclaim-$1.txt; local fee rel; fee=$(cat $R/reclaim-$1.fee)
  rel=$( [ "$fee" = 0 ] && echo 0x0000000000000000000000000000000000000000 || echo $RELAYER )
  echo "$(kv SEAL < $f) $(kv DEPOSIT_ROOT < $f) $(kv NULLIFIER < $f) $USDC 0 ${2:-$(cat $R/reclaim-$1.to)} $rel $fee"
}
say "balances before the reclaims"
B_V=$(usdc_bal $VAULT); B_F1=$(usdc_bal $F1); B_F2=$(usdc_bal $F2); B_R=$(usdc_bal $RELAYER)
echo "vault $B_V; fresh1 $F1 $B_F1; fresh2 $F2 $B_F2; relayer $RELAYER $B_R"

say "G4 (executor) for u1 -> fresh1 through the relayer (relay fee $RELAY_FEE), for u2 -> fresh2 (no fee)"
reclaim_proof u1 $F1 $RELAY_FEE; cat $R/reclaim-u1.txt | grep -v -E '^(JOURNAL|SEAL)='; echo $F1 > $R/reclaim-u1.to; echo $RELAY_FEE > $R/reclaim-u1.fee
reclaim_proof u2 $F2 0; cat $R/reclaim-u2.txt | grep -v -E '^(JOURNAL|SEAL)='; echo $F2 > $R/reclaim-u2.to; echo 0 > $R/reclaim-u2.fee
[ "$(kv FINAL_STATE < $R/reclaim-u1.txt | lower)" = "$(echo $FINAL | lower)" ] || die "the guest's final state is not the vault's (HostState layout?)"
say "the relayer tries to redirect u2's reclaim to itself: refused (the recipient is in the proof)"
erevert BadSeal $K2 $VAULT "$SIG" $(args_of u2 $RELAYER)
say "the relayer (anvil #2, not the depositor) submits both reclaims as made"
esend $K2 $VAULT "$SIG" $(args_of u1) > $R/out/reclaim-u1-tx.json
esend $K2 $VAULT "$SIG" $(args_of u2) > $R/out/reclaim-u2-tx.json
echo "gas: reclaim u1 (relay fee) $(gas_of $R/out/reclaim-u1-tx.json), reclaim u2 (no fee) $(gas_of $R/out/reclaim-u2-tx.json)"
say "u1's reclaim again (same reclaim nullifier): refused"
erevert NullifierSpent $K2 $VAULT "$SIG" $(args_of u1)
say "u3's deposit was claimed on LEZ: the reclaim host finds it in the final accumulator, so no proof exists"
rc=0; reclaim_proof u3 $F2 0 || rc=$?
cat $R/reclaim-u3.txt
[ "$rc" = 3 ] && grep -q "REFUSED: claimed" $R/reclaim-u3.txt || die "u3's reclaim was not refused as claimed (exit $rc)"

say "balances after"
A_V=$(usdc_bal $VAULT); A_F1=$(usdc_bal $F1); A_F2=$(usdc_bal $F2); A_R=$(usdc_bal $RELAYER)
echo "vault $B_V -> $A_V; fresh1 $B_F1 -> $A_F1; fresh2 $B_F2 -> $A_F2; relayer $B_R -> $A_R"
[ "$A_F1" = "$((B_F1 + DENOM - RELAY_FEE))" ] || die "fresh1"
[ "$A_F2" = "$((B_F2 + DENOM))" ] || die "fresh2"
[ "$A_R" = "$((B_R + RELAY_FEE))" ] || die "relayer"
[ "$A_V" = "$((B_V - 2 * DENOM))" ] || die "vault"
echo "counters (deposits, claims, burns, releases): $(ecall $VAULT 'counters(address,uint256)(uint64,uint64,uint64,uint64)' $USDC 0 | tr '\n' ' '); reclaims $(ecall $VAULT 'reclaims(address,uint256)(uint64)' $USDC 0)"
FEES=$(ecall $VAULT 'tokenConfig(address)(bool,bool,bool,uint8,uint64,uint256[],uint256[],uint256[],uint256)' $USDC | sed -n 9p | cut -d' ' -f1)
[ "$A_V" = "$((DENOM + FEES))" ] || die "the vault should hold exactly u3's claimed deposit plus the accrued fees"
ok "vault holds $A_V = u3's claimed 100 USDC (backing its wrapped tokens) + $FEES deposit fees; both unclaimed deposits paid to fresh addresses"
for u in u1 u2; do echo "$u cycles: $(kv USER_CYCLES < $R/reclaim-$u.txt) user / $(kv TOTAL_CYCLES < $R/reclaim-$u.txt) total / $(kv SEGMENTS < $R/reclaim-$u.txt) segments"; done
say "done. $LABEL"
