#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Branch v6-lc: an RFP-021 instance that follows Sepolia ACROSS sync-committee handoffs, in public
# transactions only (no proof anywhere), through the lez-bridge CLI of branch public-update.
# Every attempt (every INTERVAL s, default 1800):
#   1. if the store does not know its next committee: the store's own period update, in two
#      transactions (PeriodBegin, PeriodFinish); a Begin whose Finish did not land is finished next time;
#   2. a finality update when Sepolia's finality is past the store (signed by the current committee,
#      or by the next one once it is known: the rotation).
# Nothing is sent when nothing changed (the CLI refuses an update that does not advance the store and
# simulates every transaction before sending; a transaction whose simulation fails is not sent).
# One line per attempt in $L/logs/loop.log.
#
#   L=<dir> lc-handoff.sh newkey | deploy | init | once | start | stop | status | show | bal
# L holds: bin/lez-bridge (the public-update CLI), elf/attest_guest_public.bin, wallet/ (the payer,
# mode 700), run/ (ids.env, chain.env with SEQ, one directory per update), logs/.
set -uo pipefail
L=${L:?give L}
CLI=$L/bin/lez-bridge ELF=$L/elf/attest_guest_public.bin
RUN=$L/run LOG=$L/logs/loop.log PIDF=$L/run/loop.pid
mkdir -p "$RUN" "$L/logs" "$L/tmp"
touch "$RUN/chain.env" "$RUN/ids.env"; . "$RUN/chain.env"; . "$RUN/ids.env"
SEQ=${SEQ:?set SEQ in $RUN/chain.env}
INTERVAL=${INTERVAL:-1800}
FLOOR=${FLOOR:-400000000}        # > 4 full-limit fees (10M gas x exec base fee + bytes)
BEACON=${BEACON:-https://lodestar-sepolia.chainsafe.io}
BEACON_X=${BEACON_X:-https://ethereum-sepolia-beacon-api.publicnode.com}
ELF_SHA=${ELF_SHA:-2879adfa52e6ab316c261f67be6b73cba76ed095051abbea79ef836703652d2b}   # image a7ea6565…5b05
export LEZ_BRIDGE_SEQUENCER=$SEQ LEE_WALLET_PASSWORD= LEE_WALLET_HOME_DIR=$L/wallet TMPDIR=$L/tmp
save() { echo "$1=$2" >> "$RUN/ids.env"; eval "$1=$2"; }
line() { echo "$(date -u +%FT%TZ) $*" >> "$LOG"; }
q() { grep -vE '^(Transaction data|Stored)'; }
rpc() { curl -s -m 20 -X POST "$SEQ" -H 'Content-Type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":$2}"; }
bal() { rpc getAccountBalance "[\"$OP\"]" | /usr/bin/python3 -I -c 'import json,sys;print(json.load(sys.stdin)["result"])' 2>/dev/null; }
show() { "$CLI" attest show --program "$AP" --instance "$INSTANCE" 2>&1 | q; }
# from `attest show`: "<slot> <updates> <next: unknown|0x…>"
state() { show | /usr/bin/python3 -I -c '
import re,sys
t=sys.stdin.read()
s=re.search(r"finalized beacon slot (\d+)",t); u=re.search(r"updates (\d+)",t); n=re.search(r"next committee (\S+)",t)
print(s.group(1), u.group(1), n.group(1)) if s and u and n else print("")'; }
# "tx <hash> block <n> gas <g> fee <f>" from a CLI send log (the last transaction in it)
sent() { /usr/bin/python3 -I -c '
import re,sys
t=open(sys.argv[1]).read()
tx=re.findall(r"^tx ([0-9a-f]{64})$",t,re.M); b=re.findall(r"^applied in block (\d+)",t,re.M)
g=re.findall(r"execution gas = .*? = (\d+) in block",t); f=re.findall(r"charged: fee (\d+)",t)
print("tx",tx[-1] if tx else "?","block",b[-1] if b else "?","gas",g[-1] if g else "?","fee",f[-1] if f else "?")' "$1"; }
applied() { grep -q '^applied in block' "$1"; }
why() { grep -E 'RFP021-E|Error|error|not after|refus|wait' "$1" | tail -1 | cut -c1-240; }

period_step() {  # period_step begin|finish DIR LOGNAME
  local how=--save-dir; [ "$1" = finish ] && how=--load-dir
  "$CLI" attest update-public --program "$AP" --instance "$INSTANCE" --from "$OP" --beacon "$BEACON" --kind period --step "$1" \
    --committees "$RUN/committees" $how "$2" > "$RUN/$3.log" 2>&1
}

once() {
  local b s u n U st d
  b=$(bal); [ -n "$b" ] || { line "skip: balance unreadable"; return 0; }
  [ "$b" -ge "$FLOOR" ] || { line "STOP balance $b under floor $FLOOR"; return 3; }
  st=$(state); [ -n "$st" ] || { line "skip: store unreadable (balance $b)"; return 0; }
  read -r s u n <<<"$st"
  U=$(date -u +%Y%m%dT%H%M%SZ)
  if [ "$n" = unknown ]; then
    if [ -s "$RUN/pending-period" ]; then
      d=$(cat "$RUN/pending-period")
      period_step finish "$d" "finish-$U"
      if applied "$RUN/finish-$U.log"; then line "period Finish (pending job) $(sent "$RUN/finish-$U.log")"; else line "period Finish of the pending job refused ($(why "$RUN/finish-$U.log")); dropped"; fi
      rm -f "$RUN/pending-period"
    else
      d=$RUN/p-$U
      period_step begin "$d" "begin-$U"
      if applied "$RUN/begin-$U.log"; then
        echo "$d" > "$RUN/pending-period"
        line "period Begin $(sent "$RUN/begin-$U.log") ($(grep -oE 'period update for period [0-9]+' "$RUN/begin-$U.log" | tail -1); $(grep -oE '[0-9]+/512 signers' "$RUN/begin-$U.log" | tail -1))"
        period_step finish "$d" "finish-$U"
        if applied "$RUN/finish-$U.log"; then rm -f "$RUN/pending-period"; line "period Finish $(sent "$RUN/finish-$U.log")"
        else line "period Finish refused ($(why "$RUN/finish-$U.log")); retried next time"; fi
      else
        line "skip period update: $(why "$RUN/begin-$U.log") (store slot $s, next committee unknown)"
      fi
    fi
    st=$(state); [ -n "$st" ] && read -r s u n <<<"$st"
  fi
  "$CLI" attest update-public --program "$AP" --instance "$INSTANCE" --from "$OP" --beacon "$BEACON" --kind finality \
    --committees "$RUN/committees" --save-dir "$RUN/f-$U" > "$RUN/finality-$U.log" 2>&1
  if applied "$RUN/finality-$U.log"; then
    local s0=$s; st=$(state); read -r s u n <<<"$st"
    line "finality $(sent "$RUN/finality-$U.log") slot $s0 -> $s updates $u next $n ($(grep -oE '[0-9]+/512 signers' "$RUN/finality-$U.log" | tail -1); $(grep -oE "signed by the store's [A-Za-z]+ committee" "$RUN/finality-$U.log" | tail -1 | awk '{print $5}')) balance $(bal)"
    [ "$s" != "$s0" ] || { line "STOP the store did not move after an applied update: look before sending more"; return 3; }
  else
    rm -rf "$RUN/f-$U"
    line "no finality update: $(why "$RUN/finality-$U.log") (store slot $s, updates $u, next $n, balance $(bal))"
  fi
  return 0
}

running() { [ -f "$PIDF" ] && kill -0 "$(cat "$PIDF")" 2>/dev/null; }
case "${1:-status}" in
newkey)
  [ -z "${OP:-}" ] || { echo "already have OP=$OP"; exit 0; }
  [ -e "$L/wallet" ] && { echo "$L/wallet exists"; exit 1; }
  ( umask 077; mkdir -p "$L/wallet" ); chmod 700 "$L/wallet"
  "$CLI" wallet check-health >/dev/null 2>&1 || true
  "$CLI" wallet account new public > "$RUN/newkey.out" 2>&1; chmod 600 "$RUN/newkey.out"
  id=$(grep -oE 'Public/[1-9A-HJ-NP-Za-km-z]{32,44}' "$RUN/newkey.out" | sed -n 1p | cut -d/ -f2)
  pk=$(sed -n 's/^With pk //p' "$RUN/newkey.out" | sed -n 1p)
  [ -n "$id" ] && [ ${#pk} = 64 ] || { echo "no account made"; exit 1; }
  save OP "$id"; save OPK "$pk"; find "$L/wallet" -type f -exec chmod 600 {} \;
  echo "OP=$OP OPK=$OPK (the public key; the secret stays in $L/wallet)" ;;
bal) echo "payer $OP balance $(bal) head $(rpc getLastBlockId '[]')" ;;
deploy)
  [ -z "${AP:-}" ] || { echo "already deployed: $AP"; exit 0; }
  [ "$(shasum -a 256 "$ELF" | cut -d' ' -f1)" = "$ELF_SHA" ] || { echo "ELF sha256 mismatch"; exit 1; }
  { shasum -a 256 "$ELF" "$CLI"; echo "balance before $(bal) head $(rpc getLastBlockId '[]')"; } | tee -a "$RUN/deploy.log"
  "$CLI" deploy --which attest --elf "$ELF" --payer "$OP" 2>&1 | q | tee -a "$RUN/deploy.log" | grep -E '^(image id|program|immutable|Transaction is included)' | tail -4
  echo "balance after $(bal) head $(rpc getLastBlockId '[]')" | tee -a "$RUN/deploy.log"
  save AP "$(sed -n 's/^program //p' "$RUN/deploy.log" | tail -1)"; echo "AP=$AP" ;;
init)
  [ -z "${INSTANCE:-}" ] || { echo "already initialised: $INSTANCE"; exit 0; }
  "$CLI" attest init --program "$AP" --from "$OP" --network sepolia --checkpoint finalized --beacon "$BEACON" \
    --cross-check "$BEACON_X" --save-dir "$RUN/init" 2>&1 | q | tee "$RUN/init.log" | grep -vE '^  ' | tail -12
  save INSTANCE "$(sed -n 's/^INSTANCE=//p' "$RUN/init.log" | tail -1)"
  [ -n "$INSTANCE" ] || { echo "no instance"; exit 1; }
  mkdir -p "$RUN/committees" && cp "$RUN"/init/*.ssz "$RUN/committees/" && rm -f "$RUN/committees/bootstrap.ssz"
  echo "INSTANCE=$INSTANCE" ;;
show) show ;;
once) once; exit $? ;;
loop)
  echo $$ > "$PIDF"; line "loop start pid $$ interval $INTERVAL floor $FLOOR program $AP instance $INSTANCE"
  while true; do once; rc=$?; [ $rc = 3 ] && { line "loop stops"; rm -f "$PIDF"; exit 0; }; sleep "$INTERVAL"; done ;;
start)
  running && { echo "running, pid $(cat "$PIDF")"; exit 0; }
  nohup env L="$L" INTERVAL="$INTERVAL" FLOOR="$FLOOR" "$0" loop >> "$L/logs/loop.console" 2>&1 < /dev/null &
  sleep 1; echo "started pid $(cat "$PIDF" 2>/dev/null)" ;;
stop)
  running || { echo "not running"; rm -f "$PIDF"; exit 0; }
  kill "$(cat "$PIDF")"; rm -f "$PIDF"; line "loop stopped by hand"; echo stopped ;;
status)
  if running; then echo "running, pid $(cat "$PIDF")"; else echo "not running"; fi
  tail -4 "$LOG" 2>/dev/null ;;
*) echo "usage: L=<dir> $0 newkey|deploy|init|once|start|stop|status|show|bal"; exit 2 ;;
esac
