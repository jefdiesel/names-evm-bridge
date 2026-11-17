# Low participation: the finality update split at the pairing seam (2026-11-17)

**Result.** At 427 and at 342 signers (342 is the protocol's 2/3 floor), a finality update does
not fit one transaction: 10.16M and 11.10M gas against LEZ's 10M cap. Split at the pairing seam
it takes two public transactions. **Begin** runs every check up to and including the Miller loop
(6.55M at 427, 7.49M at 342). **Finish** runs the final exponentiation and the store write
(3.78M). Both steps ran on a **local** LEZ v0.4.0-rc3 chain (127.0.0.1:3464, loopback; not the
public demo chain), and an update one signer under the policy was refused there. No proof was made
anywhere. Branch `public-update`.

**SYNTHETIC committee.** The committee is 512 test keys from fixed public seeds,
`SHA256("RFP021/synthetic-committee/v1" ‖ i)`; nothing about them is secret. No real Sepolia
update with 342 signers exists to use. The recorded Sepolia finality update
(`attest/fixtures/sepolia`) is kept byte for byte: its headers, finality branch and signature
slot. Only the committee, the participation bits and the aggregate signature are synthetic.

The instances run on a **TEST-ONLY image**, `lez-bridge/methods/attest_guest_synthetic`: the public
program plus the feature `synthetic-init`. Its `InitSynthetic` creates an instance on the
synthetic committee without a bootstrap, which no real image accepts. Everything after creation
is the public program's own code.

## On the chain

Program `HWjcstPenwtuNnmJtytFYDtJyfioXJX1tSvjKB6tvamM` (synthetic test image, ELF sha256
`d90a6c7a…95df`). Gas is derived from each fee with the wire length read back from the
sequencer (`gas.txt`). Every figure equals the executor's count.

| Step | LEZ block | tx | Gas | Of 10M |
|---|---:|---|---:|---:|
| 427/512: `InitSynthetic` (instance `0x081b1ecf…63fb`) | 346 | `bd6c2b2d9202cc58c8b96a5754bfac3703bbf694f6cd624af8e61599f3ab1e5c` | 108,065 | 1.1% |
| 427/512: **`FinalityBegin`** | **347** | `d23d70b3f1dc210d90f9605b611dde6ccc78611a27bea1bc5bdfe35a4d29a87d` | **6,549,558** | 65.5% |
| 427/512: **`FinalityFinish`** | **348** | `b0b069453b4fcfd1e53a8c0ee0b554ec5ce066d84a85cf248f9ad59cb25191b7` | **3,780,392** | 37.8% |
| 342/512: `InitSynthetic` (instance `0x5c4eae87…6be3`) | 349 | `1910f99339811b613807ce5981a7af2038b8bd8e102b4d114c2c2f42d4196109` | 108,065 | 1.1% |
| 342/512: **`FinalityBegin`** | **350** | `3647e19c1aaf9d72f82b082c9f5dd452ce8cdc211796aebd98a711477f917eec` | **7,485,921** | 74.9% |
| 342/512: **`FinalityFinish`** | **351** | `95355072ec0fcfe18dec045f1a7ccc1e0af4017344b9fbc9beb14703773ab383` | **3,780,392** | 37.8% |
| 341/512: `FinalityBegin`, refused in simulation: `RFP021-E11 participation below policy: participation 341 is below the instance's policy of 342`; then forced through | 352 | `a3f50c583bc28ef3349b898379adb77b19d6a7565e080849455334753699d8e8` | (limit) | **reverted** |

After each Finish the store reads `updates 1`, with finalized slot 11563424 and anchor
`0xc637bfee…11f1` recorded (`show.txt`). These are the same finalized header and anchor that the
real 509/512 fixture update gives.

## Executor (`attest_cycles split`, `cycles-split.txt`; pinned by `attest/cycles/tests/public_cycles.rs`)

| Signers | One transaction | Begin | Finish |
|---:|---:|---:|---:|
| 512 | 9,224,671 | 5,615,063 | 3,780,389 |
| 441 | 10,000,427 (just over) | 6,391,206 | 3,780,389 |
| 427 | 10,158,667 | 6,549,561 | 3,780,389 |
| 400 | 10,452,659 | 6,843,102 | 3,780,389 |
| 342 | 11,095,486 | 7,485,924 | 3,780,389 |

The one-transaction column here is this image's. In the earlier sweep
(`public-demo/prove/public-update-20261117/`), 441 fit with 3,178 cycles to spare; the extra
checks since then took that margin, so the lowest that fits is now about 442 (an estimate from the 11K per signer; 442 was not run). Each non-signer
adds about 11K cycles to Begin, and Finish does not depend on participation.

## The design (`lez-bridge/attest/program/src/public.rs`)

- **`FinalityBegin`** runs every check of `UpdatePublic` through the Miller loop
  `e(pk, H(msg)) · e(-g1, sig)` (`fast::miller`), with the store asserting it is the job's `prev`
  first. It writes a job PDA keyed on the store and the whole update. The job holds the Miller
  value as 576 canonical bytes: twelve Fp coefficients, big endian, the same on every target.
- **`FinalityFinish`** (anyone, only the job id) has the job PDA take the final exponentiation of
  its own stored value. If the result is one, the job marks itself done and sends the store the
  same `Advance`, applied only if the store still equals `prev`. The time window is the job's
  signature slot to its weak-subjectivity deadline. `CloseJob` closes a stale job, as for period
  jobs.
- **A bad signature is caught at Finish, not at Begin.** Begin cannot tell before the final
  exponentiation, so such a job opens and can never finish; anyone closes it once the store moves.
  A refused key witness or signature encoding is caught at Begin.
- **Native tests** (`lez-bridge/attest/program/tests/finality_split.rs`, synthetic committee):
  427 and 342 advance the store exactly as the one-transaction verification of the same input;
  a second Finish is E09; a signature over another message is refused at Finish (E02); 341 signers
  are refused at Begin (E11); a wrong key witness is refused at Begin (E02); a period job cannot
  be finished as a finality job.

## Commands

```sh
lez-bridge/attest/cycles/build-public.sh && lez-bridge/attest/cycles/build-public.sh run split
public-demo/prove/split-run.sh deploy && public-demo/prove/split-run.sh messages && public-demo/prove/split-run.sh run
```

The test-only instructions (`InitSynthetic`) and the split's steps go out with the CLI's hidden
`raw-call`; `update-public` has no split mode yet. `$W` is the build host's work directory.

## Not done

- No real low-participation update ran: none exists for this period. The one-transaction path at
  506 to 509/512 ran on real data (`public-demo/prove/public-update-20261117/`).
- Period updates at low participation: their Finish is a whole signature check, about 9.38M at
  509/512. It would need the same seam split; not built.
- A CLI mode for the split.
