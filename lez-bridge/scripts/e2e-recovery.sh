#!/bin/bash
# NOTE (2026-11-14): written for the CLI and programs BEFORE the security fixes (`init --zone-id`, init
# signed by --admin, first-come). The security-fixes CLI's `init` is signed by the bridge's header key
# with --payer and takes no --zone-id (see lez-bridge/scripts/e2e-security.sh); update this script
# before using it with that CLI. Kept as is for the live v2 deployment and its logs.
# Position recovery from the wallet seed, end to end on local chains, public-account mode
# (RFP-022 Functionality #9, Reliability #5). No proving.
#
#   1. a fresh LEZ wallet (its recovery phrase captured to a mode-600 file, tests only), three
#      seed-tree public accounts P1 P2 P3 (P3 never touches LEZ: its only footprint is a deposit);
#   2. a new token + bridge deployment (guest ELFs copied from the deployed build, image ids
#      checked) and a new vault on the shared anvil (mock verifier, anvil account #5);
#   3. deposits through the DERIVED path (`deposit-request`), one request abandoned, plus foreign
#      deposits; claims (`claim --key-index`); burns (`burn` with derived and user-chosen
#      destinations);
#   4. a mock-verified outbound update puts two of the three exits in the vault's exit tree and a
#      mock-verified release pays one of them, so all three exit states exist;
#   5. DELETE the client cache, `recover` from the wallet alone, compare with the truth this
#      script recorded; also `--lez-mode point`; then claim a recovered deposit and recover again;
#   6. restore a SECOND wallet from the recovery phrase alone (`wallet restore-keys`), recover
#      there with no cache, compare with the truth. (`scripts/e2e-recovery-extra.sh` then restores
#      a shallow third wallet, where `--account-gap` is what finds the missing accounts.)
#
#   R=run/recovery1 LEZ_BRIDGE_BIN=<target-client>/release/lez-bridge \
#   GUESTS=<dir with token_guest.bin bridge_guest.bin> scripts/e2e-recovery.sh
set -euo pipefail
: "${R:?set R to a fresh run directory}"
: "${LEZ_BRIDGE_BIN:?set LEZ_BRIDGE_BIN to the lez-bridge binary to test}"
: "${GUESTS:?set GUESTS to the directory holding token_guest.bin and bridge_guest.bin}"
HERE=$(cd "$(dirname "$0")/.." && pwd)
VAULT_DIR=${VAULT_DIR:-$HERE/../vault}
export PATH=$HOME/.foundry/bin:$PATH
export LEE_WALLET_HOME_DIR=$R/wallet
export LEZ_BRIDGE_SEQUENCER=${SEQ:-http://127.0.0.1:3250}
export LEZ_BRIDGE_ETH_RPC=${ETH_RPC:-http://127.0.0.1:8550}
export RISC0_PROVER=local RAYON_NUM_THREADS=4
ETH_RPC=$LEZ_BRIDGE_ETH_RPC
T="nice -n 10 $LEZ_BRIDGE_BIN"
# The images every earlier run deployed (run/sepolia2/attested/check-images.py).
TOKEN_IMAGE=633555cc0d90901139f0bf541fe2f32912f969579e829e9d6acab2def083ab5d
BRIDGE_IMAGE=249a19c6386bfab7b70018312585f4555d9a302cd972308ea4b1a77db74f0129
LOCAL_TEST_KEY=$("$HERE/scripts/lez-test-key.sh") || exit 1   # LEZ published genesis test key, read from LEZ
EK=0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba   # anvil account #5
ZERO32=0x0000000000000000000000000000000000000000000000000000000000000000
USER_DEST=0x000000000000000000000000000000000000bEEF
say() { echo; echo "## $(date +%T) $*"; }
die() { echo "FAILED: $*" >&2; exit 1; }
trap 'echo "FAILED at line $LINENO" >&2' ERR
id_of() { grep -oE "$1/[1-9A-HJ-NP-Za-km-z]{32,44}" | sed -n 1p | cut -d/ -f2; }
kv() { sed -n "s/^$1=//p" | tail -1; }
[ -e "$R/wallet" ] && die "$R already has a wallet: use a fresh R"
mkdir -p "$R"; chmod 700 "$R"
TRUTH=$R/truth.tsv; : > "$TRUTH"     # kind account key_index denom state
truth() { printf '%s\t%s\t%s\t%s\t%s\n' "$@" >> "$TRUTH"; }

say "LEZ wallet (recovery phrase to a mode-600 file, tests only)"
LEZ_BRIDGE_TEST_MNEMONIC_FILE=$R/mnemonic.txt $T wallet check-health
F=$($T wallet account import public --private-key $LOCAL_TEST_KEY | id_of Public)
P1=$($T wallet account new public | id_of Public)
P2=$($T wallet account new public | id_of Public)
P3=$($T wallet account new public | id_of Public)
echo "F=$F (imported, funds fees) P1=$P1 P2=$P2 P3=$P3 (seed tree)"
for P in $P1 $P2; do $T wallet auth-transfer send --from Public/$F --to Public/$P --amount 200000000000; done

say "deploy token and bridge from the copied guest ELFs; check their image ids"
dep() {
  local out; out=$($T deploy --which $1 --payer $F --elf $GUESTS/$1_guest.bin)
  echo "$out" >&2
  [ "$(echo "$out" | sed -n 's/^image id //p')" = "$2" ] || die "$1 image id differs from the deployed one $2"
  echo "$out" | sed -n 's/^program //p'
}
TP=$(dep token $TOKEN_IMAGE)
BP=$(dep bridge $BRIDGE_IMAGE)
PROGRAM_ID=$($T program-id --bridge $BP | kv PROGRAM_ID)

say "Ethereum: vault with the mock verifier, from anvil account #5"
out=$(cd "$VAULT_DIR" && PROGRAM_ID=$PROGRAM_ID forge script script/LocalDeploy.s.sol --rpc-url $ETH_RPC --private-key $EK --broadcast 2>&1) \
  || { echo "$out"; die "vault deploy"; }
VAULT=$(echo "$out" | sed -n 's/.*VAULT=\(0x[0-9a-fA-F]*\).*/\1/p' | sed -n 1p)
USDC=$(echo "$out" | sed -n 's/.*USDC=\(0x[0-9a-fA-F]*\).*/\1/p' | sed -n 1p)
CFG_ID=$(echo "$out" | sed -n 's/.*CFG_ID=\(0x[0-9a-fA-F]*\).*/\1/p' | sed -n 1p)
EVM=$(cast chain-id --rpc-url $ETH_RPC)
ETH_FROM=$(cast block-number --rpc-url $ETH_RPC)
DERIVED=$($T cfg-id --vault $VAULT --chain-id $EVM --lez-chain-id $ZERO32 --zone-id $ZERO32 --bridge $BP | kv CFG_ID)
[ "$(echo $DERIVED | tr A-F a-f)" = "$(echo $CFG_ID | tr A-F a-f)" ] || die "cfg_id mismatch: vault $CFG_ID, bridge $DERIVED"
$T create-wrapped --token-program $TP --bridge $BP --name "Wrapped USDC" --payer $F | tee "$R/create-wrapped.log"
DEF=$(sed -n 's/^token //p' "$R/create-wrapped.log")
# The first block the bridge deployment can appear in (a hint for recover-0 only; the
# recoveries from the wallet alone scan from block 1).
LEZ_FROM=$(sed -n 's/^applied in block //p' "$R/create-wrapped.log")
$T init --bridge $BP --token-program $TP --admin $F --vault $VAULT --chain-id $EVM --lez-chain-id $ZERO32 --zone-id $ZERO32 --freezer $F --attestor $F
$T register --bridge $BP --admin $F --eth-token $USDC --definition $DEF --decimals 6 --denoms 100000000,1000000000
cat > "$R/ids.env" <<EOF
F=$F P1=$P1 P2=$P2 P3=$P3 TP=$TP BP=$BP DEF=$DEF VAULT=$VAULT USDC=$USDC CFG_ID=$CFG_ID EVM=$EVM ETH_FROM=$ETH_FROM LEZ_FROM=$LEZ_FROM
EOF
cat "$R/ids.env"
CH="--eth-from-block $ETH_FROM"
DENOMS=(100000000 1000000000)

say "deposits through the derived path"
deposit() { # $1 account $2 denom index [$3 = abandon]
  local out c j; out=$($T deposit-request --bridge $BP --from $1 --eth-token $USDC --denom-index $2 $CH)
  c=$(echo "$out" | kv COMMITMENT); j=$(echo "$out" | kv KEY_INDEX)
  echo "$1 denom#$2 index $j commitment $c request $(echo "$out" | kv REQUEST)"
  if [ "${3:-}" = abandon ]; then echo "  (request abandoned: never paid)"; return; fi
  cast send $VAULT "deposit(address,uint32,bytes32)" $USDC $2 $c --private-key $EK --rpc-url $ETH_RPC >/dev/null
  echo "$1 $j $2" >> "$R/deposits.made"
}
: > "$R/deposits.made"
deposit $P1 0; deposit $P1 0; deposit $P1 1; deposit $P2 0; deposit $P2 0 abandon; deposit $P2 0; deposit $P3 0
for i in 1 2 3; do  # someone else's deposits
  cast send $VAULT "deposit(address,uint32,bytes32)" $USDC $((i % 2)) $(cast keccak "foreign-$i-$R") --private-key $EK --rpc-url $ETH_RPC >/dev/null
done
grep -q "^$P2 2 0$" "$R/deposits.made" || die "the abandoned request did not consume index 1 of P2"

say "LEZ: record the vault root (stand-in attestor); claim three deposits by key index"
attest_now() { $T attest --bridge $BP --attestor $F --root $(cast call $VAULT "depositRoot()(bytes32)" --rpc-url $ETH_RPC); }
attest_now
claim() { $T claim --bridge $BP --token-program $TP --from $1 --eth-token $USDC --denom-index $3 --denom ${DENOMS[$3]} --key-index $2 $CH; }
claim $P1 0 0; claim $P1 0 1; claim $P2 0 0
say "a repeated claim is refused locally"
if claim $P1 0 0 2>&1 | tee "$R/double-claim.log"; then die "double claim applied"; fi
grep -q "already claimed" "$R/double-claim.log" || die "double claim not diagnosed"

say "LEZ: burns through the derived path"
burn() { $T burn --bridge $BP --token-program $TP --from $1 --definition $DEF --amount 100000000 $CH "${@:2}"; }
burn $P1 | tee "$R/burn1.log"
burn $P1 --eth-dest $USER_DEST --max-relay-fee 0 | tee "$R/burn2.log"
burn $P2 | tee "$R/burn3.log"
E1=$(sed -n 's/^exit commitment //p' "$R/burn1.log"); E2=$(sed -n 's/^exit commitment //p' "$R/burn2.log"); E3=$(sed -n 's/^exit commitment //p' "$R/burn3.log")
say "a burn with an off-menu maxRelayFee is refused before sending"
if burn $P1 --max-relay-fee 12345 2>&1 | tee "$R/bad-fee.log"; then die "off-menu fee accepted"; fi

say "scan once with the cache, to read the exits' public values for the mock G2 update"
$T recover --bridge $BP $CH --lez-from-block $LEZ_FROM --eth-dest $USER_DEST --json > "$R/recover-0.json"
python3 - "$R/recover-0.json" "$E1" "$E2" "$E3" > "$R/exits.env" <<'PY'
import json, sys
r = json.load(open(sys.argv[1])); es = sys.argv[2:]
by = {e["exit_commitment"]: e for e in r["exits"]}
for i, e in enumerate(es, 1):
    x = by.get(e)
    assert x, f"exit {i} {e} not found by the scan: {sorted(by)}"
    print(f"NF{i}={x['release_nullifier']} DEST{i}={x['eth_dest']} DI{i}={x['denom_index']}")
PY
cat "$R/exits.env"; . "$R/exits.env"

say "Ethereum: mock-verified outbound update with exits 1 and 3 (exit 2 stays burned only)"
ROOT=$(cast call $VAULT "depositRoot()(bytes32)" --rpc-url $ETH_RPC)
PREV=$(cast call $VAULT "g2State()(bytes32)" --rpc-url $ETH_RPC)
J=$(cast abi-encode "f((bytes32,bytes32,bytes32,bytes32[],(address,uint32,uint64)[],(address,uint32,bytes32)[]))" \
  "($CFG_ID,$PREV,$(cast keccak recovery1-state),[$ROOT],[($USDC,0,2),($USDC,1,1)],[($USDC,0,$E1),($USDC,0,$E3)])")
sha() { python3 -c 'import hashlib,sys;print("0x"+hashlib.sha256(bytes.fromhex(sys.argv[1][2:])).hexdigest())' "$1"; }
SEAL=$(cast keccak g2)$(sha $J | cut -c3-)
cast send $VAULT "update(bytes,bytes)" $J $SEAL --private-key $EK --rpc-url $ETH_RPC >/dev/null
EXIT_ROOT=$(cast call $VAULT "exitRoot()(bytes32)" --rpc-url $ETH_RPC)
echo "exit root $EXIT_ROOT ($(cast call $VAULT "exitCount()(uint64)" --rpc-url $ETH_RPC) exits)"

say "Ethereum: mock-verified release of exit 3 (P2's) to its derived address, no relayer"
DIG=$(sha $(cast abi-encode "f(bytes32,bytes32,bytes32,address,uint256,address,address,uint256)" $CFG_ID $EXIT_ROOT $NF3 $USDC 100000000 $DEST3 0x0000000000000000000000000000000000000000 0))
cast send $VAULT "release(bytes,bytes32,bytes32,address,uint32,address,address,uint256)" \
  $(cast keccak g3)$(echo $DIG | cut -c3-) $EXIT_ROOT $NF3 $USDC 0 $DEST3 0x0000000000000000000000000000000000000000 0 \
  --private-key $EK --rpc-url $ETH_RPC >/dev/null
echo "released to $DEST3: balance $(cast call $USDC "balanceOf(address)(uint256)" $DEST3 --rpc-url $ETH_RPC)"

# The truth, from what this script did.
while read -r a j d; do
  st=claimable
  case "$a $j $d" in "$P1 0 0"|"$P1 0 1"|"$P2 0 0") st=claimed;; esac
  truth dep $a $j ${DENOMS[$d]} $st
done < "$R/deposits.made"
truth exit $P1 0 derived tree; truth exit $P1 0 $USER_DEST burned; truth exit $P2 0 derived released
echo "truth:"; cat "$TRUTH"

compare() { # $1 recover json, $2 "with-user-dest" or "" , $3 label
  python3 - "$1" "$TRUTH" "$2" "$3" <<'PY'
import json, sys
B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
def b58(s):
    n = 0
    for c in s: n = n * 58 + B58.index(c)
    return "0x" + n.to_bytes(32, "big").hex()
r = json.load(open(sys.argv[1])); with_user = sys.argv[3] == "with-user-dest"; label = sys.argv[4]
want_d, want_e = set(), set()
for line in open(sys.argv[2]):
    k, a, j, d, st = line.split()
    if k == "dep": want_d.add((b58(a), int(j), d, st))
    elif d == "derived" or with_user: want_e.add((b58(a), int(j), "derived" if d == "derived" else d.lower(), st))
got_d = {(x["account"], x["key_index"], x["denom"], x["state"]) for x in r["deposits"]}
st = {"burned": "burned", "unreleased": "tree", "released": "released"}
got_e = {(x["account"], x["key_index"], "derived" if x["dest_derived"] else x["eth_dest"].lower(), st[x["state"]]) for x in r["exits"]}
ok = got_d == want_d and got_e == want_e and r["tree_ok"]
print(f"{label}: deposits {len(got_d)} found / {len(want_d)} true, claimable {sum(1 for x in got_d if x[3]=='claimable')}; "
      f"exits {len(got_e)} found / {len(want_e)} true; tree_ok {r['tree_ok']}; warnings {r['warnings']}")
if not ok:
    print("  missing deposits", want_d - got_d, "\n  extra deposits", got_d - want_d)
    print("  missing exits", want_e - got_e, "\n  extra exits", got_e - want_e)
    sys.exit(1)
print(f"{label}: MATCHES THE TRUTH")
PY
}

say "DELETE the client cache; recover from the wallet alone (full LEZ block scan from block 1)"
rm -rf "$R/wallet/bridge-client"
[ ! -e "$R/wallet/bridge-client" ] || die "cache still there"
start=$(date +%s)
$T recover --bridge $BP $CH | tee "$R/recover-1.txt"
echo "recover (human) took $(( $(date +%s) - start )) s"
$T recover --bridge $BP $CH --json > "$R/recover-1.json"
compare "$R/recover-1.json" "" "wallet alone, no --eth-dest"
$T recover --bridge $BP $CH --eth-dest $USER_DEST --json > "$R/recover-1u.json"
compare "$R/recover-1u.json" with-user-dest "wallet alone + the user's chosen address"
say "point-query mode (faster, reveals nullifier PDAs to the sequencer, skips exits)"
rm -rf "$R/wallet/bridge-client"
start=$(date +%s)
$T recover --bridge $BP $CH --lez-mode point --json > "$R/recover-point.json"
echo "point mode took $(( $(date +%s) - start )) s"
python3 - "$R/recover-1.json" "$R/recover-point.json" <<'PY'
import json, sys
a, b = (json.load(open(p)) for p in sys.argv[1:])
key = lambda r: sorted((d["account"], d["key_index"], d["denom"], d["state"]) for d in r["deposits"])
assert key(a) == key(b), (key(a), key(b))
print("point mode: same deposits and claim states as the block scan;", b["warnings"][-1])
PY
$T positions --bridge $BP

say "claim a recovered deposit (P1, denomination 0, index 1) with nothing but the key index"
attest_now
claim $P1 1 0
$T recover --bridge $BP $CH --eth-dest $USER_DEST --json > "$R/recover-2.json"
sed -i.bak "s/^dep\t$P1\t1\t100000000\tclaimable$/dep\t$P1\t1\t100000000\tclaimed/" "$TRUTH"
compare "$R/recover-2.json" with-user-dest "after claiming the recovered deposit"

say "a SECOND wallet restored from the recovery phrase alone; no cache"
export LEE_WALLET_HOME_DIR=$R/wallet2
start=$(date +%s)
$T wallet restore-keys --depth ${RESTORE_DEPTH:-4} --yes < "$R/mnemonic.txt" > "$R/restore.log" 2>&1 || { tail -20 "$R/restore.log"; die "restore-keys"; }
echo "restore-keys took $(( $(date +%s) - start )) s"
$T wallet account list > "$R/wallet2-accounts.txt" 2>&1 || true
grep -c Public "$R/wallet2-accounts.txt" || true
grep -q "$P3" "$R/wallet2-accounts.txt" && echo "P3 kept by the restore" || echo "P3 not regenerated by the restore: --account-gap must find it"
$T recover --bridge $BP $CH --account-gap 0 --json > "$R/recover-w2-gap0.json" || true
python3 -c "
import json,sys; r=json.load(open('$R/recover-w2-gap0.json')); print('restored wallet, --account-gap 0:', len(r['deposits']), 'deposits found')" || true
$T recover --bridge $BP $CH --eth-dest $USER_DEST --json > "$R/recover-w2.json"
compare "$R/recover-w2.json" with-user-dest "restored from the phrase alone (--account-gap 5)"

say "no file this script wrote holds a derived secret's hex (only mnemonic.txt holds the phrase)"
grep -rl "secret" "$R"/*.json 2>/dev/null && die "a JSON names a secret" || echo "no 'secret' field in any JSON output"
say "done"
