#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Branch public-update: catch-up over three consecutive sync-committee periods on real Sepolia
# data, through the lez-bridge CLI only, on the local h2 chain (127.0.0.1:3464). The instance
# starts from a finalized checkpoint three periods back (period 1413; Nimbus still serves its
# bootstrap, the root is cross-checked), then takes the period updates for 1413, 1414 and 1415
# (each in two public transactions, PeriodBegin and PeriodFinish), then a finality update at the
# head (period 1416). Stages: deploy | init | period | head | show. Throwaway operator only.
set -u
W=<local path> cd $W
R=$W/run-catchup; mkdir -p $R
CLI=$W/target-cli/release/lez-bridge
export LEZ_BRIDGE_SEQUENCER=http://127.0.0.1:3464 LEE_WALLET_PASSWORD= LEE_WALLET_HOME_DIR=$W/chain-h2/wallets/operator TMPDIR=$W/tmp
. chain-h2/operator.env; touch $R/ids.env; . $R/ids.env
save() { echo "$1=$2" >> $R/ids.env; eval "$1=$2"; }
BEACON=https://lodestar-sepolia.chainsafe.io NIMBUS=http://unstable.sepolia.beacon-api.nimbus.team
# The checkpoint: the first block of period 1413 (slot 11575296), finalized and canonical.
CHECKPOINT=0x2ca5e6854a13cd4d0907d4f9fd8a5c894f66c979d41f85fc76fddb054b192532
log() { tee -a $R/$1.log; }
q() { grep -vE '^(Transaction data|Stored)'; }
case "${1:-}" in
deploy)
  shasum -a 256 $W/elf-cu/attest_guest_public.bin | log deploy
  $CLI deploy --which attest --elf $W/elf-cu/attest_guest_public.bin --payer $OP 2>&1 | q | log deploy
  save AP "$(sed -n 's/^program //p' $R/deploy.log | tail -1)" ;;
init)
  $CLI attest init --program $AP --from $OP --network sepolia --checkpoint $CHECKPOINT --beacon $NIMBUS \
    --cross-check https://ethereum-sepolia-beacon-api.publicnode.com --cross-check $BEACON --save-dir $R/init 2>&1 | q | log init
  save INSTANCE "$(sed -n 's/^INSTANCE=//p' $R/init.log | tail -1)"
  mkdir -p $R/committees && cp $R/init/*.ssz $R/committees/ && rm -f $R/committees/bootstrap.ssz ;;
period)   # one period update in two transactions; P names it
  P=${P:?give P}
  $CLI attest update-public --program $AP --instance $INSTANCE --from $OP --kind period --step begin \
    --committees $R/committees --save-dir $R/period-$P 2>&1 | q | log period-$P-begin
  $CLI attest update-public --program $AP --instance $INSTANCE --from $OP --kind period --step finish \
    --committees $R/committees --load-dir $R/period-$P 2>&1 | q | log period-$P-finish
  $CLI attest show --program $AP --instance $INSTANCE 2>&1 | q | log period-$P-show ;;
head)
  for i in 1 2 3 4 5 6 7 8 9 10; do   # the beacon node may not have a newer finality update yet
    $CLI attest update-public --program $AP --instance $INSTANCE --from $OP --kind finality \
      --committees $R/committees --save-dir $R/head 2>&1 | q > $R/head.log
    grep -q "^applied in block" $R/head.log && break
    cat $R/head.log; echo "try $i failed; retrying in 60 s"; sleep 60
  done
  cat $R/head.log ;;
show)
  $CLI attest show --program $AP --instance $INSTANCE 2>&1 | q | log show
  $CLI --json attest show --program $AP --instance $INSTANCE > $R/show.json 2>/dev/null; cat $R/show.json ;;
*) echo "usage: $0 deploy|init|period|head|show"; exit 2 ;;
esac
