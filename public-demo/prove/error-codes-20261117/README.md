# Typed error codes on a local chain (2026-11-17)

**Result.** The RFP-021 programs refuse with typed codes `RFP021-E01`…`E13`
(`docs/ERROR-CODES.md`), and the codes come back from a real local LEZ v0.4.0-rc3 chain (127.0.0.1:3464;
loopback, not the public demo chain) exactly as the native tests and the off-chain dry run
(`eth_attest::check`) report them for the same inputs. Branch `public-update`.

Program `Cc42wGgDmePzPPXBsG6pKU4X4Ushe3NL72dSLUWT7YAu` (image
`b56eea67cec45ffbf02a74ff974c1052ffb9d88cc0d617464ff4964fe2797df2`, ELF sha256
`777144ec…e98b`), instance `0x6465ca3748c7e66f0381b5a5302c832200e9324816894fee3305d22fdff6f437`,
`Init` at the Sepolia finalized checkpoint slot 11600672 (period 1416), LEZ block 144, tx
`a34ac09f51b86cbcce01977174141c95bd2aadbfd89cb2e2e8316627d8ec578a`.

## What the chain answered

The wallet simulates every transaction before sending; a refused one comes back as
`Failed to execute program: Guest panicked: <text>` (`send-sim-*.log`), the text being the
program's own. One refusal was also forced through (`--allow-revert`), so it is on the chain as a
reverted transaction.

| Case | Input | The chain's answer |
|---|---|---|
| E02 | the current finality update (506/512) with one signature byte flipped | `RFP021-E02 signature invalid: update rejected: invalid sync committee signature`; forced through: **LEZ block 168**, tx `5c5a2972b6f0cd519ae36c821473fc5400c72d7be3c5636539e796cab5c912d9`, **reverted** |
| (ok) | the same update, untampered | applied, **LEZ block 169**, tx `fd716d03b02679311aff299d4f5c865e4375e097c4d2b7ec9a238c5b648e6cb2` (9,289,364 cycles in the executor) |
| E09 | that update again, after it applied | `RFP021-E09 stale or replayed: the store changed since this update was verified` |
| E03 | `PeriodBegin` with the period-1415 update on this period-1416 store | `RFP021-E03 handoff not verified: update rejected: invalid sync committee period` |
| E08 | a message of two garbage bytes | `RFP021-E08 malformed input: message must decode` |
| E07 | a receipt-log statement (Sepolia block 12156302, tx 2, log 0) built for chain id **1** under this Sepolia instance's anchor, through the reference probe | `RFP021-E07 chain id mismatch: anchor is for another chain` (the attestation program's `AssertAnchor`) |

The other codes are exercised natively, each with the same code from the program's refusal and
from the dry run (`lez-bridge/attest/program/tests/error_codes.rs`, 11 tests): E01 (an anchor the
instance never recorded; a tampered finalized header), E04 (a bootstrap that is not the
checkpoint's), E05 and E06 (a tampered storage proof; a predicate that does not hold), E10 (a
valid update that brings nothing new), E11 (participation below policy), E12 (an effect from
another program), E13 (a committee witness that is not the store's).

## Commands

```sh
public-demo/prove/codes.sh setup       # deploy the coded image on the h2 chain, Init at the finalized checkpoint
public-demo/prove/codes.sh refusals    # E02 (also forced through), the real update, E09, E03, E08
public-demo/prove/e07.sh               # E07 through the reference probe
```

`$W` is the build host's work directory; in the copied logs its absolute path is written as `$W/`.

## Not done

- E04's weak-subjectivity case comes with the weak-subjectivity rule (next item); here E04 covers
  a bootstrap that does not match the checkpoint.
- The bridge program keeps its single `claim refused` for claims (privacy, `DESIGN.md` §2).
- The CLI prints these codes from its native pre-checks (`update-public`, `prove-account`,
  `prove-log`; next item); the wallet's refusal texts already carry them.
