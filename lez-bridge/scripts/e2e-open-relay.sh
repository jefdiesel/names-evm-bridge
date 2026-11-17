#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Open claim relay and sponsored private burn (branch open-relay, 2026-11-17), on a THROWAWAY
# loopback LEZ rc3 chain and a local anvil (build host only; fresh keys only; never the public demo
# chain, its relayer or Sepolia).
#
#   R=<new dir> SEQ_BIN=<rc3 sequencer_service> NETCFG=<rc3 networks/local.json>
#   CLI=<lez-bridge built from this branch (its guests embedded)> VAULT_DIR=<vault/ with lib/>
#   GUARD=<lez-bridge/scripts/guard.sh>
#   RPC=127.0.0.1:3490 METRICS=127.0.0.1:3491 RELAY1_PORT=3492 RELAY2_PORT=3493 ANVIL_PORT=8590
#   e2e-open-relay.sh [run|stop]
#
# DEV MODE: the sequencer and the CLI run with RISC0_DEV_MODE=1, so the two private transactions
# (part B: a private claim and the sponsored private burn) carry dev receipts, not proofs: their
# public outputs are real, their receipts prove nothing. Public transactions (part A) are unaffected
# (LEZ 0.4 proves no public execution). Every private send runs under guard.sh with a 3 GB footprint
# cap and an alarm, so a missed flag cannot turn into a real proof. RISC0_PROVER is unset.
#
# A. Open claim relay: two relayer.py instances, each with its own payer key, wallet, port, state
#    and log, nothing registered anywhere; the bridge refunds a relayed claim's payer from its float.
#    A1 a never-used account claims through relay 2 (refunded); A2 another through relay 1; A3 relay 1
#    stopped, the client's list (LEZ_BRIDGE_RELAYS: relay 1, relay 2) falls over to relay 2; A4 a
#    relayed claim that reverts on chain (minting frozen in the same block) refunds nothing; A5 the
#    repeat refused. The float and both payers' balances before and after each step.
# B. Sponsored private burn: a private account claims privately (one-time recipient, sponsored),
#    then burns with `burn --sponsored`: a fresh one-time private account receives the burn sponsor
#    from the float inside the transaction and pays the fee. The burn block is searched for the
#    burner's account id (must be absent) and the one-time account (present).
set -uo pipefail
: "${R:?}" "${SEQ_BIN:?}" "${NETCFG:?}" "${CLI:?}" "${VAULT_DIR:?}" "${GUARD:?}"
RPC=${RPC:-127.0.0.1:3490} METRICS=${METRICS:-127.0.0.1:3491}
RELAY1_PORT=${RELAY1_PORT:-3492} RELAY2_PORT=${RELAY2_PORT:-3493} ANVIL_PORT=${ANVIL_PORT:-8590}
for p in "$RPC" "$METRICS" "127.0.0.1:$RELAY1_PORT" "127.0.0.1:$RELAY2_PORT" "127.0.0.1:$ANVIL_PORT"; do
  case "$p" in *:349[0-5]|*:8590) ;; *) echo "refusing port $p (this run uses 3490-3495 and 8590)"; exit 1 ;; esac
done
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT_DIR=$(cd "$HERE/../.." && pwd)
unset RISC0_PROVER RISC0_SERVER_PATH BONSAI_API_URL BONSAI_API_KEY
export RISC0_DEV_MODE=1 LEZ_BRIDGE_SEQUENCER=http://$RPC LEE_WALLET_PASSWORD= PATH=$HOME/.foundry/bin:$PATH
mkdir -p "$R/run" "$R/wallets" "$R/logs" "$R/relay1" "$R/relay2"; chmod 700 "$R/wallets"
OUT=$R/run
say() { echo; echo "== $* $(date +%T)"; }
show() { grep -vE "^Transaction data" "$1" | cut -c1-260 | tail -"${2:-5}"; }
step() { local n=$1; shift; "$@" > "$OUT/$n.out" 2>&1; local rc=$?; echo "[$n] exit $rc"; show "$OUT/$n.out" "${SHOWN:-4}"; return $rc; }
FAILS=0
fail() { echo "  !! UNEXPECTED: $*"; FAILS=$((FAILS + 1)); }
must() { local n=$1; shift; step "$n" "$@" || fail "$n did not apply"; }
rpc() { curl -s -m 10 -X POST "http://$RPC/" -H 'Content-Type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":$2}"; }
idof() { grep -oE "$1/[1-9A-HJ-NP-Za-km-z]{32,44}" | sed -n 1p | cut -d/ -f2; }
newacct() { local kind=$1; shift; "$@" wallet account new "$kind" > "$OUT/new.tmp" 2>&1; idof "$( [ "$kind" = public ] && echo Public || echo Private)" < "$OUT/new.tmp"; }
pkof() { sed -n 's/^With pk //p' "$OUT/new.tmp" | sed -n 1p; }
# a public account's native balance (0 when absent), and whether it exists
nat() { rpc getAccount "[\"$1\"]" | /usr/bin/python3 -c '
import json,sys
r=json.load(sys.stdin).get("result")
raw=(((r or {}).get("data") or {}).get("actor_states") or {}).get("11111111111111111111111111111111") or []
print(int.from_bytes(bytes(raw),"little"))'; }
present() { rpc getAccount "[\"$1\"]" | /usr/bin/python3 -c 'import json,sys; r=json.load(sys.stdin).get("result"); print("absent" if r is None else "present, nonce %s" % r.get("nonce"))'; }
height() { rpc getLastBlockId '[]' | sed -n 's/.*"result":\([0-9]*\).*/\1/p'; }
block_of() { sed -nE 's/.*(applied|included) in block ([0-9]+).*/\2/p' "$1" | tail -1; }
IDS=$OUT/ids.env; touch "$IDS"; . "$IDS"
save() { echo "$1=$2" >> "$IDS"; eval "$1=$2"; }
ETH_RPC=http://127.0.0.1:$ANVIL_PORT
ANVIL_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80   # anvil's public test key 0

stop_all() {
  kill $(lsof -tiTCP:$RELAY1_PORT -sTCP:LISTEN) $(lsof -tiTCP:$RELAY2_PORT -sTCP:LISTEN) 2>/dev/null
  for p in "$R/relay1/pid" "$R/relay2/pid" "$R/anvil.pid"; do [ -f "$p" ] && kill "$(cat "$p")" 2>/dev/null; rm -f "$p"; done
  R=$R SEQ_BIN=$SEQ_BIN CLI=$CLI NETCFG=$NETCFG RPC=$RPC METRICS=$METRICS "$ROOT_DIR/public-demo/rc3/chain-rc3.sh (not published)" stop
}
[ "${1:-run}" = stop ] && { stop_all; exit 0; }

# ---- chain (dev mode: RISC0_DEV_MODE=1 is in the sequencer's environment) and anvil
if [ ! -d "$R/home" ]; then
  R=$R SEQ_BIN=$SEQ_BIN CLI=$CLI NETCFG=$NETCFG RPC=$RPC METRICS=$METRICS "$ROOT_DIR/public-demo/rc3/chain-rc3.sh (not published)" setup || exit 1
fi
R=$R SEQ_BIN=$SEQ_BIN CLI=$CLI NETCFG=$NETCFG RPC=$RPC METRICS=$METRICS "$ROOT_DIR/public-demo/rc3/chain-rc3.sh (not published)" start || exit 1
. "$R/operator.env"
echo "sequencer environment: RISC0_DEV_MODE=$(ps eww -p "$(cat "$R/sequencer.pid")" | tr ' ' '\n' | sed -n 's/^RISC0_DEV_MODE=//p')"
if ! cast chain-id --rpc-url $ETH_RPC >/dev/null 2>&1; then
  nohup anvil --port $ANVIL_PORT --silent > "$R/logs/anvil.log" 2>&1 & echo $! > "$R/anvil.pid"; sleep 2
fi
W=$R/wallets/deployer U=$R/wallets/user OPW=$R/wallets/operator
for w in "$W" "$U" "$R/wallets/relay1" "$R/wallets/relay2"; do mkdir -p "$w"; chmod 700 "$w"; done
d()  { LEE_WALLET_HOME_DIR=$W "$CLI" "$@"; }
u()  { LEE_WALLET_HOME_DIR=$U "$CLI" "$@"; }
op() { LEE_WALLET_HOME_DIR=$OPW "$CLI" "$@"; }
# a private send: dev receipts, under the guard (3 GB footprint cap) and a 300 s alarm
upriv() { LEE_WALLET_HOME_DIR=$U MAX_FOOTPRINT_GB=3 GUARD_LOG=$R/logs/guard-$1.log perl -e 'alarm 300; exec @ARGV' "$GUARD" "$CLI" "${@:2}"; }

say "accounts: deployer, two relay payers (own wallets), never-used claimers X Q Y Z (public), V (private)"
if [ -z "${D:-}" ]; then
  d wallet check-health >/dev/null 2>&1; save D "$(newacct public d)"; DPK=$(pkof)
  must fund-d op wallet auth-transfer send --from Public/$OP --to-pk $DPK --amount 100000000000
  for i in 1 2; do
    LEE_WALLET_HOME_DIR=$R/wallets/relay$i "$CLI" wallet check-health >/dev/null 2>&1
    save P$i "$(newacct public env LEE_WALLET_HOME_DIR=$R/wallets/relay$i "$CLI")"; PK=$(pkof)
    must fund-p$i op wallet auth-transfer send --from Public/$OP --to-pk $PK --amount 100000000
  done
  u wallet check-health >/dev/null 2>&1
  for n in X Q Y Z; do save $n "$(newacct public u)"; done
  save V "$(newacct private u)"
fi
echo "deployer $D | relay payers P1 $P1, P2 $P2 | claimers X $X Q $Q Y $Y Z $Z | private V $V"

say "programs (this branch's guests, embedded in the CLI), vault on anvil, Initialize by the header key"
if [ -z "${BP:-}" ]; then
  must dep-tok d deploy --which token --payer $D
  save TP "$(sed -n 's/^program //p' "$OUT/dep-tok.out")"; save TIMG "$(sed -n 's/^image id //p' "$OUT/dep-tok.out")"
  must dep-br d deploy --which bridge --payer $D
  save BP "$(sed -n 's/^program //p' "$OUT/dep-br.out")"; save BIMG "$(sed -n 's/^image id //p' "$OUT/dep-br.out")"
  must wrap d create-wrapped --token-program $TP --bridge $BP --name "Wrapped USDC (open-relay, throwaway)" --payer $D
  save DEF "$(sed -n 's/^token //p' "$OUT/wrap.out")"
fi
echo "bridge $BP image $BIMG | token $TP image $TIMG | wrapped $DEF"
if [ -z "${VAULT:-}" ]; then
  CHAIN=$(d chain-id | sed -n 's/^LEZ_CHAIN_ID=//p'); ZONE=$("$CLI" source-id | sed -n 's/^ZONE_ID=//p')
  PROGRAM_ID=$("$CLI" program-id --bridge $BP | sed -n 's/^PROGRAM_ID=//p')
  o=$(cd "$VAULT_DIR" && LEZ_CHAIN_ID=$CHAIN ZONE_ID=$ZONE PROGRAM_ID=$PROGRAM_ID \
    forge script script/LocalDeploy.s.sol --rpc-url $ETH_RPC --private-key $ANVIL_KEY --broadcast 2>&1) || { echo "$o" | tail -20; exit 1; }
  save VAULT "$(echo "$o" | sed -n 's/.*VAULT=\(0x[0-9a-fA-F]*\).*/\1/p' | sed -n 1p)"
  save USDC "$(echo "$o" | sed -n 's/.*USDC=\(0x[0-9a-fA-F]*\).*/\1/p' | sed -n 1p)"
  save CFG "$(echo "$o" | sed -n 's/.*CFG_ID=\(0x[0-9a-fA-F]*\).*/\1/p' | sed -n 1p)"
  EVM=$(cast chain-id --rpc-url $ETH_RPC)
  must init d init --bridge $BP --token-program $TP --admin $D --vault $VAULT --chain-id $EVM --freezer $D --attestor $D --payer $D
  must reg d register --bridge $BP --admin $D --eth-token $USDC --definition $DEF --decimals 6 --denoms 100000000,1000000000
fi
echo "vault $VAULT usdc $USDC cfg $CFG"

say "float and amounts: claim sponsor 25,000,000; relay refund 1,300,000 (cap $(grep -o 'MAX_RELAY_REFUND: Balance = [0-9_]*' "$ROOT_DIR/lez-bridge/bridge/src/lib.rs" | sed 's/.*= //')); burn sponsor 6,500,000 (cap); claims' float 1,000,000,000; burn float 100,000,000 (its own account)"
if [ -z "${CONFIGURED:-}" ]; then
  must sponsor d set-sponsor --bridge $BP --admin $D --amount 25000000
  must refund d set-relay-refund --bridge $BP --admin $D --amount 1300000
  must bsponsor d set-burn-sponsor --bridge $BP --admin $D --amount 6500000
  must float op fund-float --bridge $BP --from $OP --amount 1000000000
  must bfloat op fund-float --bridge $BP --from $OP --amount 100000000 --burns
  save CONFIGURED 1
fi
d show --bridge $BP 2>&1 | grep -E '^(sponsor_amount|relay_refund|burn_sponsor|sponsored_burns|float|burn_float)'
SHOWN=2 step refund-over d --allow-revert set-relay-refund --bridge $BP --admin $D --amount 2500001
grep -q "but reverted" "$OUT/refund-over.out" && echo "a refund above the program's cap: included and reverted" || fail "the cap did not refuse"

say "Ethereum: one 100 USDC deposit each for X, Q, Y, Z (public) and V (private); one attested root"
if [ ! -f "$OUT/k.env" ]; then ( umask 077; for n in X Q Y Z V; do echo "K$n=0x$(openssl rand -hex 32)"; done > "$OUT/k.env" ); fi
. "$OUT/k.env"
if [ -z "${DEPOSITED:-}" ]; then
  for n in X Q Y Z V; do
    k=K$n
    c=$("$CLI" commitment --secret ${!k} --dest ${!n} --eth-token $USDC --denom 100000000 --cfg-id $CFG | grep -oE '0x[0-9a-f]{64}' | sed -n 1p)
    cast send $VAULT "deposit(address,uint32,bytes32)" $USDC 0 $c --private-key $ANVIL_KEY --rpc-url $ETH_RPC >/dev/null || exit 1
  done
  save DEPOSITED 1
fi
cast logs --from-block 0 --address $VAULT "Deposit(address indexed,uint32,bytes32,uint64,bytes32)" --rpc-url $ETH_RPC --json \
  | /usr/bin/python3 -c '
import json,sys
for l in json.load(sys.stdin):
    d=l["data"][2:]; print(l["topics"][1][-40:], int(d[0:64],16), "0x"+d[64:128])' > "$OUT/deposits.txt"
: > "$OUT/leaves.txt"
while read -r tok di c; do
  cast call $VAULT "depositLeaf(address,uint256,bytes32)(bytes32)" 0x$tok 100000000 $c --rpc-url $ETH_RPC >> "$OUT/leaves.txt"
done < "$OUT/deposits.txt"
ROOT=$(cast call $VAULT "depositRoot()(bytes32)" --rpc-url $ETH_RPC)
echo "$(wc -l < "$OUT/leaves.txt" | tr -d ' ') vault leaves, root $ROOT"
[ -n "${ATTESTED:-}" ] || { must attest d attest --bridge $BP --attestor $D --root $ROOT; save ATTESTED 1; }

say "two independent relays: own payer, wallet, port, state, log; REQUIRE_REFUND=1; nothing registered"
RD=$ROOT_DIR/public-demo/relayer
relayer() { local i=$1; shift; local port; port=$([ "$i" = 1 ] && echo $RELAY1_PORT || echo $RELAY2_PORT); local payer=P$i
  P=$R CLI=$CLI RELAY_WALLET=$R/wallets/relay$i SEQUENCER=http://$RPC RELAY_LISTEN=127.0.0.1:$port BRIDGE=$BP TOKEN_PROGRAM=$TP \
  PAYER=${!payer} RELAY_STATE=$R/relay$i/state.json RELAY_LOG=$R/logs/relayer$i.log REQUIRE_REFUND=1 RISC0_DEV_MODE= \
  /usr/bin/python3 -I "$RD/relayer.py" "$@"; }
start_relay() { local i=$1 port; port=$([ "$i" = 1 ] && echo $RELAY1_PORT || echo $RELAY2_PORT)
  [ -f "$R/wallets/relay$i/relay-chain.json" ] || { relayer $i --bind-payer > "$OUT/bind$i.out" 2>&1; show "$OUT/bind$i.out" 1; }
  curl -s -m 2 "http://127.0.0.1:$port/relay/info" >/dev/null && return 0
  relayer $i > "$R/logs/relayer$i.out" 2>&1 < /dev/null &
  echo $! > "$R/relay$i/pid"; sleep 2
  echo "relay $i: $(curl -s -m 5 "http://127.0.0.1:$port/relay/info")"; }
start_relay 1; start_relay 2
R1=http://127.0.0.1:$RELAY1_PORT/relay R2=http://127.0.0.1:$RELAY2_PORT/relay
claim() { local who=$1 k=$2 i=$3; shift 3
  u claim --bridge $BP --token-program $TP --from ${!who} --eth-token $USDC --denom-index 0 --denom 100000000 \
    --secret ${!k} --cfg-id $CFG --leaves "$OUT/leaves.txt" --index $i "$@"; }
FLOAT() { nat "$("$CLI" program-id --bridge $BP | sed -n 's/^CONFIG=//p')"; }
BFLOAT() { d show --bridge $BP 2>/dev/null | sed -n 's/^burn_float //p'; }
TABLE=$OUT/balances.tsv; [ -f "$TABLE" ] || printf 'step\tclaims float\tburn float\tP1\tP2\tclaimer native\tblock\n' > "$TABLE"
row() { printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$1" "$(FLOAT)" "$(BFLOAT)" "$(nat $P1)" "$(nat $P2)" "$2" "$3" >> "$TABLE"; }
row start - -

say "A1: X (never used: $(present $X)) claims through relay 2; the bridge refunds relay 2's payer"
SHOWN=7 must a1 claim X KX 0 --relay $R2; sleep 1
row "A1 X via relay 2" "$(nat $X)" "$(block_of "$OUT/a1.out")"
say "A2: Q claims through relay 1"
SHOWN=5 must a2 claim Q KQ 1 --relay $R1; sleep 1
row "A2 Q via relay 1" "$(nat $Q)" "$(block_of "$OUT/a2.out")"
say "A3: relay 1 stopped; Y's client uses its configured list (relay 1, then relay 2)"
kill $(lsof -tiTCP:$RELAY1_PORT -sTCP:LISTEN) 2>/dev/null; rm -f "$R/relay1/pid"; sleep 1
curl -s -m 2 "$R1/info" >/dev/null && fail "relay 1 still answers"
SHOWN=8 LEZ_BRIDGE_RELAYS="$R1,$R2" must a3 claim Y KY 2 --relay config; sleep 1
grep -q "trying the next relay" "$OUT/a3.out" && echo "the client fell over from relay 1 to relay 2" || fail "no failover seen"
row "A3 Y, relay 1 down, via relay 2" "$(nat $Y)" "$(block_of "$OUT/a3.out")"
say "A4: Z's relayed claim lands in the same block as a mint freeze sent just before it: included, reverted, no refund"
h0=$(height); while [ "$(height)" = "$h0" ]; do sleep 0.2; done
( d freeze --bridge $BP --by $D --mint > "$OUT/a4-freeze.out" 2>&1 ) &
FREEZE=$!; sleep 0.3
SHOWN=6 step a4 claim Z KZ 3 --relay $R2
wait $FREEZE; show "$OUT/a4-freeze.out" 2
grep -q "but reverted" "$OUT/a4.out" && echo "Z's relayed claim was included and reverted" || echo "note: the freeze did not land first (see a4.out)"
sleep 1; row "A4 Z reverted (freeze)" "$(nat $Z)" "$(block_of "$OUT/a4.out")"
must a4-unfreeze d unfreeze --bridge $BP --admin $D --mint
say "A5: the repeat of X's claim: refused by the CLI, and by relay 2 itself (--no-precheck)"
SHOWN=2 step a5 claim X KX 0 --relay $R2 && fail "repeat applied"
SHOWN=2 step a5-relay claim X KX 0 --relay $R2 --no-precheck && fail "repeat applied"
row "A5 repeats refused" - -

say "B0: a PUBLIC burner (X, given 30,000,000 native to pay its own fee) names a public account's bridge actor (Q's) as notify: refused"
must b0-fund op wallet auth-transfer send --from Public/$OP --to Public/$X --amount 30000000
CONFIG=$("$CLI" program-id --bridge $BP | sed -n 's/^CONFIG=//p')
BHEX=$(/usr/bin/python3 -I - "$DEF" "$Q" "$BP" <<'PY'
import sys
A = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
def b58(s):
    n = 0
    for ch in s: n = n * 58 + A.index(ch)
    return n.to_bytes(32, "big")
d, q, bp = sys.argv[1:4]
# token Message::BurnWithNotice (variant 10) { definition, amount: 100 dUSDC, notify: Actor(Q, bridge), memo }
print((bytes([10]) + b58(d) + (100000000).to_bytes(16, "little") + b58(q) + b58(bp) + bytes([0x4D]) * 32).hex())
PY
)
f0=$(FLOAT)
burnq() { u "$@" raw-call --program $TP --signer $X --message-hex $BHEX --also "$DEF@$TP,$Q@$BP,$CONFIG@$BP" --payer $X; }
burnq > "$OUT/b0-sim.out" 2>&1 && fail "the public forwarder's burn applied"
grep -q "a sponsored burn pays a private one-time account only" "$OUT/b0-sim.out" && echo "[b0-sim] the wallet's simulation refused it with the program's text" || { echo "[b0-sim] refused:"; show "$OUT/b0-sim.out" 3; }
burnq --allow-revert > "$OUT/b0.out" 2>&1; show "$OUT/b0.out" 2
grep -q "but reverted" "$OUT/b0.out" && echo "included and reverted on chain; float $f0 -> $(FLOAT)" || fail "the public forwarder's burn was not refused on chain"
row "B0 public forwarder refused" "X $(nat $X)" "$(block_of "$OUT/b0.out")"

say "B: V claims privately (one-time recipient, sponsored by the float), DEV MODE"
SHOWN=6 must b1 upriv b1 claim --bridge $BP --token-program $TP --from $V --eth-token $USDC --denom-index 0 --denom 100000000 \
  --secret $KV --cfg-id $CFG --leaves "$OUT/leaves.txt" --index 4
row "B1 V private claim (dev)" - "$(block_of "$OUT/b1.out")"
u balances --token-program $TP 2>&1 | grep -v '^Transaction' | sed -n 1,8p > "$OUT/b1-balances.txt"; cat "$OUT/b1-balances.txt"
# a private account's native balance, from this wallet (synced first)
pnat() { u wallet account sync-private >/dev/null 2>&1; u wallet account get --account-id Private/$1 2>&1 | sed -n 's/^Balance \([0-9]*\),.*/\1/p'; }
VB1=$(pnat $V); echo "V's native LEZ after its sponsored claim (sponsor minus the claim's fee): $VB1"
say "B2: V burns 100 with --sponsored, DEV MODE: a fresh one-time account takes the burn sponsor and pays the fee"
SHOWN=8 must b2 upriv b2 burn --bridge $BP --token-program $TP --from $V --definition $DEF --amount 100000000 \
  --exit 0x$(openssl rand -hex 32) --sponsored
WONE=$(sed -n 's/^one-time fee account \([1-9A-HJ-NP-Za-km-z]*\).*/\1/p' "$OUT/b2.out")
B2=$(block_of "$OUT/b2.out"); row "B2 V sponsored burn (dev)" "W $WONE" "$B2"
VB2=$(pnat $V); WB2=$(pnat $WONE)
echo "V's native LEZ before the burn $VB1, after $VB2 (unchanged: V paid no fee); one-time W after: $WB2 (burn sponsor minus the burn's fee)"
[ "$(BFLOAT)" = 93500000 ] && echo "the burn float paid exactly 6,500,000; the claims' float did not move in B2" || fail "burn float $(BFLOAT)"
[ "$VB1" = "$VB2" ] || fail "the burner's native balance moved"
u balances --token-program $TP 2>&1 | grep -v '^Transaction' | sed -n 1,8p
say "B3: the burn block $B2: the burner V must be absent, the one-time account present"
rpc getBlock "[$B2]" > "$OUT/b2-block.json"
/usr/bin/python3 - "$OUT/b2-block.json" "$V" "$WONE" "$P1" "$P2" "$D" <<'PY' | tee "$OUT/b3-privacy.txt"
import base64, json, sys
A = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
def b58(s):
    n = 0
    for ch in s: n = n * 58 + A.index(ch)
    return n.to_bytes(32, "big")
r = json.load(open(sys.argv[1]))["result"]
raw = base64.b64decode(r) if isinstance(r, str) else json.dumps(r).encode()
print("block bytes", len(raw))
for name, acct in zip(["burner V", "one-time W", "relay payer P1", "relay payer P2", "deployer"], sys.argv[2:]):
    print("%-15s %s %s" % (name, acct[:8] + "…", "PRESENT" if b58(acct) in raw else "absent"))
PY
grep -q "burner V        .* absent" "$OUT/b3-privacy.txt" || fail "the burner appears in the burn block"
grep -q "one-time W      .* PRESENT" "$OUT/b3-privacy.txt" || fail "the positive control (W) was not found"

say "bridge state"
d show --bridge $BP 2>&1 | grep -E '^(sponsor_amount|relay_refund|burn_sponsor|sponsored_burns|float|burn_float|token)' | cut -c1-260
say "balances (native LEZ)"; column -t -s $'\t' "$TABLE"
say "relay logs"; for i in 1 2; do echo "-- relay $i"; cut -c1-300 "$R/logs/relayer$i.log" | grep -vE '^\s*$' | tail -8; done
say "done: $FAILS unexpected result(s). Stop: R=$R ... $0 stop; then delete $R"
exit $FAILS
