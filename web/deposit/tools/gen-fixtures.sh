#!/bin/sh
# Regenerate test/fixtures/cast.json from Foundry's `cast`, the independent reference the
# page's hand-written ABI encoder, keccak and error table are tested against.
# Needs ~/.foundry/bin/cast. Sends nothing anywhere.
set -eu
CAST=${CAST:-$HOME/.foundry/bin/cast}
cd "$(dirname "$0")/.."
OUT=test/fixtures/cast.json

V=0x8C8b2664De28e5287158db4DB65D448638871b74
U=0xAc207b9Bfebf3eF7C26297eA6AbaDeEcCC4Aa71e
E=0x0000000000000000000000000000000000000000
C=0xffb6f3360e0acd47eb2fb92546674c9762ef4a628364be8d922e4618e3f07e37
A=0x37a46E7334Dd061E0C7E2298247EE9278d0d321F

cd_() { "$CAST" calldata "$@"; }
sig() { "$CAST" sig "$1"; }

{
  echo '{'
  echo '  "calldata": ['
  printf '    {"fn": "deposit(address,uint32,bytes32)", "args": ["%s", 2, "%s"], "hex": "%s"},\n' $U $C "$(cd_ 'deposit(address,uint32,bytes32)' $U 2 $C)"
  printf '    {"fn": "depositETH(uint32,bytes32)", "args": [0, "%s"], "hex": "%s"},\n' $C "$(cd_ 'depositETH(uint32,bytes32)' 0 $C)"
  printf '    {"fn": "tokenConfig(address)", "args": ["%s"], "hex": "%s"},\n' $E "$(cd_ 'tokenConfig(address)' $E)"
  printf '    {"fn": "capState(address)", "args": ["%s"], "hex": "%s"},\n' $U "$(cd_ 'capState(address)' $U)"
  printf '    {"fn": "globalCapState()", "args": [], "hex": "%s"},\n' "$(cd_ 'globalCapState()')"
  printf '    {"fn": "capWeight(address)", "args": ["%s"], "hex": "%s"},\n' $U "$(cd_ 'capWeight(address)' $U)"
  printf '    {"fn": "globalDepositsFrozen()", "args": [], "hex": "%s"},\n' "$(cd_ 'globalDepositsFrozen()')"
  printf '    {"fn": "commitmentUsed(bytes32)", "args": ["%s"], "hex": "%s"},\n' $C "$(cd_ 'commitmentUsed(bytes32)' $C)"
  printf '    {"fn": "counters(address,uint256)", "args": ["%s", 1], "hex": "%s"},\n' $U "$(cd_ 'counters(address,uint256)' $U 1)"
  printf '    {"fn": "depositCount()", "args": [], "hex": "%s"},\n' "$(cd_ 'depositCount()')"
  printf '    {"fn": "balanceOf(address)", "args": ["%s"], "hex": "%s"},\n' $A "$(cd_ 'balanceOf(address)' $A)"
  printf '    {"fn": "allowance(address,address)", "args": ["%s", "%s"], "hex": "%s"},\n' $A $V "$(cd_ 'allowance(address,address)' $A $V)"
  printf '    {"fn": "approve(address,uint256)", "args": ["%s", "100000000"], "hex": "%s"},\n' $V "$(cd_ 'approve(address,uint256)' $V 100000000)"
  printf '    {"fn": "approve(address,uint256)", "args": ["%s", "115792089237316195423570985008687907853269984665640564039457584007913129639935"], "hex": "%s"}\n' $V "$(cd_ 'approve(address,uint256)' $V 115792089237316195423570985008687907853269984665640564039457584007913129639935)"
  echo '  ],'
  echo '  "errors": {'
  first=1
  for e in 'IsFrozen()' 'NotRegistered()' 'BadDenomination()' 'WrongAmount()' 'CapExceeded()' 'CommitmentUsed()' 'UnexpectedValue()' \
           'ReentrancyGuardReentrantCall()' 'SafeERC20FailedOperation(address)' 'ERC20InsufficientBalance(address,uint256,uint256)' \
           'ERC20InsufficientAllowance(address,uint256,uint256)' 'ERC20InvalidApprover(address)' 'ERC20InvalidSpender(address)'; do
    [ $first = 1 ] || echo ','
    first=0
    printf '    "%s": "%s"' "$e" "$(sig "$e")"
  done
  echo
  echo '  },'
  printf '  "depositTopic": "%s",\n' "$("$CAST" keccak 'Deposit(address,uint32,bytes32,uint64,bytes32)')"
  printf '  "checksumVault": "%s",\n' "$("$CAST" to-check-sum-address 0x8c8b2664de28e5287158db4db65d448638871b74)"
  printf '  "checksumUsdc": "%s",\n' "$("$CAST" to-check-sum-address 0xac207b9bfebf3ef7c26297ea6abadeeccc4aa71e)"
  printf '  "errorString": "%s",\n' "$("$CAST" calldata 'Error(string)' 'cap reached')"
  printf '  "allowanceError": "%s",\n' "$("$CAST" calldata 'ERC20InsufficientAllowance(address,uint256,uint256)' $V 0 100000000)"
  # tokenConfig return data shaped like the live dUSDC config, for the decoder test
  printf '  "tokenConfigReturn": "%s"\n' "$("$CAST" abi-encode 'f(bool,bool,bool,uint8,uint64,uint256[],uint256[],uint256[],uint256)' true false false 6 86400 '[100000000,1000000000,10000000000]' '[0,0,0]' '[0,0,0]' 0)"
  echo '}'
} > "$OUT"
echo "wrote $OUT"
