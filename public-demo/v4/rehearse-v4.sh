#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# v4 bring-up REHEARSAL on throwaway chains (build host only; fresh keys only; nothing proved; never
# the public demo chain or Sepolia): the order of operations of public-demo/V4-PLAN.md, so v4 does
# not repeat v3's mistake (a vault that can never take an update).
#
#   mock-verified: proofs executed, not proved; LEZ finality trusted for the test. A
#   MockGroth16Verifier (selector 0xe71e6620) stands in ONLY for RISC Zero's Groth16 check; the
#   UpdateGatedVerifier, the vault and SepoliaV4.s.sol are the real ones.
#
#   R=<new dir> SEQ_BIN=<rc3 sequencer_service> NETCFG=<rc3 networks/local.json> CLI=<lez-bridge>
#   ELF=<dir with bridge_guest.bin, token_guest.bin> HOST=<outbound_host> VAULT_DIR=<vault/ with lib/>
#   RPC=127.0.0.1:<port> METRICS=127.0.0.1:<port> ANVIL_PORT=<port>
#   MIN_RELEASE_DELAY_SECONDS=<s> RELEASE_DELAY_SECONDS=<s>   (required: v4 3600/3600, v5 0/0)
#   public-demo/v4/rehearse-v4.sh [stop]
# With delay 0 (v5) the release is sent right after the update, in the next block, and no
# ReleaseTooEarly is expected.
#
# Order (V4-PLAN step numbers in brackets):
#   1. a LEZ rc3 chain and an anvil with chain id 11155111 (so SepoliaV4.s.sol runs unchanged)
#   2. token program; the SAME bridge ELF deployed twice, at two headers [L1]: BP_OLD (stands for v3,
#      initialised against another vault, and a second Initialize refused) and BP4 (v4)
#   3. G2 genesis at the tip after BP4's deploy, before its Initialize [G1]
#   4. Ethereum: SepoliaV4.s.sol (through its test harness) with INITIAL_G2_STATE [S2]; g2State()
#      read back and compared BEFORE anything else [S3]; configure() [S4]
#   5. LEZ: wrapped ETH (authority Program(BP4)), Initialize BP4 against the v4 vault (cfg ids
#      compared), register [L3-L5]
#   6. deposit 0.01 ETH -> root attested -> public claim -> burn
#   7. G2-lite update from the genesis state (executor); refused when sent to the vault directly and
#      from anyone but the submitter; accepted through the gate from the submitter
#   8. G3 release (executor); refused before the 1-hour delay; anvil time moved; released by an
#      account that is neither the depositor nor the submitter
# Executor runs of HOST go through the build host's lock (LOCK), niced.
set -uo pipefail
: "${MIN_RELEASE_DELAY_SECONDS:?}" "${RELEASE_DELAY_SECONDS:?}"
: "${R:?}" "${SEQ_BIN:?}" "${NETCFG:?}" "${CLI:?}" "${ELF:?}" "${HOST:?}" "${VAULT_DIR:?}" "${RPC:?}" "${METRICS:?}" "${ANVIL_PORT:?}"
for p in "$RPC" "$METRICS" "127.0.0.1:$ANVIL_PORT"; do
  case "$p" in *:3062|*:3071|*:3230|*:3250|*:3290|*:3291|*:3295|*:3297|*:3298|*:3340|*:3341|*:3860|*:8550|*:8545) echo "refusing port $p"; exit 1 ;; esac
done
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT_DIR=$(cd "$HERE/../.." && pwd)
LOCK=${LOCK:-<local path>}
export PATH=$HOME/.foundry/bin:$PATH LEZ_BRIDGE_SEQUENCER=http://$RPC LEE_WALLET_PASSWORD=
export TMPDIR=${TMPDIR:-$R/tmp}
LABEL="mock-verified: proofs executed, not proved; LEZ finality trusted for the test"
ETH_RPC=http://127.0.0.1:$ANVIL_PORT
chain() { R=$R SEQ_BIN=$SEQ_BIN CLI=$CLI NETCFG=$NETCFG RPC=$RPC METRICS=$METRICS "$ROOT_DIR/public-demo/rc3/chain-rc3.sh (not published)" "$@"; }
if [ "${1:-}" = stop ]; then
  [ -f "$R/anvil.pid" ] && kill "$(cat "$R/anvil.pid")" 2>/dev/null; chain stop; exit 0
fi
[ -e "$R/home" ] && { echo "$R/home exists: a fresh R, please (or '$0 stop', then delete it)"; exit 1; }
mkdir -p "$R/run" "$R/wallets" "$R/logs" "$R/blocks" "$TMPDIR"; chmod 700 "$R/wallets"
OUT=$R/run
exec > >(tee -a "$R/rehearsal.log") 2>&1
say() { echo; echo "== $(date +%T) $*"; }
die() { echo "FAILED: $*  [$LABEL]"; exit 1; }
ok() { echo "OK: $*"; }
step() { local n=$1; shift; "$@" > "$OUT/$n.out" 2>&1; local rc=$?; grep -vE "^Transaction data" "$OUT/$n.out" | cut -c1-200 | tail -"${SHOWN:-3}"; return $rc; }
must() { local n=$1; shift; step "$n" "$@" || die "$n (output $OUT/$n.out)"; }
lower() { tr 'A-F' 'a-f'; }
idof() { grep -oE 'Public/[1-9A-HJ-NP-Za-km-z]{32,44}' | sed -n 1p | cut -d/ -f2; }
block_of() { sed -nE 's/.*(applied|included) in block ([0-9]+).*/\2/p' | tail -1; }
host() { MIN_FREE=${MIN_FREE:-15} "$LOCK" nice -n 19 "$HOST" "$@"; }
ecall() { cast call "$@" --rpc-url "$ETH_RPC"; }
esend() { local k=$1; shift; cast send "$@" --private-key "$k" --rpc-url "$ETH_RPC" --json; }
# a transaction that must revert with custom error $2 (sent from key $1)
erevert() { local k=$1 want=$2; shift 2; local out; if out=$(cast send "$@" --private-key "$k" --rpc-url "$ETH_RPC" 2>&1); then die "expected $want, it went through"; fi
  local sel; sel=$(cast sig "$want()"); grep -qi -- "${sel#0x}\|$want" <<<"$out" || die "expected $want ($sel), got: $(echo "$out" | tail -2)"; echo "refused as expected: $want"; }
# anvil's public test keys: #0 deployer (= the harness's DEPLOYER), #3 depositor, #4 SUBMITTER, #5 anyone
K0=$("$ROOT_DIR/lez-bridge/scripts/anvil-dev-key.sh" 0) || exit 1
K3=$("$ROOT_DIR/lez-bridge/scripts/anvil-dev-key.sh" 3) || exit 1
K4=$("$ROOT_DIR/lez-bridge/scripts/anvil-dev-key.sh" 4) || exit 1
K5=$("$ROOT_DIR/lez-bridge/scripts/anvil-dev-key.sh" 5) || exit 1
SUBMITTER=$(cast wallet address --private-key $K4); ANYONE=$(cast wallet address --private-key $K5)
ETH=0x0000000000000000000000000000000000000000; D001=10000000000000000
echo "$LABEL"; echo "run $R; CLI $(shasum -a 256 "$CLI" | cut -c1-16)…; host $(shasum -a 256 "$HOST" | cut -c1-16)…"
shasum -a 256 "$ELF/bridge_guest.bin" "$ELF/token_guest.bin" "$CLI" "$HOST" | tee "$R/inputs.sha256"

say "1. throwaway LEZ rc3 chain on $RPC and anvil (chain id 11155111) on $ANVIL_PORT"
chain setup || die "chain setup"; chain start || die "chain start"; . "$R/operator.env"
nohup anvil --port "$ANVIL_PORT" --chain-id 11155111 --silent > "$R/logs/anvil.log" 2>&1 & echo $! > "$R/anvil.pid"
for _ in $(seq 1 30); do cast chain-id --rpc-url "$ETH_RPC" >/dev/null 2>&1 && break; sleep 1; done
[ "$(cast chain-id --rpc-url "$ETH_RPC")" = 11155111 ] || die "anvil chain id"
for _ in $(seq 1 60); do "$HOST" last --rpc "http://$RPC" >/dev/null 2>&1 && break; sleep 2; done
W=$R/wallets/deploy; mkdir -p "$W"; chmod 700 "$W"
d()  { LEE_WALLET_HOME_DIR=$W "$CLI" "$@"; }
op() { LEE_WALLET_HOME_DIR=$R/wallets/operator "$CLI" "$@"; }
d wallet check-health >/dev/null 2>&1
d wallet account new public > "$OUT/new-d.out" 2>&1; D=$(idof < "$OUT/new-d.out"); DPK=$(sed -n 's/^With pk //p' "$OUT/new-d.out" | sed -n 1p)
d wallet account new public > "$OUT/new-p.out" 2>&1; P=$(idof < "$OUT/new-p.out"); PPK=$(sed -n 's/^With pk //p' "$OUT/new-p.out" | sed -n 1p)
must fund-d op wallet auth-transfer send --from Public/$OP --to-pk $DPK --amount 200000000000
must fund-p op wallet auth-transfer send --from Public/$OP --to-pk $PPK --amount 20000000000
echo "deployer/admin/attestor D=$D, claimer P=$P"

say "2. token program, then the SAME bridge ELF at two headers (BP_OLD, BP4)"
must deploy-token d deploy --which token --elf "$ELF/token_guest.bin" --payer $D
TP=$(sed -n 's/^program //p' "$OUT/deploy-token.out")
must deploy-old d deploy --which bridge --elf "$ELF/bridge_guest.bin" --payer $D
BP_OLD=$(sed -n 's/^program //p' "$OUT/deploy-old.out"); IMG_OLD=$(sed -n 's/^image id //p' "$OUT/deploy-old.out")
must deploy-v4 d deploy --which bridge --elf "$ELF/bridge_guest.bin" --payer $D
BP4=$(sed -n 's/^program //p' "$OUT/deploy-v4.out"); IMG4=$(sed -n 's/^image id //p' "$OUT/deploy-v4.out")
DEPLOY4_BLOCK=$(block_of < "$OUT/deploy-v4.out")
echo "token $TP; bridge BP_OLD $BP_OLD image $IMG_OLD; bridge BP4 $BP4 image $IMG4 (deploy ended in block ${DEPLOY4_BLOCK:-?})"
[ -n "$BP_OLD" ] && [ -n "$BP4" ] && [ "$BP_OLD" != "$BP4" ] || die "two distinct bridge program ids"
[ "$IMG_OLD" = "$IMG4" ] || die "the two deploys have different images"
ok "one image ($IMG4), two programs: a program id is its header account, not its image id"
LEZ_CHAIN_ID=$(d chain-id | sed -n 's/^LEZ_CHAIN_ID=//p'); ZONE_ID=$(d source-id | sed -n 's/^ZONE_ID=//p')
PROGRAM_ID=$(d program-id --bridge $BP4 | sed -n 's/^PROGRAM_ID=//p')
PROGRAM_ID_OLD=$(d program-id --bridge $BP_OLD | sed -n 's/^PROGRAM_ID=//p')
echo "LEZ_CHAIN_ID=$LEZ_CHAIN_ID ZONE_ID=$ZONE_ID PROGRAM_ID(BP4)=$PROGRAM_ID PROGRAM_ID(BP_OLD)=$PROGRAM_ID_OLD"
say "2b. BP_OLD initialised against another vault (as v3 is), then a second Initialize refused"
must init-old d init --bridge $BP_OLD --token-program $TP --admin $D --vault 0x26af59855b99a8af199361289A5Ed6088729E60E \
  --chain-id 11155111 --freezer $D --attestor $D --payer $D
if step init-old-again d init --bridge $BP_OLD --token-program $TP --admin $D --vault 0x000000000000000000000000000000000000dEaD \
  --chain-id 11155111 --freezer $D --attestor $D --payer $D; then die "a second Initialize of BP_OLD applied"; fi
ok "a second Initialize of the same program is refused (once per program)"

say "3. G2 genesis for BP4 at the current tip (after BP4's deploy, before its Initialize)"
TIP=$("$HOST" last --rpc "http://$RPC"); echo "tip $TIP"
host genesis --rpc "http://$RPC" --dir "$R/blocks" --block $TIP --bridge $BP4 --out "$R/state0.bin" > "$OUT/genesis.out" 2>&1 || { cat "$OUT/genesis.out"; die genesis; }
cat "$OUT/genesis.out"
G2_STATE0=$(sed -nE 's/^(INITIAL_)?G2_STATE=//p' "$OUT/genesis.out" | sed -n 1p)
[ -n "$G2_STATE0" ] && [ "$G2_STATE0" != 0x0000000000000000000000000000000000000000000000000000000000000000 ] || die "no genesis state"
"$HOST" ids > "$OUT/ids.out" 2>&1; cat "$OUT/ids.out"
UPDATE_IMAGE_ID=$(sed -n 's/^UPDATE_IMAGE_ID_TRUSTED_TIP=//p' "$OUT/ids.out"); RELEASE_IMAGE_ID=$(sed -n 's/^RELEASE_IMAGE_ID=//p' "$OUT/ids.out")
[ -n "$UPDATE_IMAGE_ID" ] && [ -n "$RELEASE_IMAGE_ID" ] || die "image ids"
# The ids the plan pins (inputs: v4/BUILD-outbound.txt on the build host); refuse a host that embeds others.
[ -z "${EXPECT_UPDATE_IMAGE_ID:-}" ] || [ "$(echo $UPDATE_IMAGE_ID | lower)" = "$(echo $EXPECT_UPDATE_IMAGE_ID | lower)" ] || die "host update image $UPDATE_IMAGE_ID != $EXPECT_UPDATE_IMAGE_ID"
[ -z "${EXPECT_RELEASE_IMAGE_ID:-}" ] || [ "$(echo $RELEASE_IMAGE_ID | lower)" = "$(echo $EXPECT_RELEASE_IMAGE_ID | lower)" ] || die "host release image $RELEASE_IMAGE_ID != $EXPECT_RELEASE_IMAGE_ID"
SELECTOR=$(sed -n 's/^GROTH16_SELECTOR=//p' "$OUT/ids.out"); echo "host Groth16 selector ${SELECTOR:-(not printed)}"
[ -z "$SELECTOR" ] || [ "$SELECTOR" = 0xe71e6620 ] || die "the host's Groth16 selector is $SELECTOR, not 0xe71e6620 (3.1.0)"

say "4. Ethereum: mock Groth16 verifier (selector 0xe71e6620), then SepoliaV4.s.sol (harness: deployer = anvil #0)"
( cd "$VAULT_DIR" && forge create test/mocks/Mocks.sol:MockGroth16Verifier \
    --rpc-url "$ETH_RPC" --private-key $K0 --broadcast --constructor-args 0xe71e6620 ) > "$OUT/mock.out" 2>&1 || { cat "$OUT/mock.out"; die "mock verifier"; }
MOCK=$(sed -n 's/^Deployed to: //p' "$OUT/mock.out"); echo "MockGroth16Verifier $MOCK (stands in for 0x8c91…aB3b)"
( cd "$VAULT_DIR" && SEPOLIA_PK=$K0 RISC0_VERIFIER=$MOCK SUBMITTER=$SUBMITTER RELEASE_IMAGE_ID=$RELEASE_IMAGE_ID \
    UPDATE_IMAGE_ID=$UPDATE_IMAGE_ID PROGRAM_ID=$PROGRAM_ID LEZ_CHAIN_ID=$LEZ_CHAIN_ID ZONE_ID=$ZONE_ID \
    INITIAL_G2_STATE=$G2_STATE0 MIN_RELEASE_DELAY_SECONDS=$MIN_RELEASE_DELAY_SECONDS RELEASE_DELAY_SECONDS=$RELEASE_DELAY_SECONDS \
    forge script test/SepoliaV4Script.t.sol:SepoliaV4Harness \
    --rpc-url "$ETH_RPC" --broadcast --slow ) > "$OUT/sepoliav4.out" 2>&1 || { cat "$OUT/sepoliav4.out"; die "SepoliaV4 deploy"; }
grep -E '^ *[A-Z_0-9]+=' "$OUT/sepoliav4.out" | sed 's/^ *//' | tee "$R/v4.env"
val() { sed -n "s/^$1=\([^ ]*\).*/\1/p" "$R/v4.env" | sed -n 1p; }
GATE=$(val GATE); VAULT=$(val VAULT); USDC=$(val USDC); CFG_ID=$(val CFG_ID)
[ -n "$GATE" ] && [ -n "$VAULT" ] && [ -n "$CFG_ID" ] || die "the script printed no GATE/VAULT/CFG_ID"
say "4b. FIRST, before anything else: the vault's g2State() is the genesis state"
ONCHAIN_G2=$(ecall $VAULT 'g2State()(bytes32)')
echo "vault g2State() $ONCHAIN_G2; genesis $G2_STATE0"
[ "$(echo $ONCHAIN_G2 | lower)" = "$(echo $G2_STATE0 | lower)" ] || die "g2State differs from the genesis state"
[ "$(ecall $VAULT 'verifier()(address)' | lower)" = "$(echo $GATE | lower)" ] || die "vault verifier is not the gate"
[ "$(ecall $GATE 'VERIFIER()(address)' | lower)" = "$(echo $MOCK | lower)" ] || die "gate VERIFIER"
[ "$(ecall $GATE 'SUBMITTER()(address)' | lower)" = "$(echo $SUBMITTER | lower)" ] || die "gate SUBMITTER"
ok "g2State = genesis; verifier = gate -> Groth16 stand-in; SUBMITTER = $SUBMITTER"
( cd "$VAULT_DIR" && SEPOLIA_PK=$K0 VAULT=$VAULT USDC=$USDC RELEASE_DELAY_SECONDS=$RELEASE_DELAY_SECONDS forge script test/SepoliaV4Script.t.sol:SepoliaV4Harness \
    --sig "configure()" --rpc-url "$ETH_RPC" --broadcast --slow ) > "$OUT/configure.out" 2>&1 || { cat "$OUT/configure.out"; die configure; }
[ "$(ecall $VAULT 'minReleaseDelay()(uint64)')" = "$MIN_RELEASE_DELAY_SECONDS" ] || die "minReleaseDelay"
ok "configure(): ETH 0.01/0.1/1, dUSDC 100/1,000/10,000, release delay $RELEASE_DELAY_SECONDS s (min $MIN_RELEASE_DELAY_SECONDS s)"

say "5. LEZ: wrapped ETH for BP4 on the shared token program, Initialize BP4 against the v4 vault, register"
must wrap-eth d create-wrapped --token-program $TP --bridge $BP4 --name "Wrapped Sepolia ETH (v4 rehearsal)" --payer $D
DEF_ETH=$(sed -n 's/^token //p' "$OUT/wrap-eth.out")
must init-v4 d init --bridge $BP4 --token-program $TP --admin $D --vault $VAULT --chain-id 11155111 --freezer $D --attestor $D --payer $D
INIT_BLOCK=$(block_of < "$OUT/init-v4.out"); echo "Initialize of BP4 in block ${INIT_BLOCK:-?}; genesis tip $TIP"
[ -n "$INIT_BLOCK" ] && [ "$TIP" -lt "$INIT_BLOCK" ] || die "the genesis tip is not before Initialize"
must register-eth d register --bridge $BP4 --admin $D --eth-token $ETH --definition $DEF_ETH --decimals 18 \
  --denoms 10000000000000000,100000000000000000,1000000000000000000
LEZ_CFG=$(d show --bridge $BP4 | sed -n 's/^cfg_id //p')
[ "$(echo $LEZ_CFG | lower)" = "$(echo $CFG_ID | lower)" ] || die "bridge cfg_id $LEZ_CFG != vault cfgId $CFG_ID"
OLD_CFG=$(d show --bridge $BP_OLD | sed -n 's/^cfg_id //p')
ok "BP4 cfg_id = vault cfgId = $CFG_ID (BP_OLD, same image: $OLD_CFG)"

say "6. deposit 0.01 ETH for P -> root attested on LEZ -> public claim -> burn to an Ethereum address"
SECRET=$(cast keccak "v4-rehearsal-deposit-$P")
C=$(d commitment --secret $SECRET --dest $P --eth-token $ETH --denom $D001 --cfg-id $CFG_ID)
esend $K3 $VAULT "depositETH(uint32,bytes32)" 0 $C --value $D001 > "$OUT/deposit.json" || die deposit
LEAF=$(ecall $VAULT "depositLeaf(address,uint256,bytes32)(bytes32)" $ETH $D001 $C); echo "$LEAF" > "$R/leaves.txt"
ROOT=$(ecall $VAULT "depositRoot()(bytes32)"); echo "deposit commitment $C, leaf $LEAF, root $ROOT"
must attest d attest --bridge $BP4 --attestor $D --root $ROOT
must claim d claim --bridge $BP4 --token-program $TP --from $P --eth-token $ETH --denom-index 0 --denom $D001 \
  --secret $SECRET --cfg-id $CFG_ID --leaves "$R/leaves.txt" --index 0
XSECRET=$(cast keccak "v4-rehearsal-exit-$P"); DEST=$(cast keccak "v4-rehearsal-recipient" | cut -c1-42)
E=$(d exit-commitment --secret $XSECRET --eth-dest $DEST --eth-token $ETH --denom $D001 --max-relay-fee 100000000000000 --cfg-id $CFG_ID | sed -n 1p)
must burn d burn --bridge $BP4 --token-program $TP --from $P --definition $DEF_ETH --amount $D001 --exit $E
LAST=$(block_of < "$OUT/burn.out"); echo "exit commitment $E, burn in block $LAST"
d show --bridge $BP4 > "$OUT/show.out" 2>&1; grep -E '^token ' "$OUT/show.out"

say "7. G2-lite update from the genesis state over blocks $((TIP + 1))..$LAST (native + TrustedTip guest in the executor)"
host update --rpc "http://$RPC" --dir "$R/blocks" --state "$R/state0.bin" --to $LAST --out "$R/update1" > "$OUT/update.out" 2>&1 || { tail -20 "$OUT/update.out"; die update; }
tail -6 "$OUT/update.out"
J=$(cat "$R/update1/journal.hex"); S=0xe71e6620$(sed 's/^0x//' "$R/update1/seal.hex")
PREV=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["prevState"])' "$R/update1/report.json")
NEW=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["newState"])' "$R/update1/report.json")
echo "journal prevState $PREV, newState $NEW"
[ "$(echo $PREV | lower)" = "$(echo $G2_STATE0 | lower)" ] || die "the journal's prevState is not the vault's genesis state"
erevert $K5 UpdateNotThroughGate $VAULT "update(bytes,bytes)" $J $S
erevert $K4 UpdateNotThroughGate $VAULT "update(bytes,bytes)" $J $S
erevert $K5 NotSubmitter $GATE "submitUpdate(address,bytes,bytes)" $VAULT $J $S
esend $K4 $GATE "submitUpdate(address,bytes,bytes)" $VAULT $J $S > "$OUT/submit.json" || die "submitUpdate"
echo "submitUpdate tx $(python3 -c 'import json,sys;j=json.load(open(sys.argv[1]));print(j["transactionHash"],"gas",int(j["gasUsed"],16))' "$OUT/submit.json")"
[ "$(ecall $VAULT 'g2State()(bytes32)' | lower)" = "$(echo $NEW | lower)" ] || die "g2State after the update"
COUNTERS=$(ecall $VAULT "counters(address,uint256)(uint64,uint64,uint64,uint64)" $ETH 0 | tr '\n' ' ')
echo "vault ETH 0.01 counters (deposits claims burns releases): $COUNTERS"
[ "$(echo $COUNTERS | awk '{print $1,$2,$3,$4}')" = "1 1 1 0" ] || die "counters"
erevert $K4 StaleState $GATE "submitUpdate(address,bytes,bytes)" $VAULT $J $S
ok "update accepted from the genesis state through the gate only; a replay is stale"

say "8. G3 release proof (executor), the release delay ($RELEASE_DELAY_SECONDS s), then a release sent by anyone"
cast logs --from-block 0 --address $VAULT "StateUpdated(bytes32,bytes32,bytes32,uint64,bytes32[])" --rpc-url "$ETH_RPC" --json > "$OUT/stateupdated.json"
python3 - "$OUT/stateupdated.json" > "$R/exit-leaves.txt" <<'EOF'
import json, sys
for l in json.load(open(sys.argv[1])):
    d = bytes.fromhex(l["data"][2:]); w = lambda i: d[32 * i:32 * i + 32]
    off = int.from_bytes(w(4), "big"); n = int.from_bytes(d[off:off + 32], "big")
    for k in range(n): print("0x" + d[off + 32 + 32 * k: off + 64 + 32 * k].hex())
EOF
host release --secret $XSECRET --eth-dest $DEST --token $ETH --denom $D001 --max-relay-fee 100000000000000 \
  --cfg-id $CFG_ID --leaves "$R/exit-leaves.txt" --index 0 --relayer $ETH --relay-fee 0 > "$OUT/release.out" 2>&1 || { tail -20 "$OUT/release.out"; die release; }
grep -E '^(ROOT|NULLIFIER|USER_CYCLES)=' "$OUT/release.out"
XROOT=$(sed -n 's/^ROOT=//p' "$OUT/release.out"); NF=$(sed -n 's/^NULLIFIER=//p' "$OUT/release.out")
RS=0xe71e6620$(sed -n 's/^SEAL=//p' "$OUT/release.out" | sed 's/^0x//')
SIG="release(bytes,bytes32,bytes32,address,uint32,address,address,uint256)"
if [ "$RELEASE_DELAY_SECONDS" -gt 0 ]; then
  erevert $K5 ReleaseTooEarly $VAULT "$SIG" $RS $XROOT $NF $ETH 0 $DEST $ETH 0
  cast rpc evm_increaseTime $((RELEASE_DELAY_SECONDS + 1)) --rpc-url "$ETH_RPC" >/dev/null && cast rpc evm_mine --rpc-url "$ETH_RPC" >/dev/null
else
  echo "release delay 0: no wait, no time moved (releasableAt $(ecall $VAULT 'releasableAt(address,bytes32)(uint256)' $ETH $XROOT), now $(cast block latest -f timestamp --rpc-url "$ETH_RPC"))"
  BADRS=0xe71e6620$(printf 'ab%.0s' {1..64})
  erevert $K5 BadSeal $VAULT "$SIG" $BADRS $XROOT $NF $ETH 0 $DEST $ETH 0
fi
B0=$(cast balance $DEST --rpc-url "$ETH_RPC")
esend $K5 $VAULT "$SIG" $RS $XROOT $NF $ETH 0 $DEST $ETH 0 > "$OUT/release.json" || die "release"
B1=$(cast balance $DEST --rpc-url "$ETH_RPC")
echo "release tx $(python3 -c 'import json,sys;j=json.load(open(sys.argv[1]));print(j["transactionHash"],"gas",int(j["gasUsed"],16))' "$OUT/release.json"); recipient $DEST: $B0 -> $B1"
[ "$((B1 - B0))" = "$D001" ] || die "the recipient did not get 0.01 ETH"
erevert $K5 NullifierSpent $VAULT "$SIG" $RS $XROOT $NF $ETH 0 $DEST $ETH 0
ok "released 0.01 ETH by proof (executor; Groth16 stand-in) from a vault that started at the genesis state"

say "done: the v4 order of operations held. $LABEL"
echo "stop: R=$R ... $0 stop; then delete $R"
