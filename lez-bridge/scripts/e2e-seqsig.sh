#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# The "sequencer signatures" withdrawal route end to end on throwaway local chains (branch seq-sig):
#   deposit (anvil) -> claim (LEZ) -> burn (LEZ) -> an update signed by 3 of 4 signer keys, staged
#   through the open SeqSigGate by an account with no role, finalized after the contest window ->
#   release paid; 2 of 4 refused (in the guest, and a threshold-2 journal at the gate); a conflicting
#   hash signed by a key that did NOT sign the pending update cancels nothing; a key that DID sign it
#   signs a conflicting hash (equivocation): the update is cancelled, the vault left as before, the
#   key ejected for good (its signature no longer counts); an update past it without that key; then
#   a quorum of active keys signs a conflicting hash and the gate halts.
#
#   mock-verified: proofs executed, not proved; signer keys are STAND-INS (ours), not LEZ sequencer
#   keys (LEZ v0.4.0-rc3 standalone headers carry no signature, and there is no Logos Blockchain
#   node here whose inscriptions carry the sequencers' signatures).
#
# Build host only. A throwaway LEZ rc3 chain from the testnet kit (RPC LEZ_PORT, metrics +1, proxy
# port +5 reserved: 3470/3471/3475 by default) and a throwaway anvil (ANVIL_PORT, 8570); both are
# stopped at the end unless KEEP=1. The vault's verifier is the gate in front of the TEST
# MockVerifier; every G2 and G3 journal comes from RISC Zero's executor.
#
#   R=<fresh dir> KIT=<kit>/testnet SEQ_BIN=… WALLET_BIN=… CLI_SRC=<lez-bridge CLI> \
#   HOST=<outbound_host> SEQSIG=<seqsig> lez-bridge/scripts/e2e-seqsig.sh
set -euo pipefail
: "${R:?set R to a fresh run directory}" "${KIT:?}" "${SEQ_BIN:?}" "${WALLET_BIN:?}" "${CLI_SRC:?}" "${HOST:?}" "${SEQSIG:?}"
HERE=$(cd "$(dirname "$0")/.." && pwd)
VAULT_DIR=${VAULT_DIR:-$HERE/../vault}
LOCK=${LOCK:-<local path>}
LEZ_PORT=${LEZ_PORT:-3470}; ANVIL_PORT=${ANVIL_PORT:-8570}; BLOCK_TIME=${BLOCK_TIME:-3}
WINDOW=${WINDOW:-60}; THRESHOLD=3; DEPTH=${DEPTH:-3}   # the attestors sign only blocks DEPTH under the head
SEQ=http://127.0.0.1:$LEZ_PORT; ETH_RPC=http://127.0.0.1:$ANVIL_PORT
LABEL="mock-verified: proofs executed, not proved; signer keys are stand-ins (ours), not LEZ sequencer keys"
export PATH=$HOME/.foundry/bin:$PATH
export LEE_WALLET_HOME_DIR=$R/wallet LEZ_BRIDGE_SEQUENCER=$SEQ LEE_WALLET_PASSWORD= RISC0_PROVER=local RAYON_NUM_THREADS=4
export TMPDIR=${TMPDIR_RUN:-$R/tmp}
# anvil default accounts (public test keys): #3 deploys and is the vault admin; #5 and #6 hold no role
EK=$("$HERE/scripts/anvil-dev-key.sh" 3) || exit 1
K5=$("$HERE/scripts/anvil-dev-key.sh" 5) || exit 1
K6=$("$HERE/scripts/anvil-dev-key.sh" 6) || exit 1
LOCAL_TEST_KEY=$("$HERE/scripts/lez-test-key.sh") || exit 1   # LEZ published genesis test key, read from LEZ
ETH=0x0000000000000000000000000000000000000000
D=10000000000000000; DF=100000000000000; WF=50000000000000   # 0.01 ETH, its deposit and withdraw fees

[ -e "$R" ] && { echo "FAILED: $R exists (use a fresh run dir)" >&2; exit 1; }
for p in $LEZ_PORT $((LEZ_PORT + 1)) $((LEZ_PORT + 5)) $ANVIL_PORT; do
  case $p in 3290|3291|3295) echo "refusing the public demo chain's port $p" >&2; exit 1 ;; esac
  python3 -c 'import socket,sys; s=socket.socket(); s.settimeout(0.5); sys.exit(0 if s.connect_ex(("127.0.0.1", int(sys.argv[1]))) else 1)' $p \
    || { echo "FAILED: port $p in use" >&2; exit 1; }
done
mkdir -p "$R"/{bin,blocks,out,keys,cp,upd,rel,tmp}; chmod 700 "$R/keys"
exec > >(tee -a "$R/seqsig.log") 2>&1
say() { echo; echo "## $(date +%T) $*"; }
die() { echo "FAILED: $*  [$LABEL]" >&2; exit 1; }
set -E; trap 'echo "FAILED: line $LINENO: $BASH_COMMAND (exit status $?)  [$LABEL]" >&2' ERR
to() { local f=$1; shift; local rc=0; "$@" > "$f" 2>&1 || rc=$?; cat "$f"; [ "$rc" = 0 ] || echo "FAILED: exit $rc from ${*:1:2} (output: $f)" >&2; return "$rc"; }
ok() { echo "OK: $*  [$LABEL]"; }
lower() { tr 'A-F' 'a-f'; }
heavy() { MIN_FREE=${MIN_FREE:-15} $LOCK nice -n 19 "$@"; }
cp "$CLI_SRC" "$R/bin/lez-bridge"; T=$R/bin/lez-bridge
id_of() { grep -oE "$1/[1-9A-HJ-NP-Za-km-z]{32,44}" | sed -n 1p | cut -d/ -f2; }
block_of() { sed -nE 's/.*(applied|included) in block ([0-9]+).*/\2/p' | tail -1; }
ecall() { cast call "$@" --rpc-url $ETH_RPC; }
TXS=$R/transactions.tsv; printf 'step\tfrom\ttx\tgas\tresult\n' > $TXS
# esend <step> <key> <to> <sig> args...: send, record tx hash and gas
esend() { local step=$1 key=$2; shift 2; local out; out=$(cast send "$@" --private-key $key --rpc-url $ETH_RPC --json) || die "$step: transaction failed"
  local h g; h=$(echo "$out" | python3 -c 'import json,sys;print(json.load(sys.stdin)["transactionHash"])'); g=$(echo "$out" | python3 -c 'import json,sys;print(int(json.load(sys.stdin)["gasUsed"],16))')
  printf '%s\t%s\t%s\t%s\tok\n' "$step" "$(cast wallet address --private-key $key)" "$h" "$g" >> $TXS; echo "$step: tx $h gas $g"; }
# erevert <step> <key> <want error> <to> <sig> args...: must revert with that custom error
erevert() { local step=$1 key=$2 want=$3; shift 3; local out; if out=$(trap - ERR; cast send "$@" --private-key $key --rpc-url $ETH_RPC 2>&1); then die "$step: expected $want, the transaction went through"; fi
  local sel; sel=$(cast sig "$want()"); grep -qi -- "${sel#0x}\|$want" <<<"$out" || die "$step: expected $want ($sel), got: $(echo "$out" | tail -3)"
  printf '%s\t%s\t-\t-\trefused: %s\n' "$step" "$(cast wallet address --private-key $key)" "$want" >> $TXS; echo "$step: refused as expected: $want"; }
cleanup() { if [ "${KEEP:-0}" != 1 ]; then echo "cleanup: stopping anvil and the chain"; [ -n "${ANVIL_PID:-}" ] && kill $ANVIL_PID 2>/dev/null || true
  "$KIT/down.sh" --home "$R/kit" > /dev/null 2>&1 || true; fi; }
trap cleanup EXIT
echo "$LABEL"; echo "run $R; LEZ $SEQ; anvil $ETH_RPC; window ${WINDOW}s; threshold $THRESHOLD of 4"

# ------------------------------------------------------------------------------- 1. chains
say "Throwaway LEZ rc3 chain (kit, genesis test key) on $LEZ_PORT and anvil on $ANVIL_PORT"
SEQ_BIN=$SEQ_BIN WALLET_BIN=$WALLET_BIN "$KIT/up.sh" --genesis-key default --port $LEZ_PORT --block-time $BLOCK_TIME --home "$R/kit" > $R/out/up.txt 2>&1 || { cat $R/out/up.txt; die "up.sh"; }
tail -5 $R/out/up.txt
anvil --host 127.0.0.1 --port $ANVIL_PORT --silent > $R/anvil.log 2>&1 < /dev/null & ANVIL_PID=$!
for _ in $(seq 1 30); do cast chain-id --rpc-url $ETH_RPC > /dev/null 2>&1 && break; sleep 1; done
EVM_CHAIN_ID=$(cast chain-id --rpc-url $ETH_RPC)
EA=$(cast wallet address --private-key $EK); A5=$(cast wallet address --private-key $K5); A6=$(cast wallet address --private-key $K6)
echo "anvil chain $EVM_CHAIN_ID; deployer/admin $EA; no-role accounts $A5 (submits) and $A6 (finalizes, cancels, releases)"

# ------------------------------------------------------------------------------- 2. LEZ wallet, programs, G2 genesis
say "LEZ: wallet, genesis account F, our account P"
to $R/out/health.txt $T wallet check-health > /dev/null
to $R/out/import-f.txt $T wallet account import public --private-key $LOCAL_TEST_KEY > /dev/null; F=$(id_of Public < $R/out/import-f.txt)
to $R/out/new-p.txt $T wallet account new public > /dev/null; P=$(id_of Public < $R/out/new-p.txt)
to $R/out/fund-p.txt $T wallet auth-transfer send --from Public/$F --to Public/$P --amount 200000000000 > /dev/null
START=$($HOST last --rpc $SEQ)
say "LEZ: G2 genesis at block $START; deploy token and bridge programs"
to $R/out/deploy-token.txt $T deploy --which token --payer $P; TP=$(sed -n 's/^program //p' $R/out/deploy-token.txt)
to $R/out/deploy-bridge.txt $T deploy --which bridge --payer $P; BP=$(sed -n 's/^program //p' $R/out/deploy-bridge.txt)
PROGRAM_ID=$($T program-id --bridge $BP | sed -n 's/^PROGRAM_ID=//p')
LEZ_CHAIN_ID=$($T chain-id | sed -n 's/^LEZ_CHAIN_ID=//p'); ZONE_ID=$($T source-id | sed -n 's/^ZONE_ID=//p')
to $R/genesis.txt $HOST genesis --rpc $SEQ --dir $R/blocks --block $START --bridge $BP --out $R/state0.bin
G2_STATE0=$(sed -n 's/^G2_STATE=//p' $R/genesis.txt)
to $R/ids.txt $SEQSIG ids
UPDATE_IMAGE_ID=$(sed -n 's/^SEQSIG_UPDATE_IMAGE_ID=//p' $R/ids.txt); EVIDENCE_IMAGE_ID=$(sed -n 's/^SEQSIG_EVIDENCE_IMAGE_ID=//p' $R/ids.txt)
RELEASE_IMAGE_ID=$($HOST ids | sed -n 's/^RELEASE_IMAGE_ID=//p')
[ "$RELEASE_IMAGE_ID" = "$(sed -n 's/^RELEASE_IMAGE_ID=//p' $R/ids.txt)" ] || die "release image: outbound_host and seqsig disagree"

# ------------------------------------------------------------------------------- 3. stand-in signer keys
say "Stand-in signer keys (ours, NOT LEZ sequencer keys): k0..k3 form the set, k4 is an outsider"
for i in 0 1 2 3 4; do $SEQSIG keygen --out $R/keys/k$i.key | sed -n 's/^PUBKEY=//p' > $R/keys/k$i.pub; echo "k$i $(cat $R/keys/k$i.pub)"; done
to $R/set.txt $SEQSIG set --keys $(cat $R/keys/k0.pub),$(cat $R/keys/k1.pub),$(cat $R/keys/k2.pub),$(cat $R/keys/k3.pub) --out $R/set.json
SET_HASH=$(sed -n 's/^SET_HASH=//p' $R/set.txt)
SIGNER_KEYS=$(python3 -c 'import json,sys;print(",".join(json.load(open(sys.argv[1]))["keys"]))' $R/set.json)
idx() { python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["keys"].index(open(sys.argv[2]).read().strip()))' $R/set.json $R/keys/k$1.pub; }
for i in 0 1 2 3; do echo "k$i is set index $(idx $i)"; done

# ------------------------------------------------------------------------------- 4. vault and gate
say "Ethereum: MockVerifier, SeqSigGate (update image $UPDATE_IMAGE_ID, evidence image $EVIDENCE_IMAGE_ID, set $SET_HASH, threshold $THRESHOLD, window ${WINDOW}s), BridgeVault behind it"
out=$(cd "$VAULT_DIR" && PROGRAM_ID=$PROGRAM_ID LEZ_CHAIN_ID=$LEZ_CHAIN_ID ZONE_ID=$ZONE_ID UPDATE_IMAGE_ID=$UPDATE_IMAGE_ID EVIDENCE_IMAGE_ID=$EVIDENCE_IMAGE_ID \
  RELEASE_IMAGE_ID=$RELEASE_IMAGE_ID G2_STATE=$G2_STATE0 SIGNER_KEYS=$SIGNER_KEYS THRESHOLD=$THRESHOLD CONTEST_WINDOW=$WINDOW TEST_ONLY_SHORT_WINDOW=true \
  forge script script/SeqSigDeploy.s.sol --rpc-url $ETH_RPC --private-key $EK --broadcast 2>&1) || { echo "$out"; die "deploy"; }
VAULT=$(echo "$out" | sed -n 's/.*VAULT=\(0x[0-9a-fA-F]*\).*/\1/p' | sed -n 1p); GATE=$(echo "$out" | sed -n 's/.*GATE=\(0x[0-9a-fA-F]*\).*/\1/p' | sed -n 1p)
CFG_ID=$(echo "$out" | sed -n 's/.*CFG_ID=\(0x[0-9a-fA-F]*\).*/\1/p' | sed -n 1p)
[ -n "$VAULT" ] && [ -n "$GATE" ] && [ -n "$CFG_ID" ] || { echo "$out"; die "deploy printed no VAULT/GATE/CFG_ID"; }
[ "$(ecall $VAULT 'verifier()(address)' | lower)" = "$(echo $GATE | lower)" ] || die "vault verifier is not the gate"
[ "$(ecall $GATE 'SIGNER_SET_HASH()(bytes32)' | lower)" = "$(echo $SET_HASH | lower)" ] || die "the gate's set hash (computed on chain from the keys) differs from seqsig's"
echo "the gate computed the set hash from the keys: equals seqsig's $SET_HASH; TEST_ONLY_SHORT_WINDOW = $(ecall $GATE 'TEST_ONLY_SHORT_WINDOW()(bool)') (window ${WINDOW}s, under the real minimum $(ecall $GATE 'MIN_CONTEST_WINDOW()(uint64)')s)"
DERIVED=$($T cfg-id --vault $VAULT --chain-id $EVM_CHAIN_ID --lez-chain-id $LEZ_CHAIN_ID --zone-id $ZONE_ID --bridge $BP | sed -n 's/^CFG_ID=//p')
[ "$(echo $DERIVED | lower)" = "$(echo $CFG_ID | lower)" ] || die "cfg_id: vault $CFG_ID, bridge would derive $DERIVED"
echo "vault $VAULT gate $GATE cfgId $CFG_ID; g2State = genesis $G2_STATE0"
GS="submit(bytes,bytes)"; GF="finalize(bytes)"; GC="contest(bytes,bytes)"
ejected() { ecall $GATE 'ejected()(uint64)' | cut -d' ' -f1; }
RSIG="release(bytes,bytes32,bytes32,address,uint32,address,address,uint256)"

# ------------------------------------------------------------------------------- 5. LEZ setup, deposits, claims
say "LEZ: wrapped ETH (mint authority = the bridge), init (stand-in root attestor P), register"
to $R/out/wrapped-eth.txt $T create-wrapped --token-program $TP --bridge $BP --name "Wrapped ETH seqsig" --payer $P > /dev/null; DEF_ETH=$(sed -n 's/^token //p' $R/out/wrapped-eth.txt)
to $R/out/init.txt $T init --bridge $BP --token-program $TP --admin $P --vault $VAULT --chain-id $EVM_CHAIN_ID --freezer $P --attestor $P --payer $P > /dev/null
to $R/out/register.txt $T register --bridge $BP --admin $P --eth-token $ETH --definition $DEF_ETH --decimals 18 --denoms $D,100000000000000000 > /dev/null
dep_secret() { cast keccak "seqsig-deposit-$1-$P"; }
say "Ethereum: 3 deposits of 0.01 ETH for P"
for i in 0 1 2; do
  C=$($T commitment --secret $(dep_secret $i) --dest $P --eth-token $ETH --denom $D --cfg-id $CFG_ID)
  esend deposit-$i $EK $VAULT "depositETH(uint32,bytes32)" 0 $C --value $((D + DF))
done
: > $R/leaves.txt
for i in 0 1 2; do
  C=$($T commitment --secret $(dep_secret $i) --dest $P --eth-token $ETH --denom $D --cfg-id $CFG_ID)
  ecall $VAULT "depositLeaf(address,uint256,bytes32)(bytes32)" $ETH $D $C >> $R/leaves.txt
done
ROOT=$(ecall $VAULT "depositRoot()(bytes32)")
say "LEZ: record the vault root (stand-in attestor), claim the 3 deposits into P"
to $R/out/attest.txt $T attest --bridge $BP --attestor $P --root $ROOT > /dev/null
for i in 0 1 2; do
  to $R/out/claim-$i.txt $T claim --bridge $BP --token-program $TP --from $P --eth-token $ETH --denom-index 0 --denom $D \
    --secret $(dep_secret $i) --cfg-id $CFG_ID --leaves $R/leaves.txt --index $i | tail -1
done

# ------------------------------------------------------------------------------- 6. burn 0, signatures, executor runs
x_secret() { cast keccak "seqsig-exit-$1-$P"; }
x_dest() { cast keccak "seqsig-recipient-$1" | cut -c1-42; }
burn() { local E; E=$($T exit-commitment --secret $(x_secret $1) --eth-dest $(x_dest $1) --eth-token $ETH --denom $D --max-relay-fee 0 --cfg-id $CFG_ID | sed -n 1p)
  echo "$1 $E" >> $R/exit-commitments.txt
  to $R/out/burn-$1.txt $T burn --bridge $BP --token-program $TP --from $P --definition $DEF_ETH --amount $D --exit $E | tail -1; }
say "LEZ: burn 0.01 wETH -> exit 0"
: > $R/exit-commitments.txt; burn 0; LAST1=$(block_of < $R/out/burn-0.txt)
# a few more blocks so the batch end is not the burn's own block
sleep $((BLOCK_TIME * 2)); LAST1=$($HOST last --rpc $SEQ)
attest_all() { # $1 tag, $2 state, $3 block: every key signs the checkpoint (state tip -> block)
  for i in 0 1 2 3 4; do $SEQSIG attest --key $R/keys/k$i.key --rpc $SEQ --dir $R/blocks --state $2 --cfg-id $CFG_ID --to $3 \
    --record $R/keys/k$i.record --final-depth $DEPTH --out $R/cp/$1-k$i.json; done; }
# a block counts as final for the attestors once it is DEPTH blocks under the head
wait_final() { while [ $(( $($HOST last --rpc $SEQ) )) -lt $(( $1 + DEPTH )) ]; do sleep 1; done; }
say "Attestors: each stand-in key replays blocks $((START+1))..$LAST1 itself, then signs the checkpoint (cfgId, genesis tip $START, block $LAST1)"
wait_final $LAST1; attest_all u1 $R/state0.bin $LAST1
cps() { local t=$1; shift; local s=""; for k in "$@"; do s="$s,$R/cp/$t-k$k.json"; done; echo "${s#,}"; }
upd() { local name=$1 state=$2 to=$3 t=$4 ks=$5; shift 5
  to $R/upd/$name.txt heavy $SEQSIG update --rpc $SEQ --dir $R/blocks --state $state --to $to --set $R/set.json --threshold $t --ejected "$(ejected)" --sigs "$ks" --out $R/upd/$name "$@"; }
say "Executor: the seq-sig guest refuses 2 of 4 (k0, k1; threshold 3)"
upd u1-2of4 $R/state0.bin $LAST1 3 "$(cps u1 0 1)" --expect-refusal
say "Executor: k1 signing twice counts once (k0, k1, k1; threshold 3): refused"
upd u1-dup $R/state0.bin $LAST1 3 "$(cps u1 0 1 1)" --expect-refusal
say "Executor: an outsider's signature (k4) presented as set member 2 (k0, k1, k4): refused"
upd u1-outsider $R/state0.bin $LAST1 3 "$(cps u1 0 1 4)" --expect-refusal --outsider-index 2
say "Executor: 2 of 4 with the threshold claimed as 2: the guest refuses a threshold that is not a strict majority"
upd u1-t2 $R/state0.bin $LAST1 2 "$(cps u1 0 1)" --expect-refusal
say "Executor: 4 of 4 with the threshold claimed as 4 (a journal the gate must refuse: its threshold is 3; also the per-signature cycles)"
upd u1-4of4 $R/state0.bin $LAST1 4 "$(cps u1 0 1 2 3)"
say "Executor: update 1 signed by 3 of 4 (k0, k1, k3)"
upd u1 $R/state0.bin $LAST1 3 "$(cps u1 0 1 3)"
J1=$(cat $R/upd/u1/journal.hex); S1=$(cat $R/upd/u1/seal.hex); VJ1=$(cat $R/upd/u1/vault-journal.hex)
NEW1=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["newState"])' $R/upd/u1/report.json)

# ------------------------------------------------------------------------------- 7. gate: update 1
say "Ethereum: the threshold-4 journal is refused by the gate (its THRESHOLD is 3)"
erevert submit-threshold-4 $K5 WrongThreshold $GATE "$GS" $(cat $R/upd/u1-4of4/journal.hex) $(cat $R/upd/u1-4of4/seal.hex)
say "Ethereum: update 1's journal with the 4-of-4 run's seal is refused (the seal binds threshold and signer bitmap)"
erevert submit-wrong-seal $K5 BadSeal $GATE "$GS" $J1 $(cat $R/upd/u1-4of4/seal.hex)
# the release proof for exit 0 against the root update 1 will create (made before the submit, so the
# checks below run inside the window)
python3 - 0 $R/exit-commitments.txt "$CFG_ID" $ETH $D > $R/exit-leaves-expected.txt <<'EOF'
import hashlib, sys
j, ecf, cfg, tok, d = sys.argv[1:]
E = dict(l.split() for l in open(ecf))[j]
tag = bytes.fromhex("22018742ad6028fda6cb0726afaae90ea20dff3cef6b1ec5b3c14f856d76c59c")  # keccak256("RFP022/exit-leaf/v1")
print("0x" + hashlib.sha256(tag + bytes.fromhex(cfg[2:]) + bytes.fromhex(tok[2:]) + int(d).to_bytes(32, "big") + bytes.fromhex(E[2:])).hexdigest())
EOF
prove_release() { # $1 exit, $2 leaves file, $3 index
  to $R/rel/$1.txt heavy $HOST release --secret $(x_secret $1) --eth-dest $(x_dest $1) --token $ETH --denom $D --max-relay-fee 0 --cfg-id $CFG_ID \
    --leaves $2 --index $3 --relayer $ETH --relay-fee 0 > /dev/null
  grep -E '^(ROOT|NULLIFIER|SEAL|USER_CYCLES)=' $R/rel/$1.txt > $R/rel/$1.env; }
prove_release 0 $R/exit-leaves-expected.txt 0; . $R/rel/0.env
say "Ethereum: update 1 (3 of 4) submitted by $A5, an account with no role"
esend submit-u1 $K5 $GATE "$GS" $J1 $S1
AT1=$(ecall $GATE "finalizableAt()(uint256)" | cut -d' ' -f1)
[ "$(ecall $VAULT 'g2State()(bytes32)' | lower)" = "$(echo $G2_STATE0 | lower)" ] || die "the vault moved while the update is pending"
echo "pending; finalizable at $AT1; the vault's g2State is still genesis"
say "Ethereum: during the window, exit 0's release is refused (its root is not in the vault) and finalize is refused"
erevert release-0-early $K6 UnknownExitRoot $VAULT "$RSIG" $SEAL $ROOT $NULLIFIER $ETH 0 $(x_dest 0) $ETH 0
erevert finalize-u1-early $K6 ContestWindowOpen $GATE "$GF" $VJ1
while [ "$(date +%s)" -le "$((AT1 + 1))" ]; do sleep 2; done; cast send $A6 --value 0 --private-key $K6 --rpc-url $ETH_RPC > /dev/null
say "Ethereum: after the window, $A6 (no role) finalizes update 1"
esend finalize-u1 $K6 $GATE "$GF" $VJ1
[ "$(ecall $VAULT 'g2State()(bytes32)' | lower)" = "$(echo $NEW1 | lower)" ] || die "g2State after update 1"
[ "$(ecall $VAULT 'exitRoot()(bytes32)' | lower)" = "$(echo $ROOT | lower)" ] || die "exit root after update 1"
B0=$(cast balance $(x_dest 0) --rpc-url $ETH_RPC)
esend release-0 $K6 $VAULT "$RSIG" $SEAL $ROOT $NULLIFIER $ETH 0 $(x_dest 0) $ETH 0
GOT=$(( $(cast balance $(x_dest 0) --rpc-url $ETH_RPC) - B0 )); [ "$GOT" = "$((D - WF))" ] || die "recipient got $GOT"
ok "exit 0 paid: recipient $(x_dest 0) +$GOT wei (0.01 ETH - withdraw fee $WF)"

# ------------------------------------------------------------------------------- 8. update 2, contest
say "LEZ: burn 0.01 wETH -> exit 1"
burn 1; sleep $((BLOCK_TIME * 2)); LAST2=$($HOST last --rpc $SEQ)
wait_final $LAST2; attest_all u2 $R/upd/u1/next-state.bin $LAST2
upd u2 $R/upd/u1/next-state.bin $LAST2 3 "$(cps u2 0 1 2)"
J2=$(cat $R/upd/u2/journal.hex); S2=$(cat $R/upd/u2/seal.hex); VJ2=$(cat $R/upd/u2/vault-journal.hex)
say "Ethereum: update 2 (k0, k1, k2 over block $LAST2) submitted by $A5"
esend submit-u2 $K5 $GATE "$GS" $J2 $S2
FAKE=$(cast keccak "seqsig: a block $LAST2 that is not the chain's")
conflict() { # $1 key number: it signs the checkpoint (update 1's tip -> LAST2) with the FAKE hash (TEST)
  $SEQSIG attest --key $R/keys/k$1.key --rpc $SEQ --dir $R/blocks --state $R/upd/u1/next-state.bin --cfg-id $CFG_ID --to $LAST2 --test-conflicting-hash $FAKE --record $R/keys/k$1.record --final-depth $DEPTH --out $R/cp/conflict-k$1.json; }
say "k3 did NOT sign update 2; it signs a conflicting hash for block $LAST2"
conflict 3
to $R/out/evidence-k3.txt heavy $SEQSIG evidence --checkpoints $R/cp/conflict-k3.json --set $R/set.json --out $R/upd/ev-k3
erevert contest-nonsigner $K6 NoEquivocation $GATE "$GC" $(cat $R/upd/ev-k3/journal.hex) $(cat $R/upd/ev-k3/seal.hex)
ok "a conflicting hash from a key that did not sign the update cancels nothing"
say "Ethereum: evidence for the SAME hash (k1's signature on the real block) is no conflict"
to $R/out/evidence-same.txt heavy $SEQSIG evidence --checkpoints $R/cp/u2-k1.json --set $R/set.json --out $R/upd/ev-same
erevert contest-same-hash $K6 NoConflict $GATE "$GC" $(cat $R/upd/ev-same/journal.hex) $(cat $R/upd/ev-same/seal.hex)
say "The attestor's record: k1 re-signing its update-2 checkpoint is fine; a record saying it signed another hash for that key makes it refuse"
$SEQSIG attest --key $R/keys/k1.key --rpc $SEQ --dir $R/blocks --state $R/upd/u1/next-state.bin --cfg-id $CFG_ID --to $LAST2 --record $R/keys/k1.record --final-depth $DEPTH --out $R/cp/u2-k1-again.json | tail -1
cp $R/keys/k1.record $R/keys/k1.record.copy
sed -i '' "\$s/ 0x[0-9a-f]*\$/ $FAKE/" $R/keys/k1.record.copy
if $SEQSIG attest --key $R/keys/k1.key --rpc $SEQ --dir $R/blocks --state $R/upd/u1/next-state.bin --cfg-id $CFG_ID --to $LAST2 --record $R/keys/k1.record.copy --final-depth $DEPTH --out $R/cp/u2-k1-refused.json > $R/out/record-refusal.txt 2>&1; then
  cat $R/out/record-refusal.txt; die "the attestor signed a second hash"; fi
grep -o "refusing to sign.*" $R/out/record-refusal.txt | sed -n 1p
ok "the attestor refuses to sign a second hash for a checkpoint key in its record"
say "k1 SIGNED update 2 and now signs a conflicting hash for the same (cfgId, previous tip, block $LAST2): equivocation"
conflict 1
to $R/out/evidence-k1.txt heavy $SEQSIG evidence --checkpoints $R/cp/conflict-k1.json --set $R/set.json --out $R/upd/ev-k1
esend contest-u2 $K6 $GATE "$GC" $(cat $R/upd/ev-k1/journal.hex) $(cat $R/upd/ev-k1/seal.hex)
[ "$(ecall $GATE 'finalizableAt()(uint256)' | cut -d' ' -f1)" = 0 ] || die "update 2 still pending"
[ "$(ecall $VAULT 'g2State()(bytes32)' | lower)" = "$(echo $NEW1 | lower)" ] || die "g2State moved"
[ "$(ecall $VAULT 'exitCount()(uint64)')" = 1 ] || die "exit count changed"
[ "$(ejected)" = "$((1 << $(idx 1)))" ] || die "k1 not ejected (mask $(ejected))"
ok "update 2 cancelled (g2State = update 1's, exit count 1); k1 (set index $(idx 1)) ejected: mask $(ejected), active signers $(ecall $GATE 'activeSigners()(uint256)')"
erevert resubmit-u2-counted-k1 $K5 BadSignerBitmap $GATE "$GS" $J2 $S2
erevert finalize-u2 $K6 NoPending $GATE "$GF" $VJ2
say "Executor: k1's signature no longer counts (k0, k1, k2 with the new mask): refused"
upd u2-with-k1 $R/upd/u1/next-state.bin $LAST2 3 "$(cps u2 0 1 2)" --expect-refusal
say "The same checkpoint signed by k0, k2, k3 with the new mask: the guest commits it, the gate refuses it (refuted)"
upd u2-again $R/upd/u1/next-state.bin $LAST2 3 "$(cps u2 0 2 3)"
erevert resubmit-u2-refuted $K5 Refuted $GATE "$GS" $(cat $R/upd/u2-again/journal.hex) $(cat $R/upd/u2-again/seal.hex)

# ------------------------------------------------------------------------------- 9. an honest update past it
sleep $((BLOCK_TIME * 2)); LAST3=$($HOST last --rpc $SEQ)
say "Update 3: from update 1's state over blocks to $LAST3 (contains exit 1), signed by k0, k2, k3 (k1 ejected)"
wait_final $LAST3; attest_all u3 $R/upd/u1/next-state.bin $LAST3
upd u3 $R/upd/u1/next-state.bin $LAST3 3 "$(cps u3 0 2 3)"
VJ3=$(cat $R/upd/u3/vault-journal.hex); NEW3=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["newState"])' $R/upd/u3/report.json)
esend submit-u3 $K5 $GATE "$GS" $(cat $R/upd/u3/journal.hex) $(cat $R/upd/u3/seal.hex)
AT3=$(ecall $GATE "finalizableAt()(uint256)" | cut -d' ' -f1)
while [ "$(date +%s)" -le "$((AT3 + 1))" ]; do sleep 2; done; cast send $A6 --value 0 --private-key $K6 --rpc-url $ETH_RPC > /dev/null
esend finalize-u3 $K6 $GATE "$GF" $VJ3
[ "$(ecall $VAULT 'g2State()(bytes32)' | lower)" = "$(echo $NEW3 | lower)" ] || die "g2State after update 3"
cast logs --from-block 0 --address $VAULT "StateUpdated(bytes32,bytes32,bytes32,uint64,bytes32[])" --rpc-url $ETH_RPC --json > $R/stateupdated.json
python3 - $R/stateupdated.json > $R/exit-leaves.txt <<'EOF'
import json, sys
for l in json.load(open(sys.argv[1])):
    d = bytes.fromhex(l["data"][2:]); off = int.from_bytes(d[128:160], "big"); n = int.from_bytes(d[off:off + 32], "big")
    for k in range(n): print("0x" + d[off + 32 + 32 * k: off + 64 + 32 * k].hex())
EOF
prove_release 1 $R/exit-leaves.txt 1; . $R/rel/1.env
B1=$(cast balance $(x_dest 1) --rpc-url $ETH_RPC)
esend release-1 $K6 $VAULT "$RSIG" $SEAL $ROOT $NULLIFIER $ETH 0 $(x_dest 1) $ETH 0
GOT=$(( $(cast balance $(x_dest 1) --rpc-url $ETH_RPC) - B1 )); [ "$GOT" = "$((D - WF))" ] || die "recipient 1 got $GOT"
ok "exit 1 paid after the honest update 3: +$GOT wei"

# ------------------------------------------------------------------------------- 10. two quorums: halt
sleep $((BLOCK_TIME * 2)); LAST4=$($HOST last --rpc $SEQ)
say "Update 4 (blocks $((LAST3+1))..$LAST4, k0, k2, k3) staged; then k0, k2, k3 also sign a conflicting hash for block $LAST4"
wait_final $LAST4; attest_all u4 $R/upd/u3/next-state.bin $LAST4
upd u4 $R/upd/u3/next-state.bin $LAST4 3 "$(cps u4 0 2 3)"
esend submit-u4 $K5 $GATE "$GS" $(cat $R/upd/u4/journal.hex) $(cat $R/upd/u4/seal.hex)
FAKE4=$(cast keccak "seqsig: a block $LAST4 that is not the chain's")
for i in 0 2 3; do $SEQSIG attest --key $R/keys/k$i.key --rpc $SEQ --dir $R/blocks --state $R/upd/u3/next-state.bin --cfg-id $CFG_ID --to $LAST4 --test-conflicting-hash $FAKE4 --record $R/keys/k$i.record --final-depth $DEPTH --out $R/cp/quorum-k$i.json; done
to $R/out/evidence-quorum.txt heavy $SEQSIG evidence --checkpoints $R/cp/quorum-k0.json,$R/cp/quorum-k2.json,$R/cp/quorum-k3.json --set $R/set.json --out $R/upd/ev-quorum
esend contest-quorum $K6 $GATE "$GC" $(cat $R/upd/ev-quorum/journal.hex) $(cat $R/upd/ev-quorum/seal.hex)
[ "$(ecall $GATE 'halted()(bool)')" = true ] || die "gate not halted"
erevert submit-after-halt $K5 Halted $GATE "$GS" $(cat $R/upd/u4/journal.hex) $(cat $R/upd/u4/seal.hex)
erevert finalize-after-halt $K6 Halted $GATE "$GF" $(cat $R/upd/u4/vault-journal.hex)
ok "two conflicting quorums: the gate halted for good (dead = $(ecall $GATE 'dead()(bool)')); the vault keeps update 3's state"

# ------------------------------------------------------------------------------- 11. conservation and summary
say "Conservation: vault counters"
read -r dep cl bu rel <<< "$(ecall $VAULT "counters(address,uint256)(uint64,uint64,uint64,uint64)" $ETH 0 | tr '\n' ' ')"
echo "ETH d0: deposits $dep claims $cl burns $bu releases $rel; claimedValue $(ecall $VAULT 'claimedValue(address)(uint256)' $ETH | cut -d' ' -f1) burnedValue $(ecall $VAULT 'burnedValue(address)(uint256)' $ETH | cut -d' ' -f1)"
[ "$rel" -le "$bu" ] && [ "$bu" -le "$cl" ] && [ "$cl" -le "$dep" ] || die "conservation"
[ "$dep $cl $bu $rel" = "3 3 2 2" ] || die "counters $dep $cl $bu $rel, want 3 3 2 2"
ok "released 2 <= burned 2 <= claimed 3 <= deposited 3"
say "Cycles (executor, user cycles)"
python3 - $R/upd <<'EOF'
import json, os, sys
d = sys.argv[1]
for n in ["u1", "u1-4of4", "u2", "u2-again", "u3", "u4"]:
    r = json.load(open(os.path.join(d, n, "report.json")))
    print(f"{n}: blocks {r['fromBlock']}..{r['toBlock']}, signatures {r['signaturesGiven']} ({r['distinctSigners']} distinct), "
          f"seq-sig guest {r['seqsigUserCycles']}, TrustedTip guest {r['trustedTipUserCycles']}, difference {r['seqsigUserCycles'] - r['trustedTipUserCycles']}, "
          f"inside the signature check {r['signatureCheckCycles']}, signer bitmap {r['signerBitmap']:#06b}, ejected mask {r['ejectedMask']:#06b}")
for n in ["ev-k3", "ev-same", "ev-k1", "ev-quorum"]:
    r = json.load(open(os.path.join(d, n, "report.json")))
    print(f"evidence {n}: {r['signatures']} signature(s), bitmap {r['signerBitmap']:#06b}, {r['userCycles']} user cycles, {r['signatureCheckCycles']} in the check")
for n in ["u1-2of4", "u1-dup", "u1-outsider", "u1-t2", "u2-with-k1"]:
    r = json.load(open(os.path.join(d, n, "report.json")))
    print(f"{n}: refused by the guest: {r['guestError'].splitlines()[0][:160]}")
EOF
say "Transactions"; column -t -s $'\t' $TXS
cat > $R/ids.env <<EOF
VAULT=$VAULT
GATE=$GATE
CFG_ID=$CFG_ID
BP=$BP
TP=$TP
UPDATE_IMAGE_ID=$UPDATE_IMAGE_ID
EVIDENCE_IMAGE_ID=$EVIDENCE_IMAGE_ID
RELEASE_IMAGE_ID=$RELEASE_IMAGE_ID
SET_HASH=$SET_HASH
START=$START
LAST1=$LAST1
LAST2=$LAST2
LAST3=$LAST3
LAST4=$LAST4
EJECTED=$(ejected)
EOF
echo "ALL STEPS PASSED  [$LABEL]"
