# `DemoAttestedVerifier`: the operator-attested demo stage

> **Not trustless.** This verifier checks no zero-knowledge proof. An operator ("the attestor") checks proofs off chain and signs. The vault accepts any `update` or `release` that this one key signs. Use it only for the public **testnet** demo, and always label it "operator-attested". Never present it as the RFP-022 design.

- Contract: `DemoAttestedVerifier.sol`. Tests: `test/DemoAttestedVerifier.t.sol` (unit tests and vectors) and `test/DemoVaultFlow.t.sol` (the unchanged `BridgeVault` running on this verifier).
- Deploy script: `script/SepoliaDemoV3.s.sol`, run on 2026-11-14 for vault v3 (`../../DEPLOYMENTS.md`). Its logic is exercised by `test/SepoliaDemoV3Script.t.sol`.

> **The v3 vault cannot take any update (found 2026-11-15).** It was deployed with `initialG2State` zero, so `g2State()` is zero, and `update` requires `prevState == g2State`. The G2-lite guest's `prevState` is always the hash of a real state, never zero, so no journal it produces can be applied, signed or proved, and nothing can be released from v3. The bridge's cfgId binds the v3 vault's address, so no other vault can take over its burns. Signing a hand-made bootstrap journal (prevState zero) would break rule 3 of UPDATE below; it is not done. The script now refuses `initialG2State` zero: a later vault must start from the genesis state of a tip before its bridge's `Initialize` (`outbound_host genesis`, `lez-bridge/outbound/`).

## Why it exists

The user decided this on 2026-11-13. On-chain verification of the outbound state proof (G2) does not exist yet. The real-verifier demo vaults (v1, v2) pin a placeholder update image, so no exit can ever be recorded there.

This verifier lets the whole loop run on Sepolia today:

1. deposit on Sepolia;
2. claim on the Logos test chain;
3. burn;
4. state update;
5. release on Sepolia.

Some check is still needed. With the test `MockVerifier` (`seal == imageId ‖ digest`), anyone could forge a seal and drain the demo vault's test funds.

## Design choices

| Choice | Taken | Why |
|---|---|---|
| Message format | **EIP-712 typed data** | A hardware or standard wallet (and `cast wallet sign --data`) shows the attestor exactly what it signs: the domain name, chain, verifier and the three fields. The domain binds `block.chainid` and `address(this)` in a standard way. The cost over a plain digest is three short `keccak`s per call, an estimated few hundred gas (not measured), which is small next to `ecrecover`'s 3,000. A plain digest would be smaller, but every signer would need custom code to show it. |
| Domain-separation tag | `TAG = keccak256("RFP022/demo-attested-verifier/v1")`, a field of the signed struct | The tag the spec asked for. It is also the seal prefix (below), so the version is visible in every seal. |
| Attestor | **One immutable address** | m-of-n would only spread the same off-chain trust and would need its own key ceremony. A rotatable attestor would add an owner, an upgrade path and a second key to protect. If the key leaks, deploy a new demo vault, the same as the real design's "new version" rule for any change of verification. |
| Lifetime | **Immutable `EXPIRES_AT = deploy time + lifetime`**, `0 < lifetime ≤ 365 days`; the script defaults to 180 days | The stage can't quietly become permanent. The constructor takes a duration, not an absolute time, so the deploy script and an offline simulation give the same result. |
| Owner, pause, upgrade | **None** | The vault's freeze (deposits and releases, per token or global) is the pause. |
| Seal | `SEAL_PREFIX ‖ r ‖ s ‖ v`, **69 bytes** | The prefix makes a seal self-describing, like RISC Zero's 4-byte selector. A bare 65-byte signature could start with `0x73c457ba` by chance (2^-32) and then could never be accepted. |
| Real RISC Zero seal | Refused with `ZkProofNotCheckedHere()` | The check comes before the length check, so a 260-byte Groth16 seal (`0x73c457ba…`) gets this error, not `BadSealLength`. A real proof can never be mistaken for "verified" here. |
| Signatures | v ∈ {27, 28}, s ≤ n/2, `ecrecover` ≠ 0 | No malleable twin (`HighS`), no ambiguous v (`BadV`), no zero signer (`InvalidSignature`). |
| Errors | Custom errors | `Expired(expiresAt)`, `ZkProofNotCheckedHere()`, `BadSealLength(len)`, `UnknownSealPrefix(prefix)`, `BadV(v)`, `HighS()`, `InvalidSignature()`, `NotAttestor(recovered)`, and in the constructor `ZeroAttestor()` and `BadLifetime()`. |

**Check order in `verify`:**
1. expiry;
2. the RISC Zero selector;
3. length (69 bytes);
4. prefix;
5. v;
6. s;
7. `ecrecover` ≠ 0;
8. signer == `ATTESTOR`.

A wrong chain id, a wrong verifier address, a wrong image id or a wrong digest all give a valid signature by *some other* address. They all end in `NotAttestor(recovered)`, because the contract can't tell which field was wrong.

**Views:**
- `ATTESTOR()`, `EXPIRES_AT()`;
- `KIND()` = `"demo-attested: an operator checked the proof off chain; NOT trustless"`, `VERSION()` = `"1.0.0"`;
- `domainSeparator()`, `attestationDigest(imageId, journalDigest)`, `encodeSeal(r, s, v)`;
- the constants `TAG`, `SEAL_PREFIX`, `ATTESTATION_TYPEHASH`, `DOMAIN_TYPEHASH`, `DOMAIN_NAME`, `DOMAIN_VERSION`, `RISC_ZERO_GROTH16_SELECTOR`, `SEAL_LENGTH`, `MAX_LIFETIME`.

**One verifier per vault.** Deploy a separate verifier for each vault: the EIP-712 domain then binds the vault as well. Two vaults that share one verifier are still safe:
- an update journal carries the vault's `cfgId`, so it fails with `WrongConfig` at the other vault;
- a release digest includes `cfgId`;
- both cases are tested.

## Exact signing scheme (for the Rust attestor)

```
DOMAIN_TYPEHASH  = keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)")
                 = 0x8b73c3c69bb8fe3d512ecc4cf759cc79239f7b179b0ffacaa9a75d522b39400f
name             = "RFP022 DemoAttestedVerifier"   keccak = 0x0475713f9caf5cbd66ca03acbeb50f804562bb1c17bbd1ff10faafd0a1b6bcbf
version          = "1"                             keccak = 0xc89efdaa54c0f20c7adf612882df0950f5a951637e0307cdcb4c672f298b8bc6
domainSeparator  = keccak256(abi.encode(DOMAIN_TYPEHASH, keccak(name), keccak(version), chainId, verifier))
                   (5 words, 160 bytes; chainId as uint256, verifier left-padded to 32 bytes)

ATTESTATION_TYPEHASH = keccak256("DemoAttestation(bytes32 tag,bytes32 imageId,bytes32 journalDigest)")
                     = 0xcc992c6b34f335b92a6b96d1f87b9171d5c2c156eb4b1843c3801e0362013efd
TAG                  = keccak256("RFP022/demo-attested-verifier/v1")
                     = 0x7fa98d9a081f1336a9cea9fee115b0cc96a9a094b5daa49fe0df4757618bc17f
structHash = keccak256(ATTESTATION_TYPEHASH ‖ TAG ‖ imageId ‖ journalDigest)        (4 words, 128 bytes)

digest = keccak256(0x19 ‖ 0x01 ‖ domainSeparator ‖ structHash)                    (66 bytes)
```

**How to sign:**
- Sign `digest` itself with secp256k1. Do not hash it again, and do not add the "\x19Ethereum Signed Message" prefix. This is standard `eth_signTypedData_v4`. In alloy, a `sol!` struct `DemoAttestation { bytes32 tag; bytes32 imageId; bytes32 journalDigest; }` with `eip712_signing_hash(&domain)` should give the same digest. That was not compiled here; the vectors below are the reference.
- Use RFC 6979 deterministic nonces, and use **low s** (s ≤ n/2). k256 and alloy normalise s; check that whatever library you use does too.

**The seal:** `0x7fa98d9a ‖ r (32 bytes) ‖ s (32 bytes) ‖ v (1 byte)` = 69 bytes, with **v = 27 + recovery id**. k256 returns a recovery id of 0 or 1; add 27.

**What `imageId` and `journalDigest` are:**
- **`imageId`:** the vault's `updateImageId` or `releaseImageId`.
- **`journalDigest`:**
  - **update:** `sha256(journal)`, where the journal is `abi.encode(UpdateJournal)`, the bytes passed to `update`;
  - **release:** `sha256(abi.encode(cfgId, root, nullifier, token, denom, recipient, relayer, relayFee))` (256 bytes), which is exactly the G3 guest's journal.
- The digest is plain SHA-256, not keccak.

### Test vectors

Context for both vectors:
- `chainId = 11155111` (Sepolia);
- verifier at `0x0000000000000000000000000000000000de3071`;
- attestor = Anvil/Hardhat account 0, the PUBLIC test key (`lez-bridge/scripts/anvil-dev-key.sh 0` prints it), address `0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266`.

`domainSeparator = 0xe6b874687824a21c15cc5c4cb2de2e3ba9015adc0e7dc330b64abe48610ffb63`

| | Vector 1 (v = 28) | Vector 2 (v = 27) |
|---|---|---|
| `imageId` | `0x1111111111111111111111111111111111111111111111111111111111111111` | `0x6bcbd714ca182de91cfb698216662531ae8243fa579bd31b8e48b514df0672a4` (the v1/v2 release image id) |
| `journalDigest` | `0x2222222222222222222222222222222222222222222222222222222222222222` | `0x3333333333333333333333333333333333333333333333333333333333333333` |
| `structHash` | `0x5821fc36b723560edbece58bc7c0a6524a9015cd4e5b1ed15a19b0102ff27ea2` | `0x04881f3c297418dccafe97327bdf29d7b2131f2aee35de97a5e7118224917352` |
| digest signed | `0xccd95db1a44605ac7755a99f998c2357a38c70b51ff5fcc129511baab566543a` | `0xcfdb0f6252fd4bfcf2e36fb161b724f9262cdc670398d8332a78290cd9b8bf7d` |
| r | `0xb548f39a529a2830d35ce44dd7321a713f950ff063c45b017a0cf08606096bcb` | `0x1e726f13485fbd807ae94d9308e3da98140007678e039767864a22eab3216e5f` |
| s | `0x17cae6edf919145681cfcd99b04496413f81e6531d9780983ab77bbb5dd792f6` | `0x43fe4f685a4544bad2a8db39dcb353482fc5efb17842243edab45c215ffeb100` |
| v | `0x1c` (28) | `0x1b` (27) |

**Seals:**
- Vector 1: `0x7fa98d9ab548f39a529a2830d35ce44dd7321a713f950ff063c45b017a0cf08606096bcb17cae6edf919145681cfcd99b04496413f81e6531d9780983ab77bbb5dd792f61c`
- Vector 2: `0x7fa98d9a1e726f13485fbd807ae94d9308e3da98140007678e039767864a22eab3216e5f43fe4f685a4544bad2a8db39dcb353482fc5efb17842243edab45c215ffeb1001b`

**Three independent checks agree:**
- The Foundry test `test_Vectors` deploys the verifier at that address on chain 11155111. It asserts every intermediate value, asserts that `vm.sign` reproduces both seals, and asserts that `verify` accepts the literal bytes.
- `cast wallet sign --no-hash` over the hand-built digest gives the same signatures.
- `cast wallet sign --data` over the EIP-712 JSON (domain and message above) gives the same signatures.

## What the attestor must do before signing

**Sign only for the vault's two image ids.** Read them from the vault (`updateImageId()`, `releaseImageId()`). Refuse any other image id.

### RELEASE (`imageId = releaseImageId`)

1. **Get the receipt and journal.** The user proves the release guest (G3, `methods/release_guest`) on their own machine and sends a RISC Zero **succinct receipt** with its 256-byte journal. Proving takes under a minute. The exit secret `k_j` and the Merkle path never leave the user's machine: the receipt reveals only the journal, and `release` publishes the journal's fields anyway.
2. **Verify the receipt** against `releaseImageId`, with the receipt verifier and control root that match the pinned risc0 version. Refuse a composite receipt or one with unresolved assumptions.
3. **Check the journal against the vault.** Decode it as `(cfgId, root, nullifier, token, denom, recipient, relayer, relayFee)`, then:
   - `cfgId` equals the vault's `cfgId()`;
   - `exitRootTime(root) ≠ 0`;
   - `releaseNullifierSpent(nullifier)` is false;
   - the recipient is not the vault and not zero.

   These are courtesy checks: the vault re-checks all of them. Optionally, `eth_call` the `release` with the seal before handing it out.
4. **Sign** `sha256(journal)`. The user or a relayer submits the release. Anyone can submit the seal, but it is bound to that recipient, relayer and fee.

### UPDATE (`imageId = updateImageId`)

1. **Run the outbound state guest** from the vault's **current** `g2State()` (the state whose commitment it is; a vault whose `g2State()` is zero, as v3's, can take no update at all) over the LEZ blocks the attestor accepts as final. Today that is the trusted-tip G2-lite build, `outbound_guest_trusted_tip`: the release build `outbound_guest` refuses every batch until the Logos Blockchain light client exists. Run it in the RISC Zero executor, and **prove** it when feasible. A proof would let anyone audit the operator later; it still would not be checked on chain.
2. **Check the journal** the guest produced:
   - `cfgId` equals the vault's;
   - `prevState` equals the vault's `g2State()` now;
   - **`newState ≠ prevState`, and `newState` has never been a state before.** G2-lite refuses an empty batch, so a journal it really produced always advances. Check it anyway: the vault refuses `newState == prevState` (`NoStateAdvance`), but not a revisited state (see the replay section below).
3. **Sign** `sha256(journal)` of exactly those bytes. Never sign a journal the attestor did not produce or re-derive itself.

### Operational rules

- **Rate-limit** signatures (per hour, and in value per token) to well below the vault's release caps.
- **Log** every request and every signature: inputs, journal, digest, seal and requester. Keep the log append-only.
- **Separate keys:** the attestor key is not the admin/freezer key (the script refuses that). The freezer must be able to stop a bad signature during the delay.
- **Freeze on doubt:** if the key may be exposed, freeze releases globally (`freezeAll(false, true)`, which also stops `update`) and deploy a new demo vault.

## Trust: what a bad attestor can and cannot do

**A malicious or compromised attestor CAN:**
- sign any update: claims and exits that never happened on LEZ;
- sign any release: to its own address, under the exit root it just created.

So it can take **up to the vault's whole balance of deposited principal**, per token, within the release caps and after the release delay, unless the freezer acts first. That is exactly the trust the real design removes (`test_RogueAttestorCanDrainDepositedPrincipal`).

It can also:
- **halt withdrawals** by refusing to sign (liveness);
- **link users to their releases through metadata:** who asked, from where and when. The content is public on chain anyway; the request metadata is not.

**What the unchanged vault still enforces, whatever is signed** (`test_RogueAttestorBoundedByVault`, `testFuzz_RogueAttestorSolvency`):
- **Per token:** released value ≤ burned value ≤ claimed value ≤ deposited value. Per (token, denomination): claims ≤ deposits and releases ≤ burns. Nothing beyond deposited principal can leave; fees stay.
- **Known roots only:** deposit roots must be ones the vault recorded, and releases need a recorded exit root.
- **`prevState` chaining**, release nullifiers, `cfgId`, and the payee checks.
- **Release caps** (per token and global), the release **delay**, and the **freeze**.

**The image ids pinned in the vault remain the real guest image ids,** so the vault runs exactly the logic it will run with real proofs. Only the check of "this journal came from that program" moves from Ethereum to the operator.

## Replay

`verify` is stateless (it must be `view`), so a seal for (imageId, digest) can be submitted again and again. That is harmless here:

- **Update:** the journal fixes `prevState`. After it is applied, `g2State` is `newState`, so a replay reverts with `StaleState`, now and after later updates (`test_ReplayedUpdateIsStale`).
  - **A journal with `newState == prevState`** would stay valid after it is applied, so anyone could replay it, re-adding its claims and exits each time. The vault refuses it with `NoStateAdvance`, signed or not (`test_NonAdvancingJournalRefusedEvenIfAttested`).
  - **Still the attestor's rule:** a journal that returns to an earlier state (A → B → A) would make the old A → B journal valid again. The vault does not track past states, so the attestor must never sign one. G2-lite cannot produce one: every batch moves the LEZ tip forward.
- **Release:** the second use reverts with `NullifierSpent` (`test_ReplayedReleaseIsSpent`). The nullifier depends only on the exit secret, so the same exit under a later root is also spent.
  - Changing the recipient, relayer, fee, root or nullifier changes the digest, so the seal fails (`NotAttestor`).
- **Across vaults and chains:** the domain binds the chain id and the verifier address. A shared verifier is covered by `cfgId` (see above).

## Expiry

From `EXPIRES_AT` on, every `verify` reverts with `Expired`, so the vault takes no update and pays no release. **Anything still in the vault then stays there for good.** That includes exits burned on LEZ but not yet released.

The vault itself does not know about the expiry and keeps accepting deposits. The web app must:
- stop offering deposits well before the expiry;
- warn users to finish withdrawals.

## Size and gas (local EVM, Foundry 1.5.1, `optimizer_runs = 200`, via-IR)

**Size:** runtime 2,340 bytes, initcode 2,594 bytes. `BridgeVault` is otherwise unchanged; with the `NoStateAdvance` check it is 20,642 bytes (20,614 in the v2 deploy).

| What | Demo verifier | `MockVerifier` | Real RISC Zero Groth16 (DESIGN §6.2) |
|---|---:|---:|---:|
| `verify` frame, called from the vault | 5,319 (of which `ecrecover` 3,000) | 723 | about 230k |
| `update`, 1 claim + 1 exit, same warm state | 153,967 | 151,868 | — |
| `release`, fee 0, same warm state | 62,316 | 57,691 | — |

- These are not Sepolia gas figures. Sepolia runs Glamsterdam pricing, and the absolute `update`/`release` numbers above come from deliberately warm storage. Use the differences: the demo verifier adds about 2–5k gas per call over the mock, and about 225k less than the real Groth16 verifier.
- A direct `verify` call from a test is 6,169 gas, including the cold-account surcharge.
