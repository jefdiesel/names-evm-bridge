# What LEZ and the Logos Blockchain let a third party prove (2026-11-06)

This combines two research reports, checked against LEZ 0.4-rc1 (1806e87), LEZ `dev`, and the node rev that rc1 pins.
Paths:
- `LEZ` = github.com/logos-blockchain/logos-execution-zone at tag `v0.4.0-rc1`.
- `NODE` = github.com/logos-blockchain/logos-blockchain at rev `d480efc` (the rev LEZ rc1 pins). Upstream master 4eddb9bc has the same Header.
- LIPs: github.com/logos-co/logos-lips, `docs/blockchain/raw/` at commit `58b56988` (cited by file name and line).

## LEZ (the zone)

- **Block** = header `{block_id, prev_block_hash, hash, timestamp}` plus body `{transactions, reverted}` (LEZ/lez/common/src/block.rs:51-67).
  - The block hash is SHA256 over `prefix ‖ borsh(whole block)`.
  - There is **no tx Merkle tree and no state root**.
- **State** (LEZ/lee/state_machine/src/state/mod.rs:158-175):
  - Public state is a HashTrieMap with no root.
  - Commitments go in an append-only SHA256 Merkle tree with `root_history`. Public message leaves go into the same tree.
  - Nullifiers are kept in a plain set with no root.
  - Nothing state-wide is ever published.
- **How blocks reach L1:** the sequencer inscribes the *full* borsh block into its channel, signed Ed25519 by a rotating, staked and accredited committee key.
  - Sources: LEZ/lez/sequencer/actors/bedrock/src/actor.rs:1041; committee_discovery.rs; programs/sequencer_stake, where slashing takes 2/3.
  - LEZ's own comment: "The channel authorizes who may write, not what they may claim" (lez/cross_zone/src/acceptance.rs:90-96).
- **Public execution is never proven.** The executor only (lee/state_machine/src/program/mod.rs:121-135). There is no block or state-transition proof and no roadmap item for one.
- **Private txs** are RISC0 **succinct** receipts (risc0-zkvm 3.0.5).
  - The journal is `(PublicExecutionContext, ProvenExecution)`. Each PrivateAction carries a nullifier, the **root it used**, a commitment and the encrypted post-state.
  - It also carries the boundary messages exchanged with public programs, the validity window and the fee.
  - The sequencer, not the proof, checks:
    - that the root is in root_history,
    - that the nullifier is unspent,
    - that commitments are fresh,
    - that the validity window holds,
    - that the public part succeeded,
    - that the tx is not `reverted`.
- **Reading a public account from a private tx** works only through a `PrivateToPublic` sync message that the sequencer runs publicly. The prover predicts the reply and the sequencer checks it. So the read is enforced by the sequencer, and the message is public.
- **Private execution emits no events** (`EMITS_EVENTS = false`).
- **Existing bridge code relies on the committee:**
  - Deposit op ids are never checked against L1 (LEZ#809), and block authors can forge deposits (LEZ#1032).
  - Withdraw panics with "Withdraws are disabled".
  - L1 `ChannelWithdraw` is a threshold multisig of accredited keys.
- **Token program:** no 0.4 token program has mint authorities or ATA. LEZ has only native_token plus a `wrapped_token` fed by the cross-zone inbox. lez-programs pins LEE v0.2.5-rc2.

## Logos Blockchain (L1, Cryptarchia)

- **Header** = `{slot, parent_block, body_root, proof_of_leadership{Groth16 BN254 proof, entropy, leader_key Ed25519, voucher_cm}}` (NODE/core/src/header/mod.rs).
  - Block id: Blake2b-256.
  - body_root = Blake2b over uncles plus the tx Merkle root (a binary Blake2b tree).
  - The leader also signs the header with Ed25519.
  - **There is no ledger or state root.** The spec's 297-byte header adds a version byte that the code doesn't have.
- **Finality:** the k-deep block under the Online rule (Praos-style); bootstrap uses the Genesis-style density rule.
  - Spec value: k = 2160, about 18 h at f = 1/30.
  - Testnet template: k = 120, about 1 h. The live k is unverified.
  - LEZ rc1 deployment settings: k = 5.
- **PoL public inputs:** `slot, epoch_nonce, t0, t1, ledger_aged, ledger_latest, P_lead`.
  - The epoch nonce can be derived from headers through a Poseidon2 chain of entropies, and t0/t1 from density.
  - **`ledger_aged` and `ledger_latest` are Poseidon2 UTXO-tree roots (depth 32) of the executed ledger, and no header commits to them.** The LIP admits it: "headers alone do not suffice" (cryptarchia-v1-protocol.md:328).
  - If those roots aren't authenticated, a prover can invent stake and pass every PoL.
- **Channel inscription** = `{channel_id, bytes, parent MsgId, signer}`, which forms a per-channel hash chain.
  - The signature covers the tx hash, but the tx hash leaves out proofs and signatures. So a Merkle proof against body_root proves the *content* of a post, not that an authorised signer made it.
  - Authorisation rests on L1 block validity, checked by honest validators.
- **No light-client spec or code exists anywhere in the org.** The validium and sovereign-rollup examples both trust a node API.
- **Per-block cost for a light client:** one Groth16-BN254 verify, one Ed25519 verify, two Blake2b hashes, one Poseidon2 hash, plus uncle PoLs. The RISC0 cycle cost of one BN254 Groth16 verify has not been measured.

## Consequences for an Ethereum verifier of LEZ burns

1. **A Logos-Blockchain light client needs the ledger root.** Either:
   - upstream commits it in the header or in a mandatory op (a LIP change), or
   - the guest replays the Mantle ledger from a checkpoint.
2. **The single-view private-fork problem.** A verifier that only sees the chain a prover submits can't tell a minority-stake private fork from the canonical chain without density rules, and possibly a permissionless contest window.
3. **A valid private burn receipt is not enough.** Its root must be a real LEZ root. That needs either sequencer trust (block validity) or full LEZ replay with public execution proven, which is a zone validity prover that doesn't exist.
