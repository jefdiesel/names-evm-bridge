# Statements on real Sepolia data and the two reference consumers, on a local chain (2026-11-17)

**Result.** On a **local** LEZ v0.4.0-rc3 chain (127.0.0.1:3464; the handoff re-run's chain,
`public-demo/prove/public-handoff-20261117/rerun-h2/`), under anchors that the instance's own
**public** finality updates recorded, these ran as ordinary public transactions, no proof
anywhere:
- a **receipt-log statement** on a real Sepolia ERC-20 `Transfer` log (RFP-021 F3);
- an **account-field statement** on a real Sepolia account (the v2 vault's balance);
- the **ERC-20 consumer**: a transfer of a given token to a given contract, claimed once, with the
  replay and a wrong-chain claim both refused;
- the **native-ETH consumer**: ETH sent to WETH9, proved through WETH9's `Deposit` event, claimed
  once (twice: an ETH deposit made inside a router call, and an account's own direct deposit), with
  the replays refused.

State label (`DESIGN.md`): **(1) run on a chain: a local LEZ sequencer** (loopback). Not the
public demo chain, not a Logos network. Branch `public-update`.

## Programs and data

- Attestation program `DVM9PDn8VGnGAyw24jbesZ62GGcMJ1ppYBeFmmZSjwzi` (image `bf0dc638…354c`),
  instance `0x4e66926d34a1d27ad37b07a077f662a8eaea614f49b4bf09cfb04ff30139ba1d` (Sepolia,
  chain id 11155111). Reference probe `Gvn9t7ELG8bHgta7WBd9mLuCh4hmedwwbJ3URCqs8wQC` (the rc3
  CLI's embedded probe). Consumers: `HBdZtcY57WPGJcpKRAYmdx2zx2zDNsiw3vRbRNUHZ87n`
  (`lez-bridge/attest/consumer`, guest `lez-bridge/methods/eth_consumer_guest`, ELF sha256
  `21cb95cc…ded9`), one program, a deployment per configuration (`Init`).
- Anchors, each recorded by a public finality update of this instance:
  - **f1**: LEZ block 39, Sepolia finalized slot 11600544 (492/512 signers, 9,441,703 gas),
    execution block 12156175 `0xeaa55b9f…6eba`;
  - **f2**: LEZ block 67, slot 11600576 (506/512, 9,289,209 gas), execution block 12156207
    `0xd09a889c…7f3f`.
- Block and receipts: `stm-f1/`, `stm-f2/` (`block.json` = `eth_getBlockByHash`, `receipts.json` =
  `eth_getBlockReceipts`, from PublicNode, by `public-demo/prove/fetch-block.py`). The harness
  rebuilds the header RLP (it must hash to the anchor) and the receipts trie (its root must equal
  the header's `receiptsRoot`) before building any message (`lez-bridge/attest/cycles/src/stmt.rs`).

## Transactions

Gas from the fee and the wire length read back from the sequencer (`txlen.log`; each `send-*.log`
has the balances and fee state around it). A transaction refused in simulation was then sent
anyway with the CLI's test-only `--allow-revert`, so the refusal is on the chain too: it is
included, **reverted**, and pays its whole 10M limit; the refusal text is the wallet simulation's
(`send-sim-*.log`).

| What | LEZ block | tx | Gas | Result |
|---|---:|---|---:|---|
| **Receipt-log statement**: log 1 of tx 0 of block 12156175, token `0x07ea2ff8…705e` `Transfer`; statement id `0xd0246172…b67f` | **42** | `813ccdbdac44937820024277d11003799bdf81b391cdd8790309bd24d05d02e1` | 174,001 | applied |
| **Account-field statement**: v2 vault `0x8C8b…1b74` balance = 230,000,000,000,000,000 wei under anchor f2 | **71** | `4353f95be2ceb1ac073cd84e05e6589f03f93af00db02731a14e2da319a5c6e7` | 216,605 | applied |
| ERC-20 consumer `Init` (token `0x07ea…705e`, recipient contract `0xbdc1fe97…6aa1`, Sepolia) | 47 | `04d2dacc5a8f9d9a2a14392c4fbd8adc8467105099d8631a93d0e39701cdc02e` | 32,662 | applied |
| **ERC-20 claim**: sender `0x133e6955…0965`, amount 20,105,379,721,935,501, recipient `0xbdc1…6aa1` (the same log) | **48** | `735ae5ef3b338b74f1a89ef1a59e2bf3190e94493854d9d0ef0cafc2916ebdc3` | 216,310 | applied |
| ERC-20 **replay** of that claim | 49 | `1a35a5f54b55821089888aefac7d3f2e7b0e302ddfc55f66e08aec4b14916994` | (limit) | **reverted**: "already claimed" |
| ERC-20 consumer `Init` for **chain id 1** on this Sepolia instance | 44 | `733b62f96c8d111ae8b97e365df163f278fe865610cff64d1e214668f5ef1a74` | — | applied (log of the first attempt not kept) |
| ERC-20 claim on the **chain-1 deployment** | 50 | `8fd2386df16132dd202a800f577ed8c8966e22f65ca736f443bb7051e05e2289` | (limit) | **reverted**: "anchor is for another chain" (the attestation's `AssertAnchor`) |
| ETH consumer `Init` (WETH `0x7b79995e…e7f9`) | 51 | `08e8e5ac7256df61245f8fdbdb5d38fafe6b1abb48ff3e128cc89da510bc7c05` | 32,051 | applied |
| **ETH claim**: tx 81 log 2 of block 12156175, `Deposit` dst `0xa1f78bed…a4a8` (a router that sent the ETH within its call), 20,000,000,000,000 wei | **52** | `c626249344ea17368ed2acc21c3bd03bfd099bb02651f2d5edfe00da6bf2016c` | 198,352 | applied |
| ETH **replay** | 53 | `99814decb39ca462ab3795b60da18bfbd67c4f5fb31cfe1a33c98f9c98ac648b` | (limit) | **reverted**: "already claimed" |
| ETH consumer `Init` (WETH9 `0xfFf99767…6B14`) | 74 | `3e646d26e150e64f070a7caefb3901a802b6f84c5474a66d9e1267f44cd79873` | 32,051 | applied |
| **ETH claim**: tx 17 log 1 of block 12156207: account `0xaa0761c2…c7df` called `deposit()` on WETH9 with 2,424,000,000,000,000 wei (tx `to` = WETH9, `value` = that amount) | **75** | `b3cbe5136aa78b2f93b1b30d00966882a0e72b8eebc7db1944bf6c78dc7bd953` | 158,267 | applied |
| ETH **replay** | 76 | `b388db64e0090f4deb0a987cac65a86f706274e619e6566133c75796d26ecd16` | (limit) | **reverted** ("already claimed" in simulation) |

The claimer's actor holds the last claim (`Claimed { statement_id, sender, amount, recipient }`);
each nullifier PDA is keyed on the deployment and the statement id (`measure-*.txt` name them).

## The consumers (`lez-bridge/attest/consumer/src/lib.rs`)

- **ERC-20 transfer to a given contract** (token, recipient; the claim gives sender and amount): a
  `Transfer(from, to, value)` log of the configured token whose `to` is the configured contract.
- **Native ETH transfer to a given contract** (recipient contract; the claim gives sender and
  amount): **an event, not state.** A balance or a storage slot says how much the contract holds,
  not who sent it or which transfer it was; the contract's `Deposit(address indexed dst, uint256
  wad)` log names the sender and the amount and sits at a fixed place (block, transaction, log),
  so it can be spent once. The cost: only contracts that emit such an event when they receive ETH
  work (WETH9 does, and so does the bridge vault's deposit); a plain ETH transfer to a contract
  that emits nothing would need the transaction itself proved and its sender recovered from the
  signature, which is not built.
- A claim verifies the receipt log in the claimer's own actor, then, in the same transaction:
  `AssertAnchor` to the attestation program (the anchor is this instance's, for this chain), a
  nullifier PDA created once per (deployment, statement id), and a check that the claim used the
  deployment's own configuration (its config PDA). Tests: `lez-bridge/attest/program/tests/consumer.rs`
  (3, native, synthetic receipts): each kind claimed once, replays, wrong token, wrong recipient,
  zero-value deposit, a made-up configuration, another chain, an anchor the instance never
  finalized, a second `Init`.

## Commands (build host; `$W` is the work directory, written so in the copied logs)

```sh
H=h2 public-demo/prove/public-handoff.sh deploy-consumer       # CELF = the eth_consumer_guest ELF
H=h2 public-demo/prove/public-handoff.sh probe-deploy
H=h2 N=12 loop-updates.sh                                       # a public finality update per epoch
python3 public-demo/prove/fetch-block.py <anchor block hash> $W/run-h2/stm-f1
attest_cycles stmt-log     ATTEST PROBE SIGNER INSTANCE 11155111 $W/run-h2/stm-f1 0 1
attest_cycles stmt-account ATTEST PROBE SIGNER INSTANCE 11155111 $W/run-h2/slot-f2 balance
attest_cycles consumer CONSUMER SIGNER ATTEST INSTANCE 11155111 erc20 <token> <recipient> $W/run-h2/stm-f1 0 1
attest_cycles consumer CONSUMER SIGNER ATTEST INSTANCE 1        erc20 <token> <recipient> $W/run-h2/stm-f1 0 1
attest_cycles consumer CONSUMER SIGNER ATTEST INSTANCE 11155111 eth <weth> $W/run-h2/stm-f2 17 1
H=h2 PROG=<program> public-demo/prove/public-handoff.sh raw <name> <message.hex> <accounts>   # [ALLOW_REVERT=1]
```

## Not done, and limits

- **The account proof window:** PublicNode, Tenderly and 1rpc refused `eth_getProof` for a block
  71 blocks behind the head ("distance to target block exceeds maximum proof window",
  `probe-fetch-f1.log`, `probe-fetch-f2.log`); thirdweb served it. Storage and account statements
  need an archive node or a fetch within minutes of the anchor; receipt-log statements do not
  (`eth_getBlockReceipts` serves old blocks).
- The refusal texts are fixed strings today; typed error codes are the next item.
- The probe on the chain is the rc3 CLI's embedded probe build, not this branch's; its statement
  code is the same library, but its cycle count differs slightly from the executor's
  (174,001 on the chain against 174,148).
- No CLI command builds these messages yet (the harness does; `raw-call` sends).
- Not run on the public demo chain or any Logos network.
