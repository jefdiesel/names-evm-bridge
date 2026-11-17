# The public path through the CLI, and the weak-subjectivity rule (2026-11-17)

**Result.** Every RFP-021 action on the public path ran through the `lez-bridge` CLI's own
commands, with no `raw-call` and no harness, on a **local** LEZ v0.4.0-rc3 chain (127.0.0.1:3464,
loopback; not the public demo chain). The commands were deploy, init, a public finality update,
an account statement, a receipt-log statement, a period update in two steps, closing the job, and
reading the instance (`--json` too). The image has the weak-subjectivity rule. The CLI's dry run
refuses an update signed past the bound with `RFP021-E04`. Branch `public-update`.

Program `2Ut36VhoZVy85X8ZCFkz5brt1Cwv13K8puyZwhWXmVLa`, image
`e5e06eaf42f0eca97568e7b553ed97e916a8e0cf931a18405480c5b181cd53e4` (ELF sha256 `a340ceb2…fd73`,
`deploy.log`), instance `0x3d3dcc90f7a348d71c71d5f61244d2715dfacf9632bcc58f862404331ac5422a`. The
CLI is built from this branch (`build-public.sh cli`; its guests are not embedded, so `deploy`
takes `--elf`).

## Transactions (gas as the CLI derives it from the fee; `*.log`)

| Command | LEZ block | tx | Gas |
|---|---:|---|---:|
| `attest init --checkpoint finalized` (slot 11600768, period 1416) | 227 | `419d63643c497899e4857d9ed564a76fba4f5b259142369d9db8637e9060c52e` | ~9.12M |
| `attest update-public --kind finality` (506/512, finalized 11600800) | 245 | `5e1896f772382562fddcfb681f85482562a16b489b54d274f77aa41fa1da1582` | 9,289,923 |
| `attest prove-account --field balance` (v2 vault = 230,000,000,000,000,000 wei) | 246 | `2ae535c571799cadfa6d1bb1654671370666cb821864d78f42d96ff0390b49bd` | 218,212 |
| `attest prove-log` (block 12156430, tx 0, log 0) | 247 | `2ab74fc9ee5a8329cf8707bb834f7b2a9602985ac5922c0e1582e6bdbb9fe593` | 88,930 |
| `attest update-public --kind period --step begin` (the period-1416 update, 506/512) | 248 | (`period-begin.log`) | 3,193,778 |
| `attest update-public --kind period --step finish` (the store learns 1417's committee `0x1f920a67…a0cc`) | 249 | `f1f557e02c3678dcbc09b7ffc9873cc794201df062c594b0a1f8f9a80d02959f` | 9,338,096 |
| `attest close-job` | 250 | `aa9ffb9439d5bfb4e9bc6fd244cdce3672b1cf492dc1e0d2fcd80d1c1ffd21b9` | — |
| `attest show` and `--json attest show` | — | — | read only (`show.log`, `show.json`) |

Each send prints the native pre-check first (the program's own code); a refusal prints its
`RFP021-Enn` code and sends nothing.

## The weak-subjectivity rule (RFP-021 F10; `DESIGN.md` §9.6)

- **The bound:** 1,348 epochs (about 6 days) on Sepolia, from Gloas's guide (lower value), and
  3,532 on mainnet, from Electra's guide (`eth_attest_core::ws_epochs`, `ws_deadline_ms`).
- **The program enforces it twice, on every path** (private `Update`, `UpdatePublic`,
  `PeriodFinish`, `FinalityFinish`):
  - an update signed at or past the store's deadline is refused with E04 before any other check;
  - the deadline ends every update's timestamp window, which the sequencer checks against the
    block time. So once the chain's clock passes the deadline, no update applies, whatever slot
    it was signed in.
- **Native tests** (`lez-bridge/attest/program/tests/error_codes.rs`):
  - updates signed at the bound, one past it, and 1,000,000 slots past it are refused with E04,
    on both paths and in the dry run;
  - one slot under the bound is not refused for weak subjectivity;
  - the window's end equals the deadline on both paths, and on `PeriodFinish`
    (`public_update.rs`).
- **The CLI here:** the real update re-dated 2,000 epochs later (signature slot 11664865) was
  refused before sending: `RFP021-E04 checkpoint stale or invalid: the update does not verify
  natively, nothing sent` (`ws-cli.log`, input `ws-far.ssz`). It was not sent to the chain: the CLI
  refuses to send a transaction its pre-check refuses. The chain's own E04 path is the same program
  code (native tests). Its clock end could not be shown on a chain that runs on real time with
  instances only minutes old.
- **Replacement and consumer migration:** `DESIGN.md` §9.6. In short, an instance past its bound
  keeps its anchors, a new instance comes from a fresh checkpoint, and consumers move to the new
  instance id.

## Commands

```sh
lez-bridge/attest/cycles/build-public.sh cli       # the CLI (release) on the build host
public-demo/prove/cli-run.sh deploy|init|update|statements|ws|period|show
```

`$W` is the build host's work directory; in the copied logs its absolute path is written as `$W/`.
