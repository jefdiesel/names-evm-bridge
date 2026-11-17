# Catch-up over three consecutive periods on real Sepolia data (2026-11-17)

**Result.** An instance created at a finalized Sepolia checkpoint **three sync-committee periods
back** (period 1413) caught up to the head. It took the period updates for 1413, 1414 and 1415,
then a finality update at the head in period 1416. The run used only the `lez-bridge` CLI's own
commands, on a **local** LEZ v0.4.0-rc3 chain (127.0.0.1:3464, loopback; not the public demo
chain). The data is real: a Nimbus node still serves the period-1413 bootstrap, and Lodestar
serves the updates. No fixtures and no synthetic data were used, and no proof was made. Branch
`public-update`.

Program `9iqSp3swsQvYc8xJJCZWXg9SugGzUC8uPbSR2Aybv2LU`, the public image of this branch (image
`a7ea6565630eb33779b45ef84199a9e84033858030f97c5b936ea2f4b4b85b05`, `deploy.log`), instance
`0x0c63f6f727a5dfc4adc5b911f4fee66de7e8ceae23f98e629dd396f5f8db44d3`.

## Transactions (gas as the CLI derives it from the fee; `*.log`)

| Step | Store after it | Signers | LEZ block | tx | Gas |
|---|---|---:|---:|---|---:|
| `attest init` at checkpoint `0x2ca5e685…2532` (slot 11575296, period 1413) | period 1413, next committee unknown | — | 1087 | `fc44d443341c32c7f0d9a958ffb3ab39ba4552c7c0ab20fe26ea7476a574822c` | 9,117,222 |
| period-1413 update, `PeriodBegin` | (job open) | 507/512, current committee | 1089 | `b5a58aaf44de3cbdb806063d852fbc64701632e992c9997919f15eebcf0896fa` | 3,194,828 |
| period-1413 update, `PeriodFinish` | 1413, learns 1414's committee `0xbd0f6fec…a122` | | 1090 | `17890ffb28a048ca7c872b43e178aaf7bae59b14c8a10be7b23522c07400909d` | 9,404,283 |
| period-1414 update, `PeriodBegin` | (job open) | 511/512, **next** committee | 1091 | `6e380f4a839a4e090644b97b93cdaba92d2355afc81ca8bcd25a6d23fbef1c7b` | 3,625,417 |
| period-1414 update, `PeriodFinish` | **rotates to 1414** (finalized 11583488), learns 1415's `0xa1cc4756…0ced` | | 1092 | `8e8a0207d8689b09fff65c9845ca65d84c3fc36d00343b1b539746f8a20b5037` | 9,382,398 |
| period-1415 update, `PeriodBegin` | (job open) | 509/512, **next** committee | 1093 | `52897b5cdd968d166901be9d07a3c8412fedf042f20b175906c2c38fe27198b8` | 3,625,417 |
| period-1415 update, `PeriodFinish` | **rotates to 1415** (finalized 11591680), learns 1416's `0x8fe228b9…8891` | | 1094 | `546d1f3b34d744138c2fb9680fa655091b10de77faa77091832c134f943d86e2` | 9,408,109 |
| finality update at the head (`update-public --kind finality`) | **rotates to 1416**, finalized 11601856, anchor `0xf65166cd…9abc` recorded | 492/512, **next** committee | 1095 | `2079e9517ad487a1a79b32f889d861ce575b7e7ad1127d79d09ff0b3ddc62ccf` | 9,501,641 |

`show.log` / `show.json`: `updates 4`, finalized slot 11601856 (period 1416), with the current
committee being 1416's.

## What changed for this

- **The CLI picks the right update when catching up** (`lez-bridge/cli/src/attest.rs`,
  `fetch_update`). If the store does not know its next committee, the CLI fetches the store's own
  period update, which proves that committee. If the store knows it, the CLI fetches the next
  period's update, which that committee signed and which moves the store into that period.
  Before this change, the CLI fetched the store's own period every time, which went nowhere after
  the first step.
- The program needed no change. The period split (`PeriodBegin`/`PeriodFinish`) and the rotation
  were already there. This is the first CLI-only run through rotations by the next committee.

## Inputs

- The checkpoint's bootstrap came from `http://unstable.sepolia.beacon-api.nimbus.team`. Lodestar
  answers `syncCommitteeWitness not available` for bootstraps older than about one period.
- `attest init` cross-checked the root with publicnode and Lodestar.
- The three period updates from Lodestar and from Nimbus were byte-identical.
- `inputs.sha256` gives the digests of the saved inputs, kept on the build host.

## Limits

- The weak-subjectivity bound (1,348 epochs, about 6 days, on Sepolia) caps how far back an
  instance can start: a checkpoint older than that is refused (E04, `DESIGN.md` §9.6). This one
  was about 3.7 days old.
- A low-participation period update would still not fit: `PeriodFinish` is a whole signature
  check (about 9.4M here) and has no seam split yet (`finality-split-20261117/`).

## Commands

```sh
lez-bridge/attest/cycles/build-public.sh cli   # the CLI on the build host
public-demo/prove/catchup-run.sh deploy && public-demo/prove/catchup-run.sh init
for P in 1413 1414 1415; do P=$P public-demo/prove/catchup-run.sh period; done
public-demo/prove/catchup-run.sh head && public-demo/prove/catchup-run.sh show
```

`$W` is the build host's work directory; in the copied logs its absolute path is written as `$W/`.
