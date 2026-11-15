# SPDX-License-Identifier: MIT OR Apache-2.0
# Sourced by e2e-security.sh (fixed phase): a public relayed claim (`claim --relay`) on the FIXED
# programs, through public-demo/relayer/relayer.py with a new payer bound to this chain. A third
# vault deposit on anvil for a never-used account X (no native LEZ), a new stand-in root, the
# relayed claim, the repeat refused. Uses the fixed phase's variables and helpers.
RD=$ROOT_DIR/public-demo/relayer
RW=$R/wallets/relay
rw() { LEE_WALLET_HOME_DIR=$RW "$CLI" "$@"; }
if [ -z "${F_RP:-}" ]; then
  rw wallet check-health >/dev/null 2>&1; save F_RP "$(newacct rw)"; RPK=$(pkof)
  u wallet check-health >/dev/null 2>&1; save F_X "$(newacct u)"
  step r-fund op wallet auth-transfer send --from Public/$OP --to-pk $RPK --amount 100000000 || return 1
fi
relayer() { P=$R CLI=$CLI RELAY_WALLET=$RW SEQUENCER=http://$RPC RELAY_LISTEN=127.0.0.1:$RELAY_PORT BRIDGE=$F_BP \
  TOKEN_PROGRAM=$F_TP PAYER=$F_RP RELAY_STATE=$R/relay/state.json RELAY_LOG=$R/logs/relayer.log /usr/bin/python3 -I "$RD/relayer.py" "$@"; }
mkdir -p "$R/relay"
[ -f "$RW/relay-chain.json" ] || { relayer --bind-payer > "$OUT/r-bind.out" 2>&1; echo "[r-bind] exit $?"; show "$OUT/r-bind.out" 2; }
if ! curl -s -m 2 "http://127.0.0.1:$RELAY_PORT/relay/info" >/dev/null; then
  lsof -nP -iTCP:$RELAY_PORT -sTCP:LISTEN >/dev/null 2>&1 && { echo "port $RELAY_PORT busy"; return 1; }
  relayer > "$R/logs/relayer.out" 2>&1 < /dev/null &
  echo $! > "$R/relay/pid"; sleep 2
fi
curl -s -m 5 "http://127.0.0.1:$RELAY_PORT/relay/info"; echo
if [ -z "${F_DEP3:-}" ]; then
  ( umask 077; echo "K3=0x$(openssl rand -hex 32)" >> "$OUT/k.env" ); . "$OUT/k.env"
  C3=$("$CLI" commitment --secret $K3 --dest $F_X --eth-token $F_USDC --denom 100000000 --cfg-id $F_CFG | grep -oE '0x[0-9a-f]{64}' | sed -n 1p)
  cast send $F_VAULT "deposit(address,uint32,bytes32)" $F_USDC 0 $C3 --private-key $ANVIL_KEY --rpc-url $ETH_RPC >/dev/null || return 1
  save F_DEP3 1
fi
. "$OUT/k.env"
cast logs --from-block 0 --address $F_VAULT "Deposit(address indexed,uint32,bytes32,uint64,bytes32)" --rpc-url $ETH_RPC --json \
  | /usr/bin/python3 -c '
import json,sys
for l in json.load(sys.stdin):
    d=l["data"][2:]; print(l["topics"][1][-40:], int(d[0:64],16), "0x"+d[64:128])' > "$OUT/deposits.txt"
: > "$OUT/leaves.txt"
while read -r tok di c; do
  den=$([ "$di" = 0 ] && echo 100000000 || echo 1000000000)
  cast call $F_VAULT "depositLeaf(address,uint256,bytes32)(bytes32)" 0x$tok $den $c --rpc-url $ETH_RPC >> "$OUT/leaves.txt"
done < "$OUT/deposits.txt"
ROOT3=$(cast call $F_VAULT "depositRoot()(bytes32)" --rpc-url $ETH_RPC)
step r-attest d attest --bridge $F_BP --attestor $F_D --root $ROOT3
nat() { rpc getAccount "[\"$1\"]" | /usr/bin/python3 -c '
import json,sys
r=json.load(sys.stdin).get("result")
if r is None: print("ABSENT")
else:
    raw=((r.get("data") or {}).get("actor_states") or {}).get("11111111111111111111111111111111") or []
    print("nonce=%s native=%d" % (r.get("nonce"), int.from_bytes(bytes(raw),"little")))'; }
echo "X before: $(nat $F_X); payer: $(nat $F_RP)"
rclaim() { u claim --bridge $F_BP --token-program $F_TP --from $F_X --eth-token $F_USDC --denom-index 0 --denom 100000000 \
  --secret $K3 --cfg-id $F_CFG --leaves "$OUT/leaves.txt" --index 2 --relay http://127.0.0.1:$RELAY_PORT/relay "$@"; }
SHOWN=6 must r-claim rclaim
sleep 1; echo "X after: $(nat $F_X); payer: $(nat $F_RP)"
u balances --token-program $F_TP 2>&1 | grep -v '^Transaction' | sed -n 1,6p
clirefuses r-repeat "already claimed" rclaim
clirefuses r-repeat-relay "refused" rclaim --no-precheck
kill $(lsof -tiTCP:$RELAY_PORT -sTCP:LISTEN) "$(cat "$R/relay/pid")" 2>/dev/null
grep -c . "$R/logs/relayer.log" | sed 's/^/relayer log lines: /'
return 0
