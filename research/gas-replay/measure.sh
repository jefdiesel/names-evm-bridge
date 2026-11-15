#!/bin/zsh
# usage: measure.sh <hardfork> <caps true|false> <port>
S=${S:?set S to a work directory for logs and broadcast copies}
mkdir -p $S/replay
hf=$1; caps=$2; port=$3; U=http://127.0.0.1:$port
F=${F:-$S/foundry19}   # directory holding Foundry >= 1.9.0 binaries
$F/anvil --hardfork $hf --port $port --gas-limit 1000000000 --silent > $S/replay/anvil-m-$hf-$caps.log 2>&1 &
AP=$!; sleep 3
cd ${VAULT_COPY:-$S/vcopy}   # a scratch copy of vault/
run() { n=$1; shift; $F/forge script script/GasMeasure.s.sol:GasMeasure --rpc-url $U --broadcast --slow --hardfork $hf --sig "$@" > $S/replay/fs-$hf-$caps-$n.log 2>&1 || { echo "FAILED $n"; tail -20 $S/replay/fs-$hf-$caps-$n.log; }; cp broadcast/GasMeasure.s.sol/31337/$n-latest.json $S/replay/bc-$hf-$caps-$n.json || echo "nocopy $n"; }
run setUpAll "setUpAll(bool)" $caps
run deposits "deposits(uint256,uint256)" 0 300
run extraDeposits "extraDeposits()"
run updates "updates()"
$F/cast rpc evm_increaseTime 3600 --rpc-url $U >/dev/null; $F/cast rpc evm_mine --rpc-url $U >/dev/null
run releases "releases()"
kill $AP
