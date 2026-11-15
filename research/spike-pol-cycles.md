# Spike 1: cycles to check a Logos Blockchain leader proof in a RISC Zero guest (2026-11-06)

Code: a separate spike, not included in this repository (risc0-zkvm 3.0.5, executor only).
Input: the node's own PoL fixture (`zk/circuits/resources/tests/pol`, Groth16 BN254, 9 public inputs; it verifies natively).
All points are validated on load (on-curve check, plus the G2 subgroup check).

| | arkworks 0.5 (the node's library) | substrate-bn, risc0 accelerated |
|---|---|---|
| Load the verifying key (once per run) | 21.7M | 15.3M |
| Prepare the verifying key (once) | 41.9M | 0 |
| Load one proof and its inputs | 7.2M | 5.1M |
| Verify one proof | 68.5M | 28.6M |
| **Per block, single** | **~76M** | **~34M** |
| **Per block, batch of 16** (the node's batch_verify) | **24.4M** | **10.4M** |

- Testnet: 1 s slots, f = 1/30, so about 2,880 blocks a day (`deployment/ceremony/genesis/testnet`, k = 120).
- At 10.4M per block, accelerated and batched: about 30B cycles per day of chain, or about 1.25B per hour. Uncle PoLs, Ed25519, Blake2b and ledger-root authentication come on top.
- Proving time: NOT measured. The succinct prove of the 216M-cycle run panicked and rebooted the test machine.

## Proving time on the test machine (Apple M4, 16 GB; 2026-11-06)

One PoL check (MODE 2: validated load plus a single verify, accelerated substrate-bn), proved under a memory watchdog:
- 4 threads, segment po2 19, in-process CPU prover;
- 49.2M user cycles, padded to 55.6M across 106 segments.

| | |
|---|---|
| Succinct prove | **9,778 s (2 h 43 min)**, about 92 s per segment, about 5.7k cycles/s |
| Peak memory | **4.85 GB** RSS (4.99 GB footprint) |
| Paused by the watchdog | 0 s |
| Receipt | verified |

The first attempt (09:28) was killed by the watchdog when another process loaded 6.7 GB, not because the prover misbehaved.

**What it means:**
- A day of testnet chain (~30B cycles) at this rate would take about 2 months on the test machine.
- So G2 has to run on GPU provers or Boundless. That is the operator's job; users never run G2.
- The proofs users make in Basecamp (claim, burn, release) are far smaller. A 1-segment proof took 45 s and 2.4 GB. They get measured in the private e2e run.
