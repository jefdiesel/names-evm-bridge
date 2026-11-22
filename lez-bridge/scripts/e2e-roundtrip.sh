#!/bin/bash
# Since the 2026-11-14 security fixes: the bridge is initialised by its own header key (in this
# wallet since `deploy`) with `init --payer`; the vault is built with lezChainId = this chain's
# channel id (`chain-id`) and zoneId = the stand-in root-source id (`source-id`), which the bridge
# derives at init. Runs made before the fixes (roundtrip1, roundtrip2) used zero ids and `init --admin`.
# FIRST COMPLETE ROUND TRIP on local chains, public-account mode:
#   deposit on Ethereum -> claim on LEZ -> burn on LEZ -> G2-lite update on Ethereum -> release.
#
#   mock-verified: proofs executed, not proved; LEZ finality trusted for the test.
#
# Ethereum: the shared anvil (127.0.0.1:8550). LEZ: the shared sequencer seq2 (127.0.0.1:3250).
# The vault's verifier is the TEST MockVerifier (a seal is imageId | sha256(journal)); its
# updateImageId is the G2-lite TrustedTip guest and releaseImageId the G3 guest, both real image
# ids from outbound_host. Every G2 journal and every G3 journal below comes out of RISC Zero's
# executor (and equals the native run); nothing is proved. The G2 TrustedTip guest accepts the
# last LEZ block of each batch as final because the host says so.
#
# No anvil time warp: the vault's release delay is RELEASE_DELAY seconds (default 20) and the
# script waits in real time. Its own anvil account (#3), fresh recipient/relayer addresses, its
# own LEZ public account P funded once from the shared genesis account F.
#
#   R=run/roundtrip1 scripts/e2e-roundtrip.sh     (fresh R)
#   RESUME=1 R=... scripts/e2e-roundtrip.sh                                     (continue)
#   STOP_AFTER=<step> ...   stop after that step (wallet, programs, genesis, vault, setup, deposits,
#                           claims, burns1, update1, burns2, update2, submit1, releases1, submit2,
#                           releases2); RESUME=1 continues. scripts/judge-demo.sh runs it in phases.
# LEZ rc3: the wallet refuses to send a transaction its simulation reverts, so the repeat claim and
# the odd burn below pass --allow-revert to be included and reverted, as on rc1.
set -euo pipefail
: "${R:?set R to a fresh run directory}"
HERE=$(cd "$(dirname "$0")/.." && pwd)
VAULT_DIR=${VAULT_DIR:-$HERE/../vault}
TGT=${TGT:-$HERE/outbound/target}
HOST=${HOST:-$TGT/release/outbound_host}
LOCK=${LOCK:-env}   # a command prefix for the executor runs, e.g. a build lock
export TMPDIR=${TMPDIR_OUTBOUND:-${TMPDIR:-/tmp}}
# executor runs go through the test machine's shared heavy-job lock, niced
host() { MIN_FREE=${MIN_FREE:-15} $LOCK nice -n 19 $HOST "$@"; }
CLI_SRC=${CLI_SRC:-$HERE/target/release/lez-bridge}
SEQ=${SEQ:-http://127.0.0.1:3250}
ETH_RPC=${ETH_RPC:-http://127.0.0.1:8550}
RELEASE_DELAY=${RELEASE_DELAY:-20}
LABEL="mock-verified: proofs executed, not proved; LEZ finality trusted for the test"
export PATH=$HOME/.foundry/bin:$PATH
export LEE_WALLET_HOME_DIR=$R/wallet LEZ_BRIDGE_SEQUENCER=$SEQ RISC0_PROVER=local RAYON_NUM_THREADS=4
# anvil default account #3 (not #0, which other local runs use)
EK=0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6
LOCAL_TEST_KEY=$("$HERE/scripts/lez-test-key.sh") || exit 1   # LEZ published genesis test key, read from LEZ
ZERO32=0x0000000000000000000000000000000000000000000000000000000000000000
ETH=0x0000000000000000000000000000000000000000
USDC_D=(100000000 1000000000); ETH_D=(10000000000000000 100000000000000000)
USDC_DF=(1000000 10000000); ETH_DF=(100000000000000 1000000000000000)
USDC_WF=(500000 5000000); ETH_WF=(50000000000000 500000000000000)

if [ -d "$R" ] && [ "${RESUME:-0}" != 1 ]; then echo "FAILED: $R exists (fresh run dir, or RESUME=1)" >&2; exit 1; fi
mkdir -p "$R/bin" "$R/blocks" "$R/rel" "$R/out"
exec > >(tee -a "$R/roundtrip.log") 2>&1
say() { echo; echo "## $(date +%T) $*"; }
die() { echo "FAILED: $*  [$LABEL]" >&2; exit 1; }
# set -e stops at the first failing command; say which one, so a stop is never silent.
set -E
trap 'echo "FAILED: line $LINENO: $BASH_COMMAND (exit status $?)  [$LABEL]" >&2' ERR
# Run a command with its output in a file, then show the file: a slow, closed or early-exiting
# reader of the log can never make the CLI or the host fail half-way (Rust panics on a closed stdout).
to() { local f=$1; shift; local rc=0; "$@" > "$f" 2>&1 || rc=$?; cat "$f"
  # names the program and subcommand only (later arguments can be secrets)
  [ "$rc" = 0 ] || echo "FAILED: exit status $rc from ${*:1:2} (its output: $f)  [$LABEL]" >&2; return "$rc"; }
ok() { echo "OK: $*  [$LABEL]"; }
lower() { tr 'A-F' 'a-f'; }
touch "$R/ids.env" "$R/steps.done"; . "$R/ids.env"
save() { echo "$1=${!1}" >> "$R/ids.env"; }
done_step() { grep -qx "$1" "$R/steps.done"; }
mark() { echo "$1" >> "$R/steps.done"; if [ "${STOP_AFTER:-}" = "$1" ]; then echo "stopped after step $1 (STOP_AFTER)"; exit 0; fi; }
[ -x "$R/bin/lez-bridge" ] || { cp "$CLI_SRC" "$R/bin/lez-bridge"; shasum -a 256 "$R/bin/lez-bridge" > "$R/bin/lez-bridge.sha256"; }
T=$R/bin/lez-bridge
id_of() { grep -oE "$1/[1-9A-HJ-NP-Za-km-z]{32,44}" | sed -n 1p | cut -d/ -f2; }
block_of() { sed -nE 's/.*(applied|included) in block ([0-9]+).*/\2/p' | tail -1; }
ecall() { cast call "$@" --rpc-url $ETH_RPC; }
esend() { cast send "$@" --private-key $EK --rpc-url $ETH_RPC --json; }
# esend expecting a revert with `want` (a custom error name); prints the error line
erevert() { local want=$1; shift; local out; if out=$(cast send "$@" --private-key $EK --rpc-url $ETH_RPC 2>&1); then die "expected $want, the transaction went through"; fi
  local sel; sel=$(cast sig "$want()"); grep -qi -- "${sel#0x}\|$want" <<<"$out" || die "expected $want ($sel), got: $(echo "$out" | tail -3)"; echo "refused as expected: $want"; }
EA=$(cast wallet address --private-key $EK)
echo "$LABEL"; echo "run $R, anvil account $EA, CLI $(cut -c1-16 "$R/bin/lez-bridge.sha256")…, host $HOST"

# ------------------------------------------------------------------------------- 1. LEZ account
if ! done_step wallet; then
  say "LEZ: wallet, the genesis account F, and our own public account P funded from it"
  to $R/out/check-health.txt $T wallet check-health
  to $R/out/import-f.txt $T wallet account import public --private-key $LOCAL_TEST_KEY; F=$(id_of Public < $R/out/import-f.txt); save F
  to $R/out/new-p.txt $T wallet account new public; P=$(id_of Public < $R/out/new-p.txt); save P
  to $R/out/fund-p.txt $T wallet auth-transfer send --from Public/$F --to Public/$P --amount ${P_FUNDING:-200000000000}
  mark wallet
fi
echo "F=$F P=$P"

# ------------------------------------------------------------------------------- 2. programs, G2 genesis
if ! done_step programs; then
  START=$($HOST last --rpc $SEQ); save START
  say "LEZ: G2 starts at block $START (before our programs exist); deploy the token and bridge programs"
  to $R/out/deploy-token.txt $T deploy --which token --payer $P; TP=$(sed -n 's/^program //p' $R/out/deploy-token.txt); save TP
  to $R/out/deploy-bridge.txt $T deploy --which bridge --payer $P; BP=$(sed -n 's/^program //p' $R/out/deploy-bridge.txt); save BP
  mark programs
fi
PROGRAM_ID=$($T program-id --bridge $BP | sed -n 's/^PROGRAM_ID=//p')
# The cfgId's LEZ chain id (this chain's channel id) and zone id (the stand-in root-source id).
LEZ_CHAIN_ID=$($T chain-id | sed -n 's/^LEZ_CHAIN_ID=//p'); ZONE_ID=$($T source-id | sed -n 's/^ZONE_ID=//p')
[ -n "$LEZ_CHAIN_ID" ] && [ -n "$ZONE_ID" ] || die "no chain id / source id"
if ! done_step genesis; then
  say "G2 genesis state at block $START for bridge $BP"
  to $R/genesis.txt $HOST genesis --rpc $SEQ --dir $R/blocks --block $START --bridge $BP --out $R/state0.bin
  G2_STATE0=$(sed -n 's/^G2_STATE=//p' $R/genesis.txt); save G2_STATE0
  to $R/image-ids.txt $HOST ids
  UPDATE_IMAGE_ID=$(sed -n 's/^UPDATE_IMAGE_ID_TRUSTED_TIP=//p' $R/image-ids.txt); save UPDATE_IMAGE_ID
  RELEASE_IMAGE_ID=$(sed -n 's/^RELEASE_IMAGE_ID=//p' $R/image-ids.txt); save RELEASE_IMAGE_ID
  mark genesis
fi

# ------------------------------------------------------------------------------- 3. vault
if ! done_step vault; then
  say "Ethereum: deploy the round-trip vault (MockVerifier, updateImageId = G2-lite TrustedTip $UPDATE_IMAGE_ID, releaseImageId = G3 $RELEASE_IMAGE_ID)"
  out=$(cd "$VAULT_DIR" && PROGRAM_ID=$PROGRAM_ID LEZ_CHAIN_ID=$LEZ_CHAIN_ID ZONE_ID=$ZONE_ID UPDATE_IMAGE_ID=$UPDATE_IMAGE_ID RELEASE_IMAGE_ID=$RELEASE_IMAGE_ID \
    G2_STATE=$G2_STATE0 RELEASE_DELAY=$RELEASE_DELAY forge script script/RoundTripDeploy.s.sol \
    --rpc-url $ETH_RPC --private-key $EK --broadcast 2>&1) || { echo "$out"; die "vault deploy"; }
  VAULT=$(echo "$out" | sed -n 's/.*VAULT=\(0x[0-9a-fA-F]*\).*/\1/p' | sed -n 1p); save VAULT
  USDC=$(echo "$out" | sed -n 's/.*USDC=\(0x[0-9a-fA-F]*\).*/\1/p' | sed -n 1p); save USDC
  CFG_ID=$(echo "$out" | sed -n 's/.*CFG_ID=\(0x[0-9a-fA-F]*\).*/\1/p' | sed -n 1p); save CFG_ID
  [ -n "$VAULT" ] && [ -n "$USDC" ] && [ -n "$CFG_ID" ] || { echo "$out"; die "deploy printed no VAULT/USDC/CFG_ID"; }
  mark vault
fi
EVM_CHAIN_ID=$(cast chain-id --rpc-url $ETH_RPC)
DERIVED=$($T cfg-id --vault $VAULT --chain-id $EVM_CHAIN_ID --lez-chain-id $LEZ_CHAIN_ID --zone-id $ZONE_ID --bridge $BP | sed -n 's/^CFG_ID=//p')
[ "$(echo $DERIVED | lower)" = "$(echo $CFG_ID | lower)" ] || die "cfg_id: vault $CFG_ID, bridge would derive $DERIVED"
done_step submit1 || [ "$(ecall $VAULT 'g2State()(bytes32)' | lower)" = "$(echo $G2_STATE0 | lower)" ] || die "vault g2State is not the G2 genesis state"
[ "$(ecall $VAULT 'updateImageId()(bytes32)' | lower)" = "$(echo $UPDATE_IMAGE_ID | lower)" ] || die "updateImageId"
echo "vault $VAULT usdc $USDC cfg $CFG_ID; g2State = genesis $G2_STATE0; updateImageId = TrustedTip guest"

# ------------------------------------------------------------------------------- 4. LEZ setup
if ! done_step setup; then
  say "LEZ: wrapped USDC and ETH (mint authority = the bridge), init (stand-in attestor P), register both"
  to $R/out/wrapped-usdc.txt $T create-wrapped --token-program $TP --bridge $BP --name "Wrapped USDC rt1" --payer $P; DEF_USDC=$(sed -n 's/^token //p' $R/out/wrapped-usdc.txt); save DEF_USDC
  to $R/out/wrapped-eth.txt $T create-wrapped --token-program $TP --bridge $BP --name "Wrapped ETH rt1" --payer $P; DEF_ETH=$(sed -n 's/^token //p' $R/out/wrapped-eth.txt); save DEF_ETH
  # Signed by the bridge's header key (this wallet holds it since `deploy`), paid by P.
  to $R/out/init.txt $T init --bridge $BP --token-program $TP --admin $P --vault $VAULT --chain-id $EVM_CHAIN_ID \
    --freezer $P --attestor $P --payer $P
  to $R/out/register-usdc.txt $T register --bridge $BP --admin $P --eth-token $USDC --definition $DEF_USDC --decimals 6 --denoms ${USDC_D[0]},${USDC_D[1]}
  to $R/out/register-eth.txt $T register --bridge $BP --admin $P --eth-token $ETH --definition $DEF_ETH --decimals 18 --denoms ${ETH_D[0]},${ETH_D[1]}
  mark setup
fi
ONCHAIN=$($T show --bridge $BP | sed -n 's/^cfg_id //p')
[ "$(echo $ONCHAIN | lower)" = "$(echo $CFG_ID | lower)" ] || die "bridge config cfg_id $ONCHAIN != vault $CFG_ID"

# ------------------------------------------------------------------------------- 5. deposits
# (token, denomIndex) of deposits 1..6: USDC 100, 100, 1000; ETH 0.01, 0.01, 0.1
DEP_TOK=($USDC $USDC $USDC $ETH $ETH $ETH); DEP_DI=(0 0 1 0 0 1)
denom() { if [ "$(echo $1 | lower)" = "$ETH" ]; then echo ${ETH_D[$2]}; else echo ${USDC_D[$2]}; fi; }
dfee() { if [ "$(echo $1 | lower)" = "$ETH" ]; then echo ${ETH_DF[$2]}; else echo ${USDC_DF[$2]}; fi; }
wfee() { if [ "$(echo $1 | lower)" = "$ETH" ]; then echo ${ETH_WF[$2]}; else echo ${USDC_WF[$2]}; fi; }
dep_secret() { cast keccak "rt1-deposit-$1-$P"; }
if ! done_step deposits; then
  say "Ethereum: 6 deposits for P (USDC 100, 100, 1000; ETH 0.01, 0.01, 0.1)"
  : > $R/deposit-index.txt
  for i in 0 1 2 3 4 5; do
    tok=${DEP_TOK[$i]}; di=${DEP_DI[$i]}; d=$(denom $tok $di)
    C=$($T commitment --secret $(dep_secret $i) --dest $P --eth-token $tok --denom $d --cfg-id $CFG_ID)
    if [ "$tok" = "$ETH" ]; then
      esend $VAULT "depositETH(uint32,bytes32)" $di $C --value $((d + $(dfee $tok $di))) > $R/dep-$i.json
    else
      esend $VAULT "deposit(address,uint32,bytes32)" $tok $di $C > $R/dep-$i.json
    fi
    echo "deposit $i: token $tok denom $d C $C tx $(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["transactionHash"])' $R/dep-$i.json)"
  done
  mark deposits
fi
# the vault's leaves, rebuilt from its own Deposit events
cast logs --from-block 0 --address $VAULT "Deposit(address indexed,uint32,bytes32,uint64,bytes32)" --rpc-url $ETH_RPC --json > $R/deposit-logs.json
python3 - "$R/deposit-logs.json" > $R/deposits.txt <<'EOF'
import json, sys
for l in json.load(open(sys.argv[1])):
    d = l["data"][2:]
    print("0x" + l["topics"][1][-40:], int(d[0:64], 16), "0x" + d[64:128], int(d[128:192], 16))
EOF
: > $R/leaves.txt
while read -r tok di c idx; do
  ecall $VAULT "depositLeaf(address,uint256,bytes32)(bytes32)" $tok $(denom $tok $di) $c >> $R/leaves.txt
done < $R/deposits.txt
ROOT=$(ecall $VAULT "depositRoot()(bytes32)")
echo "$(wc -l < $R/leaves.txt) deposit leaves; vault root $ROOT"

# ------------------------------------------------------------------------------- 6. claims
if ! done_step claims; then
  say "LEZ: record the vault root (stand-in attestor P), claim all 6 deposits into P (public)"
  to $R/out/attest.txt $T attest --bridge $BP --attestor $P --root $ROOT
  for i in 0 1 2 3 4 5; do
    tok=${DEP_TOK[$i]}; di=${DEP_DI[$i]}
    to $R/claim-$i.txt $T claim --bridge $BP --token-program $TP --from $P --eth-token $tok --denom-index $di --denom $(denom $tok $di) \
      --secret $(dep_secret $i) --cfg-id $CFG_ID --leaves $R/leaves.txt --index $i
  done
  say "LEZ: claim deposit 0 again with the local pre-check off: included and reverted (G2 must skip it)"
  if $T --no-precheck --allow-revert claim --bridge $BP --token-program $TP --from $P --eth-token $USDC --denom-index 0 --denom ${USDC_D[0]} \
      --secret $(dep_secret 0) --cfg-id $CFG_ID --leaves $R/leaves.txt --index 0 > $R/claim-repeat.txt 2>&1; then
    cat $R/claim-repeat.txt; die "the repeat claim applied"
  fi
  cat $R/claim-repeat.txt
  REPEAT_BLOCK=$(block_of < $R/claim-repeat.txt); save REPEAT_BLOCK
  mark claims
fi
to $R/show-after-claims.txt $T show --bridge $BP

# ------------------------------------------------------------------------------- 7. burns
# exits 1..7: (definition, eth token, denomIndex, recipient tag, maxRelayFee)
X_TOK=($USDC $USDC $ETH $ETH $ETH $USDC $ETH); X_DI=(0 1 0 0 0 0 0)
X_MAX=(1000000 10000000 100000000000000 100000000000000 100000000000000 1000000 100000000000000)
x_secret() { cast keccak "rt1-exit-$1-$P"; }
x_dest() { cast keccak "rt1-recipient-$1" | cut -c1-42; }
RELAYER=$(cast keccak "rt1-relayer" | cut -c1-42)
x_def() { if [ "${X_TOK[$1]}" = "$ETH" ]; then echo $DEF_ETH; else echo $DEF_USDC; fi; }
burn() { # burn exit $1 (skipped if already applied, so a resumed run never burns twice)
  local j=$1 tok=${X_TOK[$1]} di=${X_DI[$1]}; local d; d=$(denom $tok $di)
  if [ -f $R/burn-$j.txt ] && grep -q "applied in block" $R/burn-$j.txt; then echo "exit $j already burned"; return; fi
  { grep -v "^$j " $R/exit-commitments.txt || true; } > $R/exit-commitments.tmp; mv $R/exit-commitments.tmp $R/exit-commitments.txt
  local E; E=$($T exit-commitment --secret $(x_secret $j) --eth-dest $(x_dest $j) --eth-token $tok --denom $d \
    --max-relay-fee ${X_MAX[$j]} --cfg-id $CFG_ID | sed -n 1p)
  echo "$j $E" >> $R/exit-commitments.txt
  to $R/burn-$j.txt $T burn --bridge $BP --token-program $TP --from $P --definition $(x_def $j) --amount $d --exit $E
}
if ! done_step burns1; then
  say "LEZ: burns, batch 1: USDC 100 -> R0, USDC 1000 -> R1, ETH 0.01 x3 -> R2, R3, R4 (three 0.01 against two 0.01 claims: by value), then an odd burn of USDC 50 (reverted)"
  : > $R/exit-commitments.txt
  for j in 0 1 2 3 4; do burn $j; done
  if $T --allow-revert burn --bridge $BP --token-program $TP --from $P --definition $DEF_USDC --amount 50000000 --exit 0x$(printf 'ab%.0s' {1..32}) > $R/burn-odd.txt 2>&1; then
    cat $R/burn-odd.txt; die "the odd burn applied"
  fi
  cat $R/burn-odd.txt
  LAST1=$(block_of < $R/burn-odd.txt); save LAST1
  mark burns1
fi
echo "batch 1 ends at LEZ block $LAST1 (the reverted odd burn)"

# ------------------------------------------------------------------------------- 8. G2 update 1
if ! done_step update1; then
  say "G2-lite update 1: LEZ blocks $((START+1))..$LAST1, native run and the TrustedTip guest in the executor"
  to $R/update1.txt host update --rpc $SEQ --dir $R/blocks --state $R/state0.bin --to $LAST1 --out $R/update1
  mark update1
fi

# ------------------------------------------------------------------------------- 9. burns 2, update 2
if ! done_step burns2; then
  say "LEZ: burns, batch 2: USDC 100 -> R5, ETH 0.01 -> R6"
  for j in 5 6; do burn $j; done
  LAST2=$(block_of < $R/burn-6.txt); save LAST2
  mark burns2
fi
if ! done_step update2; then
  say "G2-lite update 2: LEZ blocks $((LAST1+1))..$LAST2, from update 1's state (state chaining)"
  to $R/update2.txt host update --rpc $SEQ --dir $R/blocks --state $R/update1/next-state.bin --to $LAST2 --out $R/update2
  mark update2
fi
to $R/show-after-burns.txt $T show --bridge $BP
J1=$(cat $R/update1/journal.hex); S1=$(cat $R/update1/seal.hex)
J2=$(cat $R/update2/journal.hex); S2=$(cat $R/update2/seal.hex)

# ------------------------------------------------------------------------------- 10. submit updates
if ! done_step submit1; then
  say "Ethereum: update 2 before update 1 is refused (reordered: its prevState is update 1's newState)"
  erevert StaleState $VAULT "update(bytes,bytes)" $J2 $S2
  say "Ethereum: a seal for another journal is refused"
  erevert BadSeal $VAULT "update(bytes,bytes)" $J1 $S2
  say "Ethereum: update 1"
  esend $VAULT "update(bytes,bytes)" $J1 $S1 > $R/update1-tx.json
  echo "update 1 tx $(python3 -c 'import json,sys;j=json.load(open(sys.argv[1]));print(j["transactionHash"],"gas",int(j["gasUsed"],16))' $R/update1-tx.json)"
  say "Ethereum: update 1 again is refused (stale)"
  erevert StaleState $VAULT "update(bytes,bytes)" $J1 $S1
  mark submit1
fi
NEW1=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["newState"])' $R/update1/report.json)
[ "$(ecall $VAULT 'g2State()(bytes32)' | lower)" = "$(echo $NEW1 | lower)" ] || [ -n "$(grep -x submit2 $R/steps.done)" ] || die "g2State after update 1"

# the vault's counters must equal the bridge view G2 carried to LAST1 (update1/next-state.json)
check_counters() { # $1: a JSON file with the bridge's tokens (G2 view summary or parsed `show`)
  python3 - "$1" "$VAULT" "$ETH_RPC" <<'EOF'
import json, subprocess, sys
src, vault, rpc = sys.argv[1:]
tokens = json.load(open(src))["tokens"]
def call(sig, *a):
    return subprocess.check_output(["cast", "call", vault, sig, *a, "--rpc-url", rpc], text=True).split()
bad = 0
for t in tokens:
    tok = t["ethToken"]
    for d in range(len(t["denoms"])):
        out = call("counters(address,uint256)(uint64,uint64,uint64,uint64)", tok, str(d))
        dep, cl, bu, rel = (int(x) for x in out[0:4])
        print(f"  {tok} d{d}: vault deposits {dep} claims {cl} burns {bu} releases {rel} | bridge claims {t['claims'][d]} burns {t['burns'][d]}")
        bad += (cl != t["claims"][d]) + (bu != t["burns"][d])
    cv = int(call("claimedValue(address)(uint256)", tok)[0]); bv = int(call("burnedValue(address)(uint256)", tok)[0])
    print(f"  {tok}: vault claimedValue {cv} burnedValue {bv} | bridge {t['claimedValue']} {t['burnedValue']}")
    bad += (cv != int(t["claimedValue"])) + (bv != int(t["burnedValue"]))
sys.exit(1 if bad else 0)
EOF
}
show_json() { # the live bridge config (`show`) as the same JSON shape
  to $R/out/show-solvency.txt $T show --bridge $BP > /dev/null
  python3 -c '
import json, re, sys
toks = []
for l in sys.stdin:
    m = re.match(r"token 0x(\w+) def \S+ denoms \[([^\]]*)\] claims \[([^\]]*)\] burns \[([^\]]*)\] claimed_value (\d+) burned_value (\d+)", l)
    if m:
        ints = lambda s: [int(x) for x in s.split(",") if x.strip()]
        toks.append({"ethToken": "0x" + m[1], "denoms": ints(m[2]), "claims": ints(m[3]), "burns": ints(m[4]),
                     "claimedValue": m[5], "burnedValue": m[6]})
print(json.dumps({"tokens": toks}))' < $R/out/show-solvency.txt
}
if ! grep -qx submit2 $R/steps.done; then
  say "check: vault counters after update 1 = the bridge as of LEZ block $LAST1"
  python3 -c 'import json,sys;print(json.dumps(json.load(open(sys.argv[1]))))' $R/update1/next-state.json > $R/view1.json
  check_counters $R/view1.json || die "vault counters differ from the bridge's after update 1"
  ok "vault counters = bridge counters at LAST1"
fi

# ------------------------------------------------------------------------------- 11. releases
leaves_from_events() {
  cast logs --from-block 0 --address $VAULT "StateUpdated(bytes32,bytes32,bytes32,uint64,bytes32[])" --rpc-url $ETH_RPC --json > $R/stateupdated-logs.json
  python3 - $R/stateupdated-logs.json > $R/exit-leaves.txt <<'EOF'
import json, sys
# data = abi.encode(prevState, newState, exitRoot, firstExitIndex, exitLeaves[])
for l in json.load(open(sys.argv[1])):
    d = bytes.fromhex(l["data"][2:])
    w = lambda i: d[32 * i:32 * i + 32]
    first = int.from_bytes(w(3), "big"); off = int.from_bytes(w(4), "big"); n = int.from_bytes(d[off:off + 32], "big")
    for k in range(n):
        print("0x" + d[off + 32 + 32 * k: off + 64 + 32 * k].hex())
EOF
}
# release exit $1 with relayer $2 and fee $3; writes rel/<j>.env (ROOT, NULLIFIER, JOURNAL, SEAL)
prove_release() {
  local j=$1 relayer=$2 fee=$3 tok=${X_TOK[$1]} di=${X_DI[$1]}
  local idx; idx=$(python3 - $j $R/exit-commitments.txt $R/exit-leaves.txt "$CFG_ID" "$tok" "$(denom $tok $di)" <<'EOF'
import hashlib, sys
j, ecf, lf, cfg, tok, d = sys.argv[1:]
E = dict(l.split() for l in open(ecf))[j]
tag = bytes.fromhex("22018742ad6028fda6cb0726afaae90ea20dff3cef6b1ec5b3c14f856d76c59c")
leaf = "0x" + hashlib.sha256(tag + bytes.fromhex(cfg[2:]) + bytes.fromhex(tok[2:]) + int(d).to_bytes(32, "big") + bytes.fromhex(E[2:])).hexdigest()
leaves = [l.strip() for l in open(lf) if l.strip()]
print(leaves.index(leaf))
EOF
)
  to $R/rel/$j.txt host release --secret $(x_secret $j) --eth-dest $(x_dest $j) --token $tok --denom $(denom $tok $di) \
    --max-relay-fee ${X_MAX[$j]} --cfg-id $CFG_ID --leaves $R/exit-leaves.txt --index $idx --relayer $relayer --relay-fee $fee
  grep -E '^(ROOT|NULLIFIER|JOURNAL|SEAL|USER_CYCLES)=' $R/rel/$j.txt > $R/rel/$j.env
  echo "LEAF_INDEX=$idx" >> $R/rel/$j.env
}
release_args() { # $1 exit, $2 relayer, $3 fee, [recipient override]
  local j=$1 tok=${X_TOK[$1]}; . $R/rel/$j.env
  echo "$SEAL $ROOT $NULLIFIER $tok ${X_DI[$j]} ${4:-$(x_dest $j)} $2 $3"
}
SIG="release(bytes,bytes32,bytes32,address,uint32,address,address,uint256)"
# Wait in real time (no anvil warp), then mine one ordinary block (a 0-value self-transfer from our
# own account) so the next estimate runs at a timestamp past the delay.
wait_until() { while [ "$(date +%s)" -le "$(($1 + 1))" ]; do sleep 2; done; esend $EA --value 0 > /dev/null; }
bal() { if [ "$1" = "$ETH" ]; then cast balance $2 --rpc-url $ETH_RPC; else ecall $1 "balanceOf(address)(uint256)" $2 | cut -d' ' -f1; fi; }
[ -f $R/anonymity.txt ] || : > $R/anonymity.txt
anon() { # anonymity set of exit $1 just before its release, under the newest root
  python3 - $1 "${X_TOK[$1]}" "${X_DI[$1]}" $R/released.txt $R/update1/report.json $R/update2/report.json $R/exit-commitments.txt "$(grep -c submit2 $R/steps.done || true)" <<'EOF' | tee -a $R/anonymity.txt
import json, sys
j, tok, di, relf, r1, r2, ecf, sub2 = sys.argv[1:]
exits = json.load(open(r1))["exits"] + (json.load(open(r2))["exits"] if sub2 == "1" else [])
same = [e for e in exits if e["token"].lower() == tok.lower() and e["denomIndex"] == int(di)]
released = [l.split()[0] for l in open(relf)] if relf and __import__("os").path.exists(relf) else []
E = dict(l.split() for l in open(ecf))
rel_same = [x for x in released if any(E[x] == e["commitment"] for e in same)]
print(f"exit {j} ({tok} d{di}): candidates under the newest root {len(same)}, of which unreleased {len(same) - len(rel_same)}")
EOF
}
do_release() { # $1 exit, $2 relayer, $3 fee (skipped if already released, so a resumed run never resends)
  local j=$1 tok=${X_TOK[$1]}; local dest; dest=$(x_dest $j)
  if grep -q "^$j " $R/released.txt 2>/dev/null; then echo "exit $j already released"; return; fi
  local before_d before_r; before_d=$(bal $tok $dest); before_r=$(bal $tok $2)
  anon $j
  esend $VAULT "$SIG" $(release_args $j $2 $3) > $R/rel/$j-tx.json
  local h; h=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["transactionHash"])' $R/rel/$j-tx.json)
  echo "$j $h" >> $R/released.txt
  local d w; d=$(denom $tok ${X_DI[$j]}); w=$(wfee $tok ${X_DI[$j]})
  local got_d got_r; got_d=$(( $(bal $tok $dest) - before_d )); got_r=$(( $(bal $tok $2) - before_r ))
  [ "$got_d" = "$((d - w - $3))" ] || die "exit $j: recipient got $got_d, want $((d - w - $3))"
  if [ "$3" != 0 ]; then [ "$got_r" = "$3" ] || die "exit $j: relayer got $got_r, want $3"; fi
  . $R/rel/$j.env
  [ "$(ecall $VAULT 'releaseNullifierSpent(bytes32)(bool)' $NULLIFIER)" = true ] || die "nullifier of exit $j not spent"
  local rmsg="no relay fee"; [ "$3" != 0 ] && rmsg="relayer $2 +$got_r (relay fee $3, on the menu)"
  ok "exit $j released (tx $h): recipient $dest +$got_d ($d - withdraw fee $w - relay fee $3), $rmsg, nullifier $NULLIFIER spent"
}
if ! done_step releases1; then
  leaves_from_events
  echo "$(wc -l < $R/exit-leaves.txt) exit leaves; exit root $(ecall $VAULT 'exitRoot()(bytes32)')"
  [ -f $R/released.txt ] || : > $R/released.txt
  say "G3: release proofs for exits 0..3 in the executor (root = the vault's newest exit root)"
  prove_release 0 $RELAYER 500000
  . $R/rel/0.env
  AT=$(ecall $VAULT "releasableAt(address,bytes32)(uint256)" $USDC $ROOT | cut -d' ' -f1); NOW=$(date +%s)
  if [ "$NOW" -lt "$AT" ]; then
    say "Ethereum: a release before the delay is refused (releasable at $AT, now $NOW)"
    erevert ReleaseTooEarly $VAULT "$SIG" $(release_args 0 $RELAYER 500000)
  else echo "NOTE: the delay had already passed ($NOW >= $AT); ReleaseTooEarly not exercised"; fi
  prove_release 1 $RELAYER 5000000
  prove_release 2 $ETH 0
  prove_release 3 $RELAYER 100000000000000
  wait_until $AT
  say "Ethereum: releases (anyone submits; funds go where the proof says)"
  do_release 0 $RELAYER 500000
  do_release 1 $RELAYER 5000000
  do_release 2 $ETH 0
  do_release 3 $RELAYER 100000000000000
  say "Ethereum: replaying exit 0's release is refused"
  erevert NullifierSpent $VAULT "$SIG" $(release_args 0 $RELAYER 500000)
  say "Ethereum: exit 4's seal redirected to another recipient is refused"
  prove_release 4 $RELAYER 0
  erevert BadSeal $VAULT "$SIG" $(release_args 4 $RELAYER 0 $(x_dest 9))
  say "Ethereum: a relay fee not on the menu is refused"
  erevert RelayFeeNotOnMenu $VAULT "$SIG" $(release_args 4 $RELAYER 12345)
  mark releases1
fi

# ------------------------------------------------------------------------------- 12. update 2, last releases
if ! done_step submit2; then
  say "Ethereum: update 2 (prevState = update 1's newState)"
  esend $VAULT "update(bytes,bytes)" $J2 $S2 > $R/update2-tx.json
  echo "update 2 tx $(python3 -c 'import json,sys;j=json.load(open(sys.argv[1]));print(j["transactionHash"],"gas",int(j["gasUsed"],16))' $R/update2-tx.json)"
  mark submit2
fi
NEW2=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["newState"])' $R/update2/report.json)
[ "$(ecall $VAULT 'g2State()(bytes32)' | lower)" = "$(echo $NEW2 | lower)" ] || die "g2State after update 2"
say "check: vault counters after update 2 = the live bridge config (no LEZ operations since LAST2)"
show_json > $R/view-live.json
check_counters $R/view-live.json || die "vault counters differ from the bridge's after update 2"
ok "vault counters = live bridge counters"
if ! done_step releases2; then
  leaves_from_events
  say "G3 + Ethereum: release exits 4, 5, 6 against the newest root"
  for j in 4 5 6; do prove_release $j $RELAYER 0; done
  AT=$(ecall $VAULT "releasableAt(address,bytes32)(uint256)" $USDC $(. $R/rel/5.env; echo $ROOT) | cut -d' ' -f1)
  wait_until $AT
  do_release 4 $RELAYER 0
  do_release 5 $RELAYER 0
  do_release 6 $RELAYER 0
  mark releases2
fi

# ------------------------------------------------------------------------------- 13. solvency
say "solvency"
python3 - "$VAULT" "$USDC" "$ETH_RPC" <<'EOF' | tee $R/solvency.txt
import subprocess, sys
vault, usdc, rpc = sys.argv[1:]
c = lambda *a: subprocess.check_output(["cast", "call", vault, *a, "--rpc-url", rpc], text=True)
bad = 0
for tok, name in ((usdc, "USDC"), ("0x0000000000000000000000000000000000000000", "ETH")):
    cfg = c("tokenConfig(address)(bool,bool,bool,uint8,uint64,uint256[],uint256[],uint256[],uint256)", tok).split("\n")
    denoms = [int(x.split()[0]) for x in cfg[5].strip("[]").split(",")]
    fees = int(cfg[8].split()[0])
    dep = rel = 0
    for d, dv in enumerate(denoms):
        k = [int(x) for x in c("counters(address,uint256)(uint64,uint64,uint64,uint64)", tok, str(d)).split()[0:4]]
        dep += k[0] * dv; rel += k[3] * dv
    if name == "ETH":
        bal = int(subprocess.check_output(["cast", "balance", vault, "--rpc-url", rpc], text=True))
    else:
        bal = int(subprocess.check_output(["cast", "call", tok, "balanceOf(address)(uint256)", vault, "--rpc-url", rpc], text=True).split()[0])
    want = dep - rel + fees
    print(f"{name}: vault balance {bal} = deposits {dep} - releases {rel} + accrued fees {fees} = {want}: {'OK' if bal == want else 'MISMATCH'}")
    bad += bal != want
sys.exit(1 if bad else 0)
EOF
for pair in "$USDC $DEF_USDC" "$ETH $DEF_ETH"; do
  set -- $pair
  sup=$($T definition --token-program $TP --definition $2 | sed -n 's/^supply //p')
  line=$($T show --bridge $BP | grep -i "^token $(echo $1 | lower)")
  cv=$(echo "$line" | sed -n 's/.*claimed_value \([0-9]*\).*/\1/p'); bv=$(echo "$line" | sed -n 's/.*burned_value \([0-9]*\).*/\1/p')
  [ "$sup" = "$((cv - bv))" ] || python3 -c "import sys; sys.exit(0 if int('$sup') == int('$cv') - int('$bv') else 1)" || die "LEZ supply of $2: $sup, claimed - burned = $cv - $bv"
  echo "LEZ $1: supply $sup = claimed $cv - burned $bv: OK" | tee -a $R/solvency.txt
done

# ------------------------------------------------------------------------------- 14. privacy
say "privacy: no exit commitment E or exit leaf in any release transaction's calldata or logs"
python3 - $R/released.txt $R/exit-commitments.txt $R/exit-leaves.txt "$ETH_RPC" <<'EOF' | tee $R/privacy-check.txt
import json, subprocess, sys
relf, ecf, lf, rpc = sys.argv[1:]
secrets = {"E" + j: e[2:].lower() for j, e in (l.split() for l in open(ecf))}
secrets.update({f"leaf{i}": l.strip()[2:].lower() for i, l in enumerate(open(lf)) if l.strip()})
hits = 0; n = 0
for line in open(relf):
    j, h = line.split()
    calldata = subprocess.check_output(["cast", "tx", h, "input", "--rpc-url", rpc], text=True).strip()
    rc = json.loads(subprocess.check_output(["cast", "receipt", h, "--json", "--rpc-url", rpc], text=True))
    assert calldata.startswith("0x") and len(calldata) > 10, calldata
    blob = calldata.lower()
    for lg in rc["logs"]:
        blob += "".join(lg["topics"]).lower() + lg["data"].lower()
    n += 1
    for name, v in secrets.items():
        if v in blob:
            print(f"LEAK: release of exit {j} ({h}) contains {name}"); hits += 1
print(f"{n} release transactions, {len(secrets)} values (exit commitments and exit leaves) searched: {hits} found")
sys.exit(1 if hits else 0)
EOF
echo; echo "anonymity sets (observer's candidates under the root; unreleased among them):"; cat $R/anonymity.txt
say "done. $LABEL"
