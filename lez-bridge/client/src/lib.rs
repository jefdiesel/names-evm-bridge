//! RFP-022 bridge client logic: seed-derived secrets, deposit requests, and the recovery scan
//! that rediscovers every unclaimed deposit and unreleased burn from the wallet seed plus public
//! chain data alone (RFP-022 Functionality #9, Reliability #5, "Loss of access").
//!
//! Pure logic. The CLI (`lez-bridge deposit-request | claim --key-index | burn --key-index |
//! recover | positions`) supplies the wallet keys and the chain sources.
//!
//! # Key derivation (spec, v1)
//!
//! **Root.** Each LEZ account has its own root, derived from secret key material the wallet
//! already holds and re-derives from the seed phrase, so nothing new is ever backed up:
//! - public account: its 32-byte signing key (`UserKeyChain::pub_account_signing_key`);
//! - private account: its authorization secret key `ask` (`KeyChain.private_key_holder.
//!   authorization_secret_key`), the key that authorizes spending, not the viewing key: a holder
//!   of an exit secret chooses the relayer and fee of the release, so a view-only party must
//!   never be able to derive it.
//!
//! The LEZ wallet does not keep the 64-byte BIP-39 seed after building its key trees (LEZ
//! `key_protocol` `KeyChain::new_mnemonic`, `Storage::new`), so the root cannot be "the seed";
//! it is a per-account key that the seed determines. `PRK = HMAC-SHA256(key =
//! "RFP022/bridge/root/v1", msg = account_secret)` (HKDF-Extract). It lives only in memory.
//!
//! **Secrets.** HKDF-Expand to 32 bytes, one HMAC block:
//!
//! ```text
//! info = "RFP022/bridge/key/v1"(20) | cfgId(32) | kind(1) | accountId(32) | index(u64 BE, 8) | ctx
//! out  = HMAC-SHA256(key = PRK, msg = info | 0x01)
//! kind 1 (deposit):  ctx = token(20) | u256(denom)                                -> master s
//! kind 2 (exit):     ctx = token(20) | u256(denom) | ethDest(20) | u256(maxRelayFee) -> master s'
//! kind 3 (release address key): ctx = token(20) | u256(denom) | u256(maxRelayFee)  -> secp256k1 key
//! deposit claim secret k  = SHA256("RFP022/ck" | s)    (what C, nf_c, the opening and the claim use)
//! exit secret          k' = SHA256("RFP022/ek" | s')   (what E and nf_r use)
//! reclaim nullifier       = SHA256("RFP022/nfrc" | s)  (future Ethereum-side failsafe; no use yet)
//! ```
//!
//! - `cfgId` binds a secret to one deployment (vault + bridge program + chains): two deployments
//!   never share a secret.
//! - **Every input of the commitment is in `ctx`.** The nullifiers hash the secret alone
//!   (`SHA256("RFP022/nfc" | k | dest | token | u256(denom))`, `SHA256("RFP022/nfr" | k')`), so one secret behind two
//!   different commitments would make the second position unclaimable or unreleasable forever.
//!   With every input bound, reusing an index (a restored client with no cache, two devices on
//!   one seed) either reproduces the same `C`, which the vault refuses before taking any funds
//!   (`CommitmentUsed`), or gives an unrelated secret. The same `E` burned twice is the one
//!   remaining case; `burn` refuses an `E` already in the chain data before sending.
//! - Each tuple has its own index sequence: deposits per (account, token, denom), exits per
//!   (account, token, denom, destination, maxRelayFee). This deviates from a single sequence per
//!   (cfgId, account, kind): with one sequence a forgotten address or fee could push another
//!   tuple's exits past the gap limit.
//! - Master/derived split: a public-account claim publishes `k` by design, and a claim included
//!   and reverted leaves its nullifier unspent; `s` never appears anywhere, so the future
//!   reclaim path can ask for `s`. Deposits made with a raw `--secret k` (the older scripts and
//!   the Sepolia demo deposits) have no `s` and will not be reclaimable through that failsafe.
//! - The one-time recipient opening is unchanged: `SHA256("RFP022/alias" | k)`.
//! - Test vectors: `tests/vectors/kdf-v1.json`, computed independently with Python's `hmac` and
//!   `hashlib` (`tests/vectors/gen_kdf_vectors.py`) and Foundry's `cast wallet address` for the
//!   release addresses; `tests/kdf.rs` pins them.
//!
//! # Exit destinations: what the user supplies at recovery
//!
//! `E` hashes `ethDest` and `maxRelayFee`, and the burn's only free field is the 32-byte memo,
//! which must be `E` (G2 reads it). LEZ 0.4 transactions have no free-form field either
//! (`TransactionMessage`: public context, execution, nonces, admission evidence). So the scan
//! can only find an exit whose `ethDest` and `maxRelayFee` it can enumerate:
//! - **`ethDest`: a seed-derived release address by default** (kind 3 key, a fresh address per
//!   exit, which also keeps releases from sharing an Ethereum address). Recovery needs nothing.
//!   The user exports its key (`lez-bridge eth-key`) to move the released funds; an ERC-20
//!   release there needs some ETH for gas to move on.
//! - **A user-chosen address** (`burn --eth-dest 0x..`) works, but recovery then needs the user
//!   to name that address again (`recover --eth-dest 0x..`). It is not a secret generated during
//!   the flow, so Functionality #9's wording is met; the "Loss of access" section's "seed alone,
//!   with no other information" is met only for derived destinations.
//! - **`maxRelayFee`:** the default is the 2% cap `floor(denom * 200 / 10_000)`, a vault
//!   constant that admits every relay fee any menu can ever hold. The scan also tries 0 and every
//!   value ever set on the menu (rebuilt from `RelayFeeMenuSet` logs, not the current view);
//!   `burn` refuses any other value.
//! - **Next version (program change):** a burn that also carries a constant-size encrypted
//!   note-to-self (`ethDest | maxRelayFee | index` under a seed-derived key, pseudorandom and the
//!   same size for everyone) would make any user-chosen address recoverable from the seed alone.
//!   It needs a new `BurnWithNotice` field, so new token and bridge programs.
//!
//! # Recovery scan
//!
//! 1. Ethereum: every `Deposit`, `StateUpdated`, `Released` and `RelayFeeMenuSet` log of the
//!    vault, in bounded block ranges (split when the provider refuses a range, retried with a
//!    pause, resumable from a checkpoint: [`chain::fetch_logs`]). The deposit tree is rebuilt
//!    and checked against every logged root, which catches a missed range.
//! 2. LEZ: every block from the bridge's first block, keeping the non-reverted transactions
//!    that name the bridge config ([`chain::scan_lez`]). Reverted ones are skipped: a reverted
//!    claim names an unspent nullifier, a reverted burn carries an `E` that never exited.
//! 3. For each account and tuple, indexes 0, 1, 2... until `gap` (default 20) consecutive
//!    unused ones: deposits by commitment lookup; exits by searching the kept transactions'
//!    bytes for `E` ([`scan::recover`]).
//! 4. Classify: a deposit is claimable until its claim-nullifier PDA appears in a kept
//!    transaction (claimed). An exit is burned; in the exit tree (its exit leaf is in a
//!    `StateUpdated`); or released (its release nullifier is in a `Released` log).
//!
//! # Client state
//!
//! [`cache::ClientCache`] holds next indexes (including handed-out ones not seen on chain), the
//! finalized logs and LEZ transactions fetched so far, and the last result. It is a cache:
//! `recover` with no cache rebuilds everything. Allocation takes the larger of the cached and
//! the recovered next index.
//!
//! # What the scan reveals
//!
//! - **Ethereum RPC:** the vault's address and the block ranges, the same `eth_getLogs` filter for
//!   every user. Nothing about which deposits are the user's. Plus the client's network address.
//! - **LEZ sequencer, block scan (default):** that the client downloads blocks. Nothing about
//!   which nullifiers or exits it looks for.
//! - **LEZ sequencer, point queries (`--lez-mode point`, faster):** the claim-nullifier PDA of each
//!   found deposit. Before the claim nobody else can compute that PDA, so the query, tied to the
//!   client's network address and time, links the user to the later claim transaction; after it,
//!   to that claim. Residual leak under Privacy #4/#6, opt-in only.

pub mod cache;
pub mod chain;
pub mod hexser;
pub mod kdf;
pub mod request;
pub mod scan;

/// Runs a future whose sources answer at once (the in-memory fakes of the tests).
///
/// # Panics
/// If the future is not ready after many polls (a source that really waits).
pub fn block_on<F: core::future::Future>(f: F) -> F::Output {
    let mut f = core::pin::pin!(f);
    let waker = core::task::Waker::noop();
    let mut cx = core::task::Context::from_waker(waker);
    for _ in 0..1_000_000 {
        if let core::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
    panic!("block_on: the future never completed")
}
