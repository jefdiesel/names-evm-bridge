# Privacy and recovery measurements (RFP-022 PP1-PP3, F9, R5)

Build host only: throwaway loopback chains, fresh keys, nothing proved. The recorded run and its
results are in `public-demo/prove/privacy-test-20261117/README.md`.

| File | What |
|---|---|
| `harness.py` | Drives N users and the control cohorts through deposit, claim, burn, update and release on a throwaway LEZ v0.4.0-rc3 chain and anvil; captures everything public (adversary input) and keeps the ground truth apart; Part B (`recovery`): wipe each wallet, restore from the seed phrase, `recover`, count found / expected, then claim and release from the restored wallet |
| `adversary.py` | Reads only the capture and pairs claims with deposits, releases with burns, burns with claims (inside LEZ) and releases with deposits (round trip); every feature scored on its own and all together |
| `score.py` | Compares the adversary's pairing with the ground truth; exact chance levels |
| `keccak.py` | Keccak-256 (stdlib only), for event topics and address-derived candidates |
| `plant.py` | SYNTHETIC controls: copies a capture and appends one value of 15 users' own deposits (sender address, 8 bytes of the commitment, gas used) to their claim blocks, to show the byte features would catch it (a test of the adversary, not of the bridge; reads the ground truth by design) |

## Mode: development receipts

The sequencer and every wallet run with `RISC0_DEV_MODE=1` (LEZ supports it: its `Justfile` and CI run the
same way). A private transaction's receipt is then risc0's `FakeReceipt`, which the dev-mode sequencer
accepts, but the circuit still runs in the executor, so everything the block publishes (the privacy
circuit's journal: boundary messages, one-time addresses, nullifiers, commitments, fee amounts) is the real
output. The only difference a reader of the chain sees is the proof's size: about 24 KB of stub instead of
about 223 KB (LEZ `docs/benchmarks/integration_bench.md`), the same for every private transaction in either
mode. Every private send runs under `scripts/guard.sh` with a 3 GB footprint cap and a timeout, so a real
proof (about 9.5 GB) started by mistake is killed. Outbound updates and releases are executed by
`outbound_host` (TrustedTip guest, G3 guest) and accepted by the vault's test MockVerifier, as in
`e2e-roundtrip.sh`.

## Run

```sh
# on the build host, in a directory holding bin/{lez-bridge,sequencer_service,outbound_host,local.json}
# and src/ (this repository's lez-bridge/scripts, public-demo/rc3 (not published), vault with lib/); ports 3480/3481, anvil 8580
PT_HOME=<dir> python3 lez-bridge/scripts/privacy/harness.py --run <dir>/runs/<name> --n 20 --controls 5 all
PT_HOME=<dir> python3 lez-bridge/scripts/privacy/harness.py --run <dir>/runs/<name> recovery
python3 lez-bridge/scripts/privacy/adversary.py <run>/capture <run>/adv
python3 lez-bridge/scripts/privacy/score.py <run> <run>/adv
PT_HOME=<dir> python3 lez-bridge/scripts/privacy/harness.py --run <dir>/runs/<name> stop
```

The run directory holds throwaway seed phrases and keys (`secret/`, `wallets/`, mode 700): delete it when done.

## Cohorts

Each cohort has its own (token, denomination), so it is its own anonymity set:

| Cohort | Set | Behaviour |
|---|---|---|
| `main` | ETH 0.01 | private claim to a one-time address (sponsored), private burn to a fresh seed-derived address, release sent by one relayer |
| `c1_public` | USDC 100 | control: claim and burn from a public LEZ account |
| `c2_addrsecret` | USDC 1000 | control: deposit secret = the depositor's Ethereum address padded to 32 bytes; claim names its account (`--reveal-recipient`) |
| `c3_fee` | ETH 0.1 | control: one per-user fee number: the Ethereum priority fee of the deposit and of the relayed release, and the LEZ `--gas-limit` of the claim and burn |
| `c4_self` | ETH 0.1 | control: the release is sent by the user's own depositing Ethereum account |

Schedule `all`: every deposit (random order, 1-6 s apart), one attested root, every claim (random order and
gaps), every burn, one G2 update, every release. Schedule `chain deploy users inbound_mixed outbound_mixed
capture` (with `--controls 0`): deposits and claims interleaved (each claim 0.5-3.5 deposits after its own, a
root attested after every deposit), burns in two batches with an update after each, a quarter of the releases
between the updates.

The capture also holds `lez-accounts.json`, the end-state view of every id the blocks name as an actor (the
sequencer serves no account history).
