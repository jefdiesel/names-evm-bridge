#!/bin/bash
# More recovery checks on the deployment scripts/e2e-recovery.sh left in $R (run it first):
#   A. a PRIVATE account: derived deposit request, deposit, recover lists it, claim dry run
#      (simulated, nothing proven or sent: no proving in this test);
#   B. `eth-key` exports the seed-derived release key of the exit released in e2e-recovery.sh;
#      the address matches, and the key's hex appears in no other file of the run;
#   C. the legacy raw path (`commitment`, `claim --secret --leaves --index`, `exit-commitment`,
#      `burn --exit`) still works with this binary, and recovery is unchanged by it;
#   D. `claim-attested --key-index` parses, and refuses a stand-in bridge with its own text;
#   E. a THIRD wallet restored from the phrase with a shallow tree (`restore-keys --depth 2`):
#      without `--account-gap` the accounts it did not regenerate are missed, with it they are found.
#
#   R=run/recovery1 LEZ_BRIDGE_BIN=... scripts/e2e-recovery-extra.sh
set -euo pipefail
: "${R:?set R to the run directory of scripts/e2e-recovery.sh}"
: "${LEZ_BRIDGE_BIN:?set LEZ_BRIDGE_BIN}"
HERE=$(cd "$(dirname "$0")/.." && pwd)
export PATH=$HOME/.foundry/bin:$PATH
export LEE_WALLET_HOME_DIR=$R/wallet
export LEZ_BRIDGE_SEQUENCER=${SEQ:-http://127.0.0.1:3250}
export LEZ_BRIDGE_ETH_RPC=${ETH_RPC:-http://127.0.0.1:8550}
export RISC0_PROVER=local RAYON_NUM_THREADS=4
ETH_RPC=$LEZ_BRIDGE_ETH_RPC
T="nice -n 10 $LEZ_BRIDGE_BIN"
EK=$("$HERE/scripts/anvil-dev-key.sh" 5) || exit 1   # anvil account #5
USER_DEST=0x000000000000000000000000000000000000bEEF
say() { echo; echo "## $(date +%T) $*"; }
die() { echo "FAILED: $*" >&2; exit 1; }
trap 'echo "FAILED at line $LINENO" >&2' ERR
id_of() { grep -oE "$1/[1-9A-HJ-NP-Za-km-z]{32,44}" | sed -n 1p | cut -d/ -f2; }
kv() { sed -n "s/^$1=//p" | tail -1; }
# Plain shell variables, never exported: Foundry would read ETH_FROM as the sender.
eval "$(cat "$R/ids.env")"
. "$R/exits.env"
TRUTH=$R/truth.tsv
CH="--eth-from-block $ETH_FROM"
attest_now() { $T attest --bridge $BP --attestor $F --root $(cast call $VAULT "depositRoot()(bytes32)" --rpc-url $ETH_RPC); }
compare() { # $1 recover json, $2 label
  python3 - "$1" "$TRUTH" "$2" <<'PY'
import json, sys
B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
def b58(s):
    n = 0
    for c in s: n = n * 58 + B58.index(c)
    return "0x" + n.to_bytes(32, "big").hex()
r = json.load(open(sys.argv[1])); label = sys.argv[3]
want_d, want_e = set(), set()
for line in open(sys.argv[2]):
    k, a, j, d, st = line.split()
    if k == "dep": want_d.add((b58(a), int(j), d, st))
    else: want_e.add((b58(a), int(j), "derived" if d == "derived" else d.lower(), st))
got_d = {(x["account"], x["key_index"], x["denom"], x["state"]) for x in r["deposits"]}
st = {"burned": "burned", "unreleased": "tree", "released": "released"}
got_e = {(x["account"], x["key_index"], "derived" if x["dest_derived"] else x["eth_dest"].lower(), st[x["state"]]) for x in r["exits"]}
print(f"{label}: deposits {len(got_d)} found / {len(want_d)} true; exits {len(got_e)} found / {len(want_e)} true; tree_ok {r['tree_ok']}; warnings {r['warnings']}")
if got_d != want_d or got_e != want_e or not r["tree_ok"]:
    print("  missing deposits", want_d - got_d, "\n  extra deposits", got_d - want_d, "\n  missing exits", want_e - got_e, "\n  extra exits", got_e - want_e)
    sys.exit(1)
print(f"{label}: MATCHES THE TRUTH")
PY
}

say "A. private account V: derived deposit, recover, claim dry run"
V=$($T wallet account new private | id_of Private); echo "V=$V" >> "$R/extra.env"; echo "V=$V"
$T set-sponsor --bridge $BP --admin $F --amount 20000000
$T fund-float --bridge $BP --from $F --amount 100000000
out=$($T deposit-request --bridge $BP --from $V --eth-token $USDC --denom-index 0 $CH); echo "$out" | tail -3
CV=$(echo "$out" | kv COMMITMENT); JV=$(echo "$out" | kv KEY_INDEX)
[ "$JV" = 0 ] || die "V's first index is $JV"
cast send $VAULT "deposit(address,uint32,bytes32)" $USDC 0 $CV --private-key $EK --rpc-url $ETH_RPC >/dev/null
printf 'dep\t%s\t0\t100000000\tclaimable\n' "$V" >> "$TRUTH"
attest_now
$T recover --bridge $BP $CH --eth-dest $USER_DEST --json > "$R/recover-x1.json"
compare "$R/recover-x1.json" "with the private account's deposit"
$T --dry-run claim --bridge $BP --token-program $TP --from $V --eth-token $USDC --denom-index 0 --denom 100000000 --key-index 0 $CH | tee "$R/private-claim-dry.log"
grep -q "one-time address of $V" "$R/private-claim-dry.log" || die "no one-time recipient"
grep -q "pre-check passed" "$R/private-claim-dry.log" || die "pre-check"
grep -q "dry run: simulated" "$R/private-claim-dry.log" || die "the private claim did not simulate"

say "B. eth-key: the released exit's seed-derived key"
KEYF=$R/p2-release.key
rm -f "$KEYF"
$T eth-key --bridge $BP --from $P2 --eth-token $USDC --denom 100000000 --key-index 0 --out "$KEYF"
[ "$(stat -f %Lp "$KEYF")" = 600 ] || die "key file mode"
ADDR=$(cast wallet address --private-key $(cat "$KEYF"))
echo "key file address $ADDR, released-to address $DEST3"
[ "$(echo $ADDR | tr A-F a-f)" = "$(echo $DEST3 | tr A-F a-f)" ] || die "eth-key address differs"
KHEX=$(sed 's/^0x//' "$KEYF")
if grep -rl "$KHEX" "$R" "$R/../recovery1.e2e.log" 2>/dev/null | grep -v "^$KEYF$"; then die "the release key's hex is in another file"; fi
echo "the release key's hex is in no other file of the run or the e2e log"

say "C. legacy raw path: --secret / --exit with this binary"
K=$(cast keccak "legacy-$R"); KX=$(cast keccak "legacy-exit-$R")
C=$($T commitment --secret $K --dest $P1 --eth-token $USDC --denom 100000000 --cfg-id $CFG_ID)
cast send $VAULT "deposit(address,uint32,bytes32)" $USDC 0 $C --private-key $EK --rpc-url $ETH_RPC >/dev/null
cast logs --from-block $ETH_FROM --address $VAULT "Deposit(address indexed,uint32,bytes32,uint64,bytes32)" --rpc-url $ETH_RPC --json \
  | python3 -c '
import json,sys
for l in json.load(sys.stdin):
    d=l["data"][2:]; print(l["topics"][1][-40:], int(d[0:64],16), "0x"+d[64:128])' > "$R/legacy-deposits.txt"
: > "$R/legacy-leaves.txt"
while read -r tok di c; do
  cast call $VAULT "depositLeaf(address,uint256,bytes32)(bytes32)" 0x$tok $([ $di = 0 ] && echo 100000000 || echo 1000000000) $c --rpc-url $ETH_RPC >> "$R/legacy-leaves.txt"
done < "$R/legacy-deposits.txt"
IDX=$(( $(wc -l < "$R/legacy-leaves.txt") - 1 ))
attest_now
$T claim --bridge $BP --token-program $TP --from $P1 --eth-token $USDC --denom-index 0 --denom 100000000 --secret $K --cfg-id $CFG_ID --leaves "$R/legacy-leaves.txt" --index $IDX
E=$($T exit-commitment --secret $KX --eth-dest $USER_DEST --eth-token $USDC --denom 100000000 --max-relay-fee 2000000 --cfg-id $CFG_ID | sed -n 1p)
$T burn --bridge $BP --token-program $TP --from $P1 --definition $DEF --amount 100000000 --exit $E
$T recover --bridge $BP $CH --eth-dest $USER_DEST --json > "$R/recover-x2.json"
compare "$R/recover-x2.json" "after raw-secret activity (not derived: not ours to find)"

say "D. claim-attested --key-index parses and refuses a stand-in bridge"
if $T claim-attested --bridge $BP --token-program $TP --from $P1 --eth-token $USDC --denom-index 0 --denom 100000000 --key-index 1 $CH \
   --proof /dev/null --anchor-state-root 0x0000000000000000000000000000000000000000000000000000000000000001 2>&1 | tee "$R/claim-attested.log"; then
  die "claim-attested on a stand-in bridge went through"
fi
grep -q "stand-in attestor: use claim" "$R/claim-attested.log" || die "unexpected claim-attested error"

say "E. a THIRD wallet restored with a shallow tree (restore-keys --depth 2)"
export LEE_WALLET_HOME_DIR=$R/wallet3
start=$(date +%s)
$T wallet restore-keys --depth 2 --yes < "$R/mnemonic.txt" > "$R/restore3.log" 2>&1 || { tail -20 "$R/restore3.log"; die "restore-keys"; }
echo "restore-keys --depth 2 took $(( $(date +%s) - start )) s"
$T wallet account list > "$R/wallet3-accounts.txt" 2>&1 || true
for p in P1 P2 P3; do grep -q "${!p}" "$R/wallet3-accounts.txt" && echo "$p in the restored wallet" || echo "$p NOT in the restored wallet"; done
$T recover --bridge $BP $CH --eth-dest $USER_DEST --account-gap 0 --json > "$R/recover-w3-gap0.json"
if compare "$R/recover-w3-gap0.json" "shallow restore, --account-gap 0"; then echo "(gap 0 already finds everything: this restore regenerated every account)"; fi
$T recover --bridge $BP $CH --eth-dest $USER_DEST --account-gap 5 --json > "$R/recover-w3.json"
compare "$R/recover-w3.json" "shallow restore, --account-gap 5"
say "done"
