//! RFP-022 bridge program for LEZ 0.4: mints wrapped ERC-20s and ETH against deposits proven
//! in the Ethereum `BridgeVault`, and records burns as exit commitments for release on Ethereum.
//!
//! # Inbound: claim
//!
//! A depositor locked `denom` of `token` in the vault behind
//! `C = SHA256("RFP022/dep" | k | dest | token | denom | cfgId)`; the vault stored the leaf
//! `SHA256(DEPOSIT_LEAF_TAG | cfgId | token | denom | C)` in its SHA-256 tree. To claim, the
//! depositor sends [`Instruction::Claim`] to their own actor of this program, normally from a
//! fresh private account through a privacy-preserving transaction. [`instruct`] then:
//!
//! 1. recomputes `C` with `dest` = the signing account, and the leaf, and checks the Merkle
//!    path to `root`;
//! 2. asserts `root` is an attested vault deposit root ([`Effect::AssertRoot`]);
//! 3. spends the claim nullifier `nf = SHA256("RFP022/nfc" | k | dest | token | u256(denom))`
//!    ([`Effect::SpendClaim`]), which fails if it was spent before. It binds the destination,
//!    token and denomination the commitment binds, so whoever learns `k` (every public claim
//!    publishes it) cannot spend this deposit's nullifier with a deposit of their own made with
//!    the same `k` (security review 2026-11-14, H2);
//! 4. asks the config to mint ([`Effect::Claim`]), which checks the token and denomination,
//!    counts the claim, and mints `denom` through the token program to the claim's recipient
//!    (see "One-time recipient" below).
//!
//! A valid claim stays valid forever: delisting a token ([`AdminOp::DeregisterToken`]) only
//! tells the vault to take no new deposits, so deposits made before it can still be claimed.
//! Freezing minting is the only thing that stops claims.
//!
//! # Freezing
//!
//! Minting (claims) and redemption (burns) freeze independently, each globally on the config or
//! for one registered token ([`AdminOp::Freeze`], [`AdminOp::Unfreeze`]). A claim is refused if
//! minting is frozen globally or for its token, a burn if redemption is frozen globally or for
//! its token, and the stand-in [`Instruction::AttestRoot`] if minting is frozen globally. The
//! freezer or the admin may freeze; only the admin unfreezes. A refused claim gives the same
//! error whichever level froze it, so the failure says nothing more about the deposit.
//!
//! The secret `k`, the leaf index and the path stay inside the private execution. What becomes
//! public is the root used, the nullifier, the token and denomination, and the address
//! credited: none of it says which deposit was claimed.
//!
//! # One-time recipient
//!
//! A claim with `recipient_opening: Some(o)` mints to the one-time address `A = dest.blinded(o)`
//! (LEZ's blinded alias, `SHA256(tag | dest | o)`), computed by the claimer's own actor from its
//! resolved account id. The deposit commitment still binds the real account `dest`, so only
//! `dest` can claim, and only a transaction whose private witness carries `o` resolves `A` back
//! to `dest` and receives the credit inside its proof. The public `Effect::Claim`, the token's
//! `MintBy` and its `Credit` name `A`, never `dest`. Clients default `o` to
//! [`recipient_opening`] for a private claimer; `None` keeps the old behaviour (mint to the
//! signer itself), which a public claimer must use: an alias of a public account is neither
//! delivered nor publishable.
//!
//! # Float (fee sponsoring)
//!
//! A private transaction pays its fee from a private account inside the proof, and a fresh
//! private claimer holds nothing. The config's own account can hold native LEZ (the float);
//! anyone may top it up with a plain native transfer to [`config_address`]. When
//! [`Config::sponsor_amount`] is above zero, every claim to a one-time recipient also moves
//! `sponsor_amount` from the float to `A` in the same transaction
//! (`native_token::custody_transfer` under the config's seed). The claimer's wallet receives it
//! inside the proof and pays the fee from it. A claim spends a one-time nullifier, so each
//! deposit draws the float at most once. If the float holds less than `sponsor_amount` the
//! native transfer fails and so does the whole claim: nothing is minted, the nullifier stays
//! unspent. [`AdminOp::SetSponsor`] sets the amount (0 turns sponsoring off). The float has no
//! withdrawal path: only sponsored claims draw it.
//!
//! What still becomes public for a private claim: the token, the denomination, the time, the
//! root (stand-in) or the anchor (RFP-021) used, the nullifier, the one-time address `A`, and the
//! fixed sponsor amount paid to `A`.
//!
//! # Outbound: burn
//!
//! A holder burns exactly one denomination with the token program's `BurnWithNotice`, naming
//! this program's config as `notify` and the exit commitment
//! `E = SHA256("RFP022/exit" | k' | ethDest | token | denom | maxRelayFee | cfgId)` as memo.
//! The config checks the token and amount, counts the burn, and emits an [`Exited`] event. The
//! outbound state proof (G2) reads `E` from that transaction. Burns are not sponsored: the burner
//! pays its own fee (see the README for why).
//!
//! The burn bound is by value, per token: the total value burned can never exceed the total
//! value claimed. Wrapped tokens are fungible, so a holder of ten 100s can exit even if the only
//! claim was one 1000. Each burn is still exactly one denomination.
//!
//! # Initialisation and deployment id
//!
//! Only the deployer initialises: [`Instruction::Initialize`] / [`Instruction::InitializeAttested`]
//! must be signed by this program's own loader header account (on LEZ rc3 the program id IS
//! that account's id, whose key the deployer holds; the instruction reaches the header's own
//! actor of this program, so `receiver.account_id == program`). Nobody can front-run it, and the
//! roles it sets (admin, freezer, attestor) come from the instruction and may not be the header
//! itself, so after init the header key holds no power over the bridge (security review
//! 2026-11-14, H1, C1). The older [`Instruction::Init`] / [`Instruction::InitAttested`], which made
//! whoever sent them first the admin, are refused; they stay in the enum so older blocks decode.
//!
//! The config does not take `cfgId` from anyone. [`instruct`] derives
//! `cfgId = SHA256(CFG_TAG | u256(evmChainId) | vault | lezChainId | zoneId | programId)` with
//! this program's own id ([`cfg_id`]), where
//! - `lezChainId` is the LEZ chain's Bedrock channel id, declared at init and never zero. LEZ
//!   rc3 gives a program no way to read its chain's identity (the channel id lives in the
//!   `sequencer_stake` config, which answers no other program), so the program cannot check it:
//!   it binds a vault to one *declared* chain, and the CLI checks it against the chain's own
//!   `sequencer_stake` config before it sends `Initialize` (review M1);
//! - `zoneId` is the root-source id [`source_id`], derived from the config's root source (for
//!   RFP-021: the attestation program, the instance, whose id commits to the checkpoint, the
//!   fork schedule and the genesis validators root, and the vault slot), never supplied. A vault
//!   built with it names, on Ethereum, the one attestation instance its deposits are claimed
//!   through (review M4).
//!
//! The vault computes the same value from its own address and `block.chainid`; if they differ,
//! no deposit leaf matches and nothing can be claimed.
//!
//! # Roots
//!
//! Vault roots reach this program through an attestation source, fixed when the config is
//! created ([`Config::root_source`]; no instruction changes it):
//!
//! - **Stand-in** ([`Instruction::Init`]): an attestor account records roots
//!   ([`Instruction::AttestRoot`]) and a claim asserts the root PDA ([`Instruction::Claim`]).
//! - **RFP-021** ([`Instruction::InitAttested`]): no roots are recorded on LEZ at all. A claim
//!   ([`Instruction::ClaimAttested`]) carries an `eth_getProof` account + storage proof that the
//!   vault's `isDepositRoot[root]` mapping entry is `true` (slot
//!   `keccak256(abi.encode(root, N))`, [`VAULT_DEPOSIT_ROOT_SLOT`]) under an execution anchor;
//!   the claimer's own actor verifies it with the [`eth_attest_core::inclusion`] library, and
//!   the only public trace is `AssertAnchor(instance, chain, anchor)` to the configured
//!   attestation instance, which refuses unless that instance finalized the anchor. The root
//!   itself is no longer published.
//!
//! In both modes the claim nullifier is this program's own [`claim_nullifier`]: the attested
//! statement is the same for every deposit under a root, so it can't key uniqueness.
//!
//! # PDA admission
//!
//! Every effect goes to a PDA of this program (config, root, nullifier) and carries the seed that
//! derives it (`SyncMessage::with_pda_seeds`). LEZ admits a public account the transaction
//! reaches for the first time if the delivering program presents its seed, so a new nullifier or
//! root PDA needs no admission evidence in the transaction (evidence is unsigned and a forwarder
//! could strip it). A seeded delivery also authorizes the PDA; no handler here reads
//! `is_authorized` on a PDA, and the float's custody transfer carries the config seed itself.
//!
//! The attestation program id, instance id, vault, chain id and slot index a claimer uses are
//! witness values: the claim effect carries each of them and the config refuses the claim
//! unless they equal its own. Without that, a claimer could point `AssertAnchor` at a program
//! that agrees to anything (RFP-021 Verification Security #3).
//!
//! # Refusals (RFP-022 Privacy #5, Usability #8)
//!
//! Every claim refusal that depends on the deposit is the one text [`CLAIM_REFUSED`]: a witness
//! that proves no deposit under its root (a leaf that does not reach the root for this secret,
//! signer, token, amount and cfg id; a malformed path or index; a vault storage proof or anchor
//! witness that does not verify: every [`ClaimFault`]), a root no attestor recorded
//! ([`Effect::AssertRoot`]), and a spent nullifier, i.e. a repeat claim ([`Effect::SpendClaim`]).
//! The bridge has no claim caps today; one added later must refuse with the same text, after
//! the nullifier, like the checks below.
//!
//! No panic text in the claim, burn or root paths carries a value: asserts are
//! `assert!(cond, "fixed text")`, decoders fail with fixed texts, and an [`InclusionError`] (whose
//! `Display` carries the trie path, `keccak(slot)` of the claimed root, and node bytes) is never
//! printed here. `tests/panic_texts.rs` checks the source.
//!
//! These keep a text of their own, because they depend only on public state or on public fields
//! of the claim's own effect and are the same for every deposit, and Usability #8 asks for them
//! to be reportable: `minting is frozen` (the config's or the token's flag, one text for both),
//! `claim for another deployment` (cfg id), `claim for another attestation source`,
//! `token not registered`, `bad denomination`, `wrong amount for this denomination`, the
//! stand-in/RFP-021 mismatch, and, from the attestation program, `anchor not finalized by this
//! instance` or `anchor is for another chain` ("insufficient finality"; the anchor is public in
//! the claim and shared by every deposit proven under it). An anchor the instance never recorded
//! has no PDA, and the bridge cannot seed another program's PDA, so on LEZ that case fails as the
//! runtime's `UnadmittedPublicActor` before the attestation program runs (the simulator admits
//! it and shows the program's text); either way it names only the public anchor. A burn's refusals (`redemption is frozen`, `not a bridged token`,
//! `a burn is exactly one denomination`, `burns exceed claims`) speak of the definition and the
//! amount, which the burn publishes anyway, never of its exit commitment.
//!
//! What the program controls of a refused claim's public footprint: the claimer's actor builds
//! the same three messages in the same order to the same kinds of PDA for every claim (the
//! root or anchor assertion, the nullifier spend, the config claim), from the instruction alone.
//! It reads no state, so it cannot branch on whether the deposit was claimed, frozen or capped.
//! A witness that proves nothing panics in that actor before any message exists. What it does
//! not control:
//! - A private claim that would fail anywhere fails the CLI wallet's simulation of the whole
//!   transaction (`send_tx_with_pre_check`) and is never proven or sent: nothing is published.
//!   One that passed simulation and fails at the sequencer (the state changed in between: a
//!   freeze, the same claim sent twice) is included and reverted with its three messages public;
//!   which of them failed follows from public state (the root or anchor PDA, the nullifier PDA,
//!   the config), whatever the text says. A repeat names the same nullifier PDA as the claim
//!   that spent it, which links the two transactions, not the deposit.
//! - A public claimer publishes its root instruction, secret included, by design.
//! - No refusal identifies the deposit: the nullifier `SHA256(tag | k | dest | token | denom)` is
//!   unlinkable to the leaf without `k`, and a root or anchor is shared by every deposit under it.
//!
//! The CLI tells its user which case applies (root not attested, anchor not finalized, proof
//! does not verify, already claimed, frozen, wrong denomination) by running the same checks
//! natively ([`check_deposit_in_root`], [`check_attested_root`]) and reading public state before
//! it proves or sends; that diagnosis stays on the user's machine.

use borsh::{BorshDeserialize, BorshSerialize};
use bridge_token_core::{BurnNotice, Message as TokenMessage};
use eth_attest_core::{
    assert_anchor, assert_instance,
    inclusion::{
        AnchorWitness, InclusionError, Predicate, StatementContext, StorageProof, StorageStatement, mapping_slot,
        verify_storage_slot, word,
    },
};
use lee_core::{
    account::{AccountId, Actor, ActorState, Balance},
    native_token::custody_transfer,
    program::{PdaSeed, ProgramEvent, ReceiveInput, Response, SyncMessage},
};
use risc0_zkvm::sha::{Impl, Sha256 as _};

// ---------------------------------------------------------------------------------------------
// Constants shared with the Ethereum vault (BridgeVault.sol). Changing any of them breaks every
// proof and every deposit, so they are fixed per vault version.

/// `keccak256("RFP022/deposit-leaf/v1")`, BridgeVault.DEPOSIT_LEAF_TAG.
pub const DEPOSIT_LEAF_TAG: [u8; 32] =
    hex32("9e8d098bfe4f2807258ba6b200534afcacae50702d4735cb40a3f31e9bea950c");
/// `keccak256("RFP022/exit-leaf/v1")`, BridgeVault.EXIT_LEAF_TAG.
pub const EXIT_LEAF_TAG: [u8; 32] =
    hex32("22018742ad6028fda6cb0726afaae90ea20dff3cef6b1ec5b3c14f856d76c59c");
/// `keccak256("RFP022/cfg/v1")`, the domain tag of [`cfg_id`].
pub const CFG_TAG: [u8; 32] =
    hex32("26e962302b1dc9417c466ff56ef206a578cd054a0a099172cff8b80460ab5a45");
pub const TREE_DEPTH: usize = 32;
pub const DEP_DOMAIN: &[u8] = b"RFP022/dep";
pub const NFC_DOMAIN: &[u8] = b"RFP022/nfc";
/// Domain of [`source_id`].
pub const SOURCE_DOMAIN: &[u8] = b"RFP022/source/v1";
/// The most [`AdminOp::SetSponsor`] may set: about two private claims' fees plus a burn's at
/// today's prices (a private claim cost about 9.1M on the local sequencer; the demo sets 25M).
/// Fixed in the program, so one claim can never draw more than this from the float (review M2).
pub const MAX_SPONSOR_AMOUNT: Balance = 50_000_000;
pub const EXIT_DOMAIN: &[u8] = b"RFP022/exit";
pub const NFR_DOMAIN: &[u8] = b"RFP022/nfr";
/// Domain of the clients' default one-time recipient opening ([`recipient_opening`]).
pub const ALIAS_DOMAIN: &[u8] = b"RFP022/alias";
pub const MAX_TOKENS: usize = 32;
pub const MAX_DENOMS: usize = 8;
/// Storage slot index of `mapping(bytes32 => bool) isDepositRoot` in `BridgeVault`, from
/// `forge inspect BridgeVault storage-layout` (2026-11-12). It depends on every state variable
/// declared before it: re-check it whenever the vault's storage layout changes, and configure
/// the new value in a new bridge config.
pub const VAULT_DEPOSIT_ROOT_SLOT: u64 = 83;

const CONFIG_SEED: [u8; 32] = *b"/RFP022/bridge/config/v1/000000/";
const ROOT_SEED_DOMAIN: &[u8] = b"/RFP022/bridge/root/v1";
const NULLIFIER_SEED_DOMAIN: &[u8] = b"/RFP022/bridge/claim-nullifier/v1";

const fn hex32(s: &str) -> [u8; 32] {
    let b = s.as_bytes();
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        out[i] = (nib(b[2 * i]) << 4) | nib(b[2 * i + 1]);
        i += 1;
    }
    out
}

const fn nib(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        _ => panic!("bad hex"),
    }
}

/// An Ethereum address.
pub type EthAddress = [u8; 20];
/// ETH itself, as the vault encodes it (`address(0)`).
pub const ETH: EthAddress = [0; 20];

// ---------------------------------------------------------------------------------------------
// Hashes

#[must_use]
pub fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let bytes: Vec<u8> = parts.concat();
    Impl::hash_bytes(&bytes)
        .as_bytes()
        .try_into()
        .unwrap_or_else(|_| unreachable!())
}

/// A u128 as Solidity's 32-byte big-endian `uint256`.
#[must_use]
pub fn u256(v: u128) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[16..].copy_from_slice(&v.to_be_bytes());
    out
}

/// The deployment id that binds deposits and exits to one vault and one bridge program:
/// `SHA256(CFG_TAG | u256(evm_chain_id) | vault | lez_chain_id | zone_id | program)`, 180 bytes,
/// the same as the vault's
/// `sha256(abi.encodePacked(tag, uint256(block.chainid), address(this), lezChainId, zoneId, programId))`.
/// `program` is this bridge program's account id, as its 32 raw bytes.
#[must_use]
pub fn cfg_id(
    evm_chain_id: u64,
    vault: &EthAddress,
    lez_chain_id: &[u8; 32],
    zone_id: &[u8; 32],
    program: &AccountId,
) -> [u8; 32] {
    sha256(&[&CFG_TAG, &u256(u128::from(evm_chain_id)), vault, lez_chain_id, zone_id, program.value()])
}

/// The deposit commitment `C`. Client-side and inside [`instruct`]; the vault never sees its parts.
#[must_use]
pub fn deposit_commitment(secret: &[u8; 32], dest: &AccountId, token: &EthAddress, denom: Balance, cfg_id: &[u8; 32]) -> [u8; 32] {
    sha256(&[DEP_DOMAIN, secret, dest.value(), token, &u256(denom), cfg_id])
}

/// The leaf the vault stores: `sha256(abi.encodePacked(DEPOSIT_LEAF_TAG, cfgId, token, denom, C))`.
#[must_use]
pub fn deposit_leaf(cfg_id: &[u8; 32], token: &EthAddress, denom: Balance, commitment: &[u8; 32]) -> [u8; 32] {
    sha256(&[&DEPOSIT_LEAF_TAG, cfg_id, token, &u256(denom), commitment])
}

/// The vault's exit leaf for an exit commitment.
#[must_use]
pub fn exit_leaf(cfg_id: &[u8; 32], token: &EthAddress, denom: Balance, commitment: &[u8; 32]) -> [u8; 32] {
    sha256(&[&EXIT_LEAF_TAG, cfg_id, token, &u256(denom), commitment])
}

/// The claim nullifier `SHA256("RFP022/nfc" | k | dest | token | u256(denom))`, 104 bytes in:
/// what the commitment binds except `cfgId` (the nullifier PDA is already this program's own).
/// Bound to `dest`, so a second deposit made with the same `k` for another account has another
/// nullifier and cannot burn this one (security review 2026-11-14, H2).
#[must_use]
pub fn claim_nullifier(secret: &[u8; 32], dest: &AccountId, token: &EthAddress, denom: Balance) -> [u8; 32] {
    sha256(&[NFC_DOMAIN, secret, dest.value(), token, &u256(denom)])
}

/// The root-source id that fills the vault's `zoneId` in [`cfg_id`]:
/// `SHA256("RFP022/source/v1" | borsh(source))`, i.e. `0x00` for the stand-in, or
/// `0x01 | attest_program (32) | attest_instance (32) | u64le(deposit_root_slot)`. Derived by the
/// program at init, never supplied.
#[must_use]
pub fn source_id(source: &RootSource) -> [u8; 32] {
    sha256(&[SOURCE_DOMAIN, &borsh::to_vec(source).unwrap_or_else(|_| panic!("source serializes"))])
}

/// The exit commitment `E` a burn carries as its memo.
#[must_use]
pub fn exit_commitment(
    secret: &[u8; 32],
    eth_dest: &EthAddress,
    token: &EthAddress,
    denom: Balance,
    max_relay_fee: Balance,
    cfg_id: &[u8; 32],
) -> [u8; 32] {
    sha256(&[EXIT_DOMAIN, secret, eth_dest, token, &u256(denom), &u256(max_relay_fee), cfg_id])
}

#[must_use]
pub fn release_nullifier(secret: &[u8; 32]) -> [u8; 32] {
    sha256(&[NFR_DOMAIN, secret])
}

/// Client-side default opening of a claim's one-time recipient: `SHA256("RFP022/alias" | k)`.
/// Deterministic per deposit, so a retried claim uses the same address; it never leaves the
/// claimer's machine.
#[must_use]
pub fn recipient_opening(secret: &[u8; 32]) -> [u8; 32] {
    sha256(&[ALIAS_DOMAIN, secret])
}

/// The address a claim by `dest` with `opening` mints to: `dest` itself for `None`, else the
/// one-time alias `dest.blinded(opening)`.
#[must_use]
pub fn claim_recipient(dest: &AccountId, opening: Option<&[u8; 32]>) -> AccountId {
    opening.map_or(*dest, |o| dest.blinded(o))
}

/// The root reached from `leaf` at `index` along `path` (sibling at each level, leaf first),
/// with the vault's convention: bit `h` of `index` set means the node is a right child.
/// Panics unless `path` has [`TREE_DEPTH`] siblings and `index` fits the tree; programs use
/// [`checked_merkle_root`].
#[must_use]
pub fn merkle_root(leaf: &[u8; 32], index: u64, path: &[[u8; 32]]) -> [u8; 32] {
    checked_merkle_root(leaf, index, path).unwrap_or_else(|| panic!("a Merkle path has {TREE_DEPTH} siblings and a leaf index below 2^{TREE_DEPTH}"))
}

/// [`merkle_root`] of an untrusted path: `None` unless it has [`TREE_DEPTH`] siblings and
/// `index` is below `2^TREE_DEPTH`.
#[must_use]
pub fn checked_merkle_root(leaf: &[u8; 32], index: u64, path: &[[u8; 32]]) -> Option<[u8; 32]> {
    if path.len() != TREE_DEPTH || index >= (1u64 << TREE_DEPTH) {
        return None;
    }
    let mut node = *leaf;
    for (h, sibling) in path.iter().enumerate() {
        node = if (index >> h) & 1 == 1 {
            sha256(&[sibling, &node])
        } else {
            sha256(&[&node, sibling])
        };
    }
    Some(node)
}

/// Why a claim's private witness proves no deposit. A native caller (the CLI) may show it, with
/// [`InclusionError`]'s details, to its own user; the program refuses every one with
/// [`CLAIM_REFUSED`] and prints none of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimFault {
    /// The path is not [`TREE_DEPTH`] siblings, or the leaf index is outside the tree.
    PathShape,
    /// The deposit leaf for this secret, signer, token, amount and cfg id does not reach the
    /// root along the path: another account's deposit, another amount, a typo in the secret, or
    /// a root that does not hold the deposit (yet).
    NotInRoot,
    /// The vault's `isDepositRoot[root]` is not proven true under the anchor.
    Inclusion(InclusionError),
}

impl core::fmt::Display for ClaimFault {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::PathShape => write!(f, "the Merkle path is not {TREE_DEPTH} siblings, or the leaf index is outside the tree"),
            Self::NotInRoot => write!(
                f,
                "the deposit (this secret, claiming account, token, amount and cfg id) is not under this root"
            ),
            Self::Inclusion(e) => write!(f, "the vault proof does not show this root as a deposit root: {e}"),
        }
    }
}

/// Checks a claim's deposit witness: the leaf `deposit_leaf(cfg_id, token, denom,
/// deposit_commitment(secret, signer, ..))` reaches `root` at `leaf_index` along `path`. What
/// [`Instruction::Claim`] and [`Instruction::ClaimAttested`] check in the claimer's actor.
///
/// # Errors
/// [`ClaimFault::PathShape`] or [`ClaimFault::NotInRoot`].
#[allow(clippy::too_many_arguments)]
pub fn check_deposit_in_root(
    secret: &[u8; 32],
    signer: &AccountId,
    eth_token: &EthAddress,
    denom: Balance,
    cfg_id: &[u8; 32],
    root: &[u8; 32],
    leaf_index: u64,
    path: &[[u8; 32]],
) -> Result<(), ClaimFault> {
    let commitment = deposit_commitment(secret, signer, eth_token, denom, cfg_id);
    let leaf = deposit_leaf(cfg_id, eth_token, denom, &commitment);
    match checked_merkle_root(&leaf, leaf_index, path) {
        None => Err(ClaimFault::PathShape),
        Some(r) if r == *root => Ok(()),
        Some(_) => Err(ClaimFault::NotInRoot),
    }
}

/// Checks an attested claim's vault proof: `isDepositRoot[root]` (slot
/// `keccak256(abi.encode(root, source.deposit_root_slot))` of `vault`) is 1 under the anchor's
/// state root. What [`Instruction::ClaimAttested`] checks in the claimer's actor; the anchor
/// itself must still be finalized by the instance (`AssertAnchor`, public).
///
/// # Errors
/// [`ClaimFault::Inclusion`].
pub fn check_attested_root(
    source: &AttestSource,
    vault: &EthAddress,
    evm_chain_id: u64,
    root: &[u8; 32],
    anchor: &AnchorWitness,
    proof: &StorageProof,
) -> Result<StorageStatement, ClaimFault> {
    let slot = mapping_slot(root, source.deposit_root_slot);
    let context = StatementContext {
        evm_chain_id,
        instance_id: source.attest_instance,
    };
    verify_storage_slot(&context, anchor, vault, &slot, Predicate::Eq(word(1)), proof).map_err(ClaimFault::Inclusion)
}

/// `zeros[h]`: the root of an empty subtree of height `h`, as in the vault.
#[must_use]
pub fn zero_hashes() -> [[u8; 32]; TREE_DEPTH + 1] {
    let mut z = [[0u8; 32]; TREE_DEPTH + 1];
    for h in 0..TREE_DEPTH {
        z[h + 1] = sha256(&[&z[h], &z[h]]);
    }
    z
}

/// Client-side helper: the path for leaf `index` in a tree holding `leaves` (in vault order).
#[must_use]
pub fn merkle_path(leaves: &[[u8; 32]], index: usize) -> Vec<[u8; 32]> {
    let zeros = zero_hashes();
    let mut layer: Vec<[u8; 32]> = leaves.to_vec();
    let mut idx = index;
    let mut path = Vec::with_capacity(TREE_DEPTH);
    for zero in zeros.iter().take(TREE_DEPTH) {
        let sib = idx ^ 1;
        path.push(layer.get(sib).copied().unwrap_or(*zero));
        let next: Vec<[u8; 32]> = layer
            .chunks(2)
            .map(|pair| sha256(&[&pair[0], pair.get(1).unwrap_or(zero)]))
            .collect();
        layer = next;
        idx >>= 1;
    }
    path
}

// ---------------------------------------------------------------------------------------------
// Addresses

#[must_use]
pub const fn config_seed() -> PdaSeed {
    PdaSeed::new(CONFIG_SEED)
}

#[must_use]
pub fn root_seed(root: &[u8; 32]) -> PdaSeed {
    PdaSeed::new(sha256(&[ROOT_SEED_DOMAIN, root]))
}

#[must_use]
pub fn nullifier_seed(nullifier: &[u8; 32]) -> PdaSeed {
    PdaSeed::new(sha256(&[NULLIFIER_SEED_DOMAIN, nullifier]))
}

#[must_use]
pub fn config_address(program: &AccountId) -> AccountId {
    AccountId::for_public_pda(program, &config_seed())
}

#[must_use]
pub fn root_address(program: &AccountId, root: &[u8; 32]) -> AccountId {
    AccountId::for_public_pda(program, &root_seed(root))
}

#[must_use]
pub fn nullifier_address(program: &AccountId, nullifier: &[u8; 32]) -> AccountId {
    AccountId::for_public_pda(program, &nullifier_seed(nullifier))
}

// ---------------------------------------------------------------------------------------------
// State

/// One bridged Ethereum token.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct TokenEntry {
    pub eth_token: EthAddress,
    /// The wrapped token's definition in the token program; its mint authority must be this
    /// program's account id.
    pub definition: AccountId,
    pub decimals: u8,
    /// Same denominations, same order, as the vault's. Append-only, no duplicates.
    pub denoms: Vec<Balance>,
    /// Per denomination: claims minted and burns recorded (for accounting; the bound is by value).
    pub claims: Vec<u64>,
    pub burns: Vec<u64>,
    /// Total value minted by claims and destroyed by recorded burns. `burned_value` never
    /// exceeds `claimed_value`.
    pub claimed_value: Balance,
    pub burned_value: Balance,
    /// false: delisted, the vault takes no new deposits of this token. Claims of earlier deposits
    /// and burns still go through here: a valid claim stays valid forever. Freezing stops them.
    pub registered: bool,
    /// This token's claims (minting) are refused.
    pub mint_frozen: bool,
    /// This token's burns (redemption) are refused.
    pub burn_frozen: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Config {
    /// Binds the vault address, EVM chain, LEZ chain, root source and this program; derived by
    /// [`cfg_id`] at init, never supplied. Must equal the vault's cfgId.
    pub cfg_id: [u8; 32],
    pub vault: EthAddress,
    pub evm_chain_id: u64,
    /// The LEZ chain's Bedrock channel id (declared at init; see the crate docs).
    pub lez_chain_id: [u8; 32],
    /// The vault's `zoneId`: [`source_id`] of [`Config::root_source`] (configs made by
    /// [`Instruction::Initialize`]; older configs hold whatever their `Init` named).
    pub zone_id: [u8; 32],
    pub token_program: AccountId,
    pub admin: AccountId,
    pub pending_admin: Option<AccountId>,
    pub freezer: AccountId,
    /// Stand-in attestation source until RFP-021 runs on LEZ 0.4.
    pub attestor: AccountId,
    /// Every claim (minting) and every stand-in root attestation is refused.
    pub mint_frozen: bool,
    /// Every burn (redemption) is refused.
    pub burn_frozen: bool,
    pub tokens: Vec<TokenEntry>,
    /// Where vault roots come from. Written at init; no instruction changes it.
    pub root_source: RootSource,
    /// Native LEZ the config pays from its own balance (the float) to the one-time recipient of
    /// every claim that has one, in the claim's transaction. 0: no sponsoring. Admin sets it.
    pub sponsor_amount: Balance,
}

/// The attestation source a config trusts for vault deposit roots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum RootSource {
    /// The stand-in attestor ([`Config::attestor`], [`Instruction::AttestRoot`]).
    StandIn,
    /// An RFP-021 attestation instance.
    Attest(AttestSource),
}

/// An RFP-021 attestation instance and how to read the vault's deposit roots under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AttestSource {
    /// The attestation program (deployed with an immutable header).
    pub attest_program: AccountId,
    /// The instance id inside it; its chain id must be the config's `evm_chain_id`.
    pub attest_instance: [u8; 32],
    /// The slot index N of `isDepositRoot` in the vault ([`VAULT_DEPOSIT_ROOT_SLOT`]).
    pub deposit_root_slot: u64,
}

impl Config {
    fn decode(state: &[u8]) -> Self {
        assert!(!state.is_empty(), "the bridge is not initialised");
        let Ok(config) = borsh::from_slice(state) else {
            panic!("config state does not decode")
        };
        config
    }

    fn encode(&self) -> ActorState {
        ActorState::from(borsh::to_vec(self).unwrap_or_else(|_| panic!("config serializes")))
    }

    fn token_mut(&mut self, eth_token: &EthAddress) -> &mut TokenEntry {
        self.tokens
            .iter_mut()
            .find(|t| &t.eth_token == eth_token)
            .unwrap_or_else(|| panic!("token not registered"))
    }
}

/// Marker state for a root or nullifier PDA.
const MARK: [u8; 1] = [1];

/// The one refusal for every claim failure that depends on the deposit: a witness that proves no
/// deposit under an attested root or anchor ("rejected claim"), a root no attestor recorded, a
/// spent nullifier ("repeat claim"), and any future claim cap. See "Refusals" in the crate docs.
pub const CLAIM_REFUSED: &str = "claim refused";
/// The one error for a frozen claim or root attestation, whichever flag fired: a refused claim
/// must not say more about the deposit than that minting is frozen.
pub const MINT_FROZEN: &str = "minting is frozen";
/// The one error for a frozen burn, whichever flag fired.
pub const BURN_FROZEN: &str = "redemption is frozen";
/// A stand-in root or claim on a config whose roots come from RFP-021.
const STAND_IN_OFF: &str = "this bridge takes roots from an RFP-021 attestation, not the stand-in";
/// An attested claim whose source, vault or chain differs from the config's.
const WRONG_SOURCE: &str = "claim for another attestation source";
/// `Initialize` signed by anyone but this program's header account.
pub const INIT_BY_HEADER: &str = "only this program's header account initialises the bridge";
/// The retired first-come `Init` / `InitAttested`.
pub const INIT_RETIRED: &str = "Init is retired: the program's header account sends Initialize";

// ---------------------------------------------------------------------------------------------
// Messages

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Message {
    /// From a transaction, to the signer's own actor of this program.
    Instruction(Instruction),
    /// Between this program's own actors only.
    Effect(Effect),
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Instruction {
    /// RETIRED (refused): made whoever sent it first the admin (review H1). Kept so older blocks
    /// decode; use [`Instruction::Initialize`].
    Init {
        vault: EthAddress,
        evm_chain_id: u64,
        lez_chain_id: [u8; 32],
        zone_id: [u8; 32],
        token_program: AccountId,
        freezer: AccountId,
        attestor: AccountId,
    },
    Admin(AdminOp),
    /// Stand-in attestation: record a vault deposit root. Only the configured attestor.
    AttestRoot { root: [u8; 32] },
    /// Claim one deposit; see the crate docs. The signer's account is the destination.
    Claim {
        eth_token: EthAddress,
        denom_index: u32,
        /// The denomination's amount. The leaf is checked for it here and the config refuses
        /// it unless it is `denoms[denom_index]`, so a claim can't mint more than was locked.
        denom: Balance,
        root: [u8; 32],
        leaf_index: u64,
        path: Vec<[u8; 32]>,
        secret: [u8; 32],
        /// Must equal the config's; checked there. Carried so the private part can compute
        /// the leaf without reading the config.
        cfg_id: [u8; 32],
        /// `Some(o)`: mint to the one-time address `signer.blinded(o)` instead of the signer
        /// (see the crate docs). Clients default it for private claimers.
        recipient_opening: Option<[u8; 32]>,
    },
    /// RETIRED (refused), as [`Instruction::Init`]; use [`Instruction::InitializeAttested`].
    InitAttested {
        vault: EthAddress,
        evm_chain_id: u64,
        lez_chain_id: [u8; 32],
        zone_id: [u8; 32],
        token_program: AccountId,
        freezer: AccountId,
        source: AttestSource,
    },
    /// [`Instruction::Claim`] through RFP-021: instead of an attested root PDA, proves that the
    /// vault's `isDepositRoot[root]` is true under a finalized execution anchor. `source`,
    /// `vault` and `evm_chain_id` must equal the config's (checked there).
    ClaimAttested {
        eth_token: EthAddress,
        denom_index: u32,
        denom: Balance,
        root: [u8; 32],
        leaf_index: u64,
        path: Vec<[u8; 32]>,
        secret: [u8; 32],
        cfg_id: [u8; 32],
        source: AttestSource,
        vault: EthAddress,
        evm_chain_id: u64,
        /// The anchor, with the execution header RLP for a block-hash anchor.
        anchor: AnchorWitness,
        /// `eth_getProof(vault, [keccak256(abi.encode(root, N))])` at that block.
        proof: StorageProof,
        /// As in [`Instruction::Claim`].
        recipient_opening: Option<[u8; 32]>,
    },
    /// Create the config, stand-in root source. Signed by this program's own header account
    /// only (`receiver.account_id == program`); refused once the config exists. `admin`,
    /// `freezer` and `attestor` may not be the header. `lez_chain_id` (the chain's Bedrock
    /// channel id) may not be zero. The config's `zone_id` is [`source_id`] and its `cfg_id` is
    /// derived ([`cfg_id`]).
    Initialize {
        admin: AccountId,
        vault: EthAddress,
        evm_chain_id: u64,
        lez_chain_id: [u8; 32],
        token_program: AccountId,
        freezer: AccountId,
        attestor: AccountId,
    },
    /// [`Instruction::Initialize`] for a config whose roots come from an RFP-021 attestation
    /// instance. Also asks the attestation program to confirm the instance exists for
    /// `evm_chain_id`.
    InitializeAttested {
        admin: AccountId,
        vault: EthAddress,
        evm_chain_id: u64,
        lez_chain_id: [u8; 32],
        token_program: AccountId,
        freezer: AccountId,
        source: AttestSource,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum AdminOp {
    /// `denoms` must match the vault's for this token, in order.
    RegisterToken {
        eth_token: EthAddress,
        definition: AccountId,
        decimals: u8,
        denoms: Vec<Balance>,
    },
    AddDenomination { eth_token: EthAddress, denom: Balance },
    DeregisterToken { eth_token: EthAddress },
    /// Freeze minting (`mint`, i.e. claims) and/or redemption (`burn`) for one token, or for
    /// everything with `None`. A false field leaves that flag as it is; both false is refused.
    /// Freezer or admin.
    Freeze { eth_token: Option<EthAddress>, mint: bool, burn: bool },
    /// Clear the named flags, as [`AdminOp::Freeze`] sets them. Admin only.
    Unfreeze { eth_token: Option<EthAddress>, mint: bool, burn: bool },
    SetFreezer { freezer: AccountId },
    SetAttestor { attestor: AccountId },
    ProposeAdmin { next: AccountId },
    AcceptAdmin,
    /// Set [`Config::sponsor_amount`] (0 turns sponsoring off). Admin only.
    SetSponsor { amount: Balance },
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Effect {
    /// To the config PDA.
    Init(Config),
    /// To the config PDA; `by` is the signing account.
    Admin { by: AccountId, op: AdminOp },
    /// To the config PDA; `by` is the signing account.
    AttestRoot { by: AccountId, root: [u8; 32] },
    /// To a root PDA, from the config.
    RecordRoot,
    /// To a root PDA: refused unless the root was recorded.
    AssertRoot,
    /// To a nullifier PDA: refused if already spent.
    SpendClaim,
    /// To the config PDA, from a claimer's actor that has checked the deposit proof. `one_time`:
    /// `to` is a one-time alias the claimer's actor derived (the float sponsors it).
    Claim {
        eth_token: EthAddress,
        denom_index: u32,
        denom: Balance,
        to: AccountId,
        cfg_id: [u8; 32],
        one_time: bool,
    },
    /// To the config PDA, from a claimer's actor that has checked the deposit proof and the
    /// vault storage proof against `source`, `vault` and `evm_chain_id`.
    ClaimAttested {
        eth_token: EthAddress,
        denom_index: u32,
        denom: Balance,
        to: AccountId,
        cfg_id: [u8; 32],
        source: AttestSource,
        vault: EthAddress,
        evm_chain_id: u64,
        one_time: bool,
    },
}

/// Emitted by the config for every recorded burn. G2 also reads `E` from the transaction itself.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Exited {
    pub eth_token: EthAddress,
    pub denom_index: u32,
    pub exit_commitment: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Claimed {
    pub eth_token: EthAddress,
    pub denom_index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct RootRecorded {
    pub root: [u8; 32],
}

fn selector(name: &str) -> [u8; 8] {
    sha256(&[b"rfp022_bridge::", name.as_bytes()])[..8]
        .try_into()
        .unwrap_or_else(|_| unreachable!())
}

fn event<T: BorshSerialize>(name: &str, data: &T) -> ProgramEvent {
    ProgramEvent {
        selector: selector(name),
        data: borsh::to_vec(data).unwrap_or_else(|_| panic!("event serializes")),
    }
}

/// An effect to this program's PDA under `seed`, carrying the seed so LEZ admits a new PDA by
/// it (see "PDA admission" in the crate docs).
fn effect(seed: PdaSeed, program: &AccountId, effect: Effect) -> SyncMessage {
    SyncMessage::new(Actor::new(AccountId::for_public_pda(program, &seed), *program), &Message::Effect(effect))
        .with_pda_seeds([seed])
}

// ---------------------------------------------------------------------------------------------
// Entry point

/// The guest's entry point. Messages from this program or from a transaction decode as
/// [`Message`]; a message from another program can only be the token program's [`BurnNotice`].
#[must_use]
pub fn handle(input: &ReceiveInput) -> Response {
    match input.from {
        Some(from) if from.program_account_id != input.receiver.program_account_id => {
            let Ok(notice) = borsh::from_slice::<BurnNotice>(&input.message) else {
                panic!("a burn notice must decode")
            };
            burn_notice(input, from, notice)
        }
        _ => {
            let Ok(message) = borsh::from_slice::<Message>(&input.message) else {
                panic!("message must decode")
            };
            match message {
                Message::Instruction(instruction) => instruct(input, instruction),
                Message::Effect(e) => {
                    assert!(input.from_own_program(), "only this program may send an effect");
                    apply(input, e)
                }
            }
        }
    }
}

/// Turns a signed instruction into effects on the actors it concerns.
#[must_use]
pub fn instruct(input: &ReceiveInput, instruction: Instruction) -> Response {
    assert!(input.from.is_none(), "an instruction is sent by the transaction itself");
    assert!(input.is_authorized, "the signer's account must sign");
    let program = input.receiver.program_account_id;
    // The resolved account: LEZ hands a handler its canonical id, never an alias it was reached at.
    let me = input.receiver.account_id;
    let config = config_seed();
    match instruction {
        Instruction::Init { .. } | Instruction::InitAttested { .. } => panic!("{INIT_RETIRED}"),
        Instruction::Initialize { admin, vault, evm_chain_id, lez_chain_id, token_program, freezer, attestor } => {
            let config = new_config(program, me, admin, vault, evm_chain_id, lez_chain_id, token_program, freezer, attestor, RootSource::StandIn);
            Response::keep_state().send(effect(config_seed(), &program, Effect::Init(config)))
        }
        Instruction::InitializeAttested { admin, vault, evm_chain_id, lez_chain_id, token_program, freezer, source } => {
            // No stand-in attestor: AttestRoot is refused on this config.
            let none = AccountId::new([0; 32]);
            let config =
                new_config(program, me, admin, vault, evm_chain_id, lez_chain_id, token_program, freezer, none, RootSource::Attest(source));
            Response::keep_state()
                .send(assert_instance(&source.attest_program, &source.attest_instance, evm_chain_id))
                .send(effect(config_seed(), &program, Effect::Init(config)))
        }
        Instruction::Admin(op) => Response::keep_state().send(effect(config, &program, Effect::Admin { by: me, op })),
        Instruction::AttestRoot { root } => {
            Response::keep_state().send(effect(config, &program, Effect::AttestRoot { by: me, root }))
        }
        Instruction::Claim {
            eth_token,
            denom_index,
            denom,
            root,
            leaf_index,
            path,
            secret,
            cfg_id,
            recipient_opening,
        } => {
            // The commitment binds the real account; only the mint goes to the one-time address.
            // Every failure here is CLAIM_REFUSED, never the computed root (a witness value).
            check_deposit_in_root(&secret, &me, &eth_token, denom, &cfg_id, &root, leaf_index, &path)
                .unwrap_or_else(|_| panic!("{CLAIM_REFUSED}"));
            let nullifier = claim_nullifier(&secret, &me, &eth_token, denom);
            // The same three messages, in this order, for every claim: built from the
            // instruction alone, whatever the root, nullifier or config will say.
            Response::keep_state()
                .send(effect(root_seed(&root), &program, Effect::AssertRoot))
                .send(effect(nullifier_seed(&nullifier), &program, Effect::SpendClaim))
                .send(effect(
                    config,
                    &program,
                    Effect::Claim {
                        eth_token,
                        denom_index,
                        denom,
                        to: claim_recipient(&me, recipient_opening.as_ref()),
                        cfg_id,
                        one_time: recipient_opening.is_some(),
                    },
                ))
        }
        Instruction::ClaimAttested {
            eth_token,
            denom_index,
            denom,
            root,
            leaf_index,
            path,
            secret,
            cfg_id,
            source,
            vault,
            evm_chain_id,
            anchor,
            proof,
            recipient_opening,
        } => {
            check_deposit_in_root(&secret, &me, &eth_token, denom, &cfg_id, &root, leaf_index, &path)
                .unwrap_or_else(|_| panic!("{CLAIM_REFUSED}"));
            // isDepositRoot[root] == true in the vault, under the anchor's state root. Runs here,
            // in the claimer's (private) execution: the root, slot and proof are not published.
            // An InclusionError is never printed: its Display carries keccak(slot) of the root.
            let statement = check_attested_root(&source, &vault, evm_chain_id, &root, &anchor, &proof)
                .unwrap_or_else(|_| panic!("{CLAIM_REFUSED}"));
            let nullifier = claim_nullifier(&secret, &me, &eth_token, denom);
            Response::keep_state()
                // Public: the anchor is one the configured instance finalized.
                .send(assert_anchor(&source.attest_program, &source.attest_instance, evm_chain_id, &statement.anchor))
                .send(effect(nullifier_seed(&nullifier), &program, Effect::SpendClaim))
                .send(effect(
                    config,
                    &program,
                    Effect::ClaimAttested {
                        eth_token,
                        denom_index,
                        denom,
                        to: claim_recipient(&me, recipient_opening.as_ref()),
                        cfg_id,
                        source,
                        vault,
                        evm_chain_id,
                        one_time: recipient_opening.is_some(),
                    },
                ))
        }
    }
}

/// The config an `Initialize` creates, after the deployer checks (see "Initialisation" in the
/// crate docs). `me` is the signer's resolved account, which must be `program`'s header account
/// (the program id is the header account's id).
#[allow(clippy::too_many_arguments)]
fn new_config(
    program: AccountId,
    me: AccountId,
    admin: AccountId,
    vault: EthAddress,
    evm_chain_id: u64,
    lez_chain_id: [u8; 32],
    token_program: AccountId,
    freezer: AccountId,
    attestor: AccountId,
    root_source: RootSource,
) -> Config {
    assert!(me == program, "{INIT_BY_HEADER}");
    assert!(lez_chain_id != [0; 32], "init needs the LEZ chain id (its Bedrock channel id)");
    assert!(evm_chain_id != 0 && vault != [0; 20], "init needs the vault and its chain id");
    assert!(
        admin != program && freezer != program && attestor != program && token_program != program,
        "the header account holds no role"
    );
    let zone_id = source_id(&root_source);
    Config {
        cfg_id: cfg_id(evm_chain_id, &vault, &lez_chain_id, &zone_id, &program),
        vault,
        evm_chain_id,
        lez_chain_id,
        zone_id,
        token_program,
        admin,
        pending_admin: None,
        freezer,
        attestor,
        mint_frozen: false,
        burn_frozen: false,
        tokens: Vec::new(),
        root_source,
        sponsor_amount: 0,
    }
}

fn apply(input: &ReceiveInput, e: Effect) -> Response {
    let program = input.receiver.program_account_id;
    let here = input.receiver.account_id;
    match e {
        Effect::Init(config) => {
            assert!(here == config_address(&program), "init goes to the config");
            assert!(input.pre_state.is_empty(), "the bridge is already initialised");
            Response::set_state(config.encode())
        }
        Effect::Admin { by, op } => {
            assert!(here == config_address(&program), "admin goes to the config");
            let mut c = Config::decode(&input.pre_state);
            admin(&mut c, by, op);
            Response::set_state(c.encode())
        }
        Effect::AttestRoot { by, root } => {
            assert!(here == config_address(&program), "attestation goes to the config");
            let c = Config::decode(&input.pre_state);
            assert!(c.root_source == RootSource::StandIn, "{STAND_IN_OFF}");
            assert!(by == c.attestor, "only the attestor records roots");
            assert!(!c.mint_frozen, "{MINT_FROZEN}");
            Response::keep_state()
                .send(effect(root_seed(&root), &program, Effect::RecordRoot))
                .event(event("RootRecorded", &RootRecorded { root }))
        }
        Effect::RecordRoot => Response::set_state(ActorState::from(MARK.to_vec())),
        // Not attested ("rejected claim") and already claimed ("repeat claim") are one refusal
        // (RFP-022 Privacy #5); the CLI tells its user which, locally.
        Effect::AssertRoot => {
            assert!(!input.pre_state.is_empty(), "{CLAIM_REFUSED}");
            Response::keep_state()
        }
        Effect::SpendClaim => {
            assert!(input.pre_state.is_empty(), "{CLAIM_REFUSED}");
            Response::set_state(ActorState::from(MARK.to_vec()))
        }
        Effect::Claim {
            eth_token,
            denom_index,
            denom,
            to,
            cfg_id,
            one_time,
        } => {
            assert!(here == config_address(&program), "a claim goes to the config");
            let c = Config::decode(&input.pre_state);
            assert!(cfg_id == c.cfg_id, "claim for another deployment");
            assert!(c.root_source == RootSource::StandIn, "{STAND_IN_OFF}");
            mint_claim(c, here, eth_token, denom_index, denom, to, one_time)
        }
        Effect::ClaimAttested {
            eth_token,
            denom_index,
            denom,
            to,
            cfg_id,
            source,
            vault,
            evm_chain_id,
            one_time,
        } => {
            assert!(here == config_address(&program), "a claim goes to the config");
            let c = Config::decode(&input.pre_state);
            assert!(cfg_id == c.cfg_id, "claim for another deployment");
            // Every value the claimer's actor verified against must be this config's own.
            assert!(
                c.root_source == RootSource::Attest(source) && vault == c.vault && evm_chain_id == c.evm_chain_id,
                "{WRONG_SOURCE}"
            );
            mint_claim(c, here, eth_token, denom_index, denom, to, one_time)
        }
    }
}

/// Counts a proven claim and mints its denomination to `to` through the token program. For a
/// one-time recipient, also pays it `sponsor_amount` from the float (the config's own native
/// balance); an empty or short float fails the native transfer and with it the whole claim.
fn mint_claim(
    mut c: Config,
    config: AccountId,
    eth_token: EthAddress,
    denom_index: u32,
    denom: Balance,
    to: AccountId,
    one_time: bool,
) -> Response {
    assert!(!c.mint_frozen, "{MINT_FROZEN}");
    let token_program = c.token_program;
    // No `registered` check: a delisted token's earlier deposits stay claimable.
    let t = c.token_mut(&eth_token);
    assert!(!t.mint_frozen, "{MINT_FROZEN}");
    let i = denom_index as usize;
    assert!(i < t.denoms.len(), "bad denomination");
    let amount = t.denoms[i];
    assert!(amount == denom, "wrong amount for this denomination");
    t.claims[i] = t.claims[i].checked_add(1).unwrap_or_else(|| panic!("claim count overflow"));
    t.claimed_value = t.claimed_value.checked_add(amount).unwrap_or_else(|| panic!("claimed value overflow"));
    let definition = t.definition;
    let sponsor = c.sponsor_amount;
    let response = Response::set_state(c.encode())
        .send_sync(Actor::new(definition, token_program), &TokenMessage::MintBy { to, amount })
        .event(event("Claimed", &Claimed { eth_token, denom_index }));
    if one_time && sponsor > 0 {
        response.send(custody_transfer(config, config_seed(), to, sponsor))
    } else {
        response
    }
}

fn admin(c: &mut Config, by: AccountId, op: AdminOp) {
    match op {
        AdminOp::Freeze { eth_token, mint, burn } => {
            assert!(by == c.freezer || by == c.admin, "only the freezer or admin freezes");
            set_frozen(c, eth_token, mint, burn, true);
            return;
        }
        AdminOp::AcceptAdmin => {
            assert!(Some(by) == c.pending_admin, "only the proposed admin accepts");
            c.admin = by;
            c.pending_admin = None;
            return;
        }
        _ => {}
    }
    assert!(by == c.admin, "only the admin");
    match op {
        AdminOp::RegisterToken {
            eth_token,
            definition,
            decimals,
            denoms,
        } => {
            assert!(c.tokens.iter().all(|t| t.eth_token != eth_token), "token already registered");
            // One wrapped definition per Ethereum token: burns are matched by definition, and G2
            // refuses a shared one (review M3).
            assert!(c.tokens.iter().all(|t| t.definition != definition), "definition already registered");
            assert!(c.tokens.len() < MAX_TOKENS, "too many tokens");
            assert!(!denoms.is_empty() && denoms.len() <= MAX_DENOMS, "bad denominations");
            assert!(denoms.iter().all(|d| *d > 0), "a denomination is positive");
            assert!(
                denoms.iter().enumerate().all(|(i, d)| !denoms[..i].contains(d)),
                "duplicate denomination"
            );
            if eth_token == ETH {
                assert!(decimals == 18, "ETH has 18 decimals");
            }
            let n = denoms.len();
            c.tokens.push(TokenEntry {
                eth_token,
                definition,
                decimals,
                denoms,
                claims: vec![0; n],
                burns: vec![0; n],
                claimed_value: 0,
                burned_value: 0,
                registered: true,
                mint_frozen: false,
                burn_frozen: false,
            });
        }
        AdminOp::AddDenomination { eth_token, denom } => {
            let t = c.token_mut(&eth_token);
            assert!(denom > 0 && t.denoms.len() < MAX_DENOMS, "bad denomination");
            assert!(!t.denoms.contains(&denom), "duplicate denomination");
            t.denoms.push(denom);
            t.claims.push(0);
            t.burns.push(0);
        }
        AdminOp::DeregisterToken { eth_token } => c.token_mut(&eth_token).registered = false,
        AdminOp::Unfreeze { eth_token, mint, burn } => set_frozen(c, eth_token, mint, burn, false),
        AdminOp::SetFreezer { freezer } => c.freezer = freezer,
        AdminOp::SetAttestor { attestor } => c.attestor = attestor,
        AdminOp::ProposeAdmin { next } => c.pending_admin = Some(next),
        AdminOp::SetSponsor { amount } => {
            assert!(amount <= MAX_SPONSOR_AMOUNT, "sponsor amount above the program's cap");
            c.sponsor_amount = amount;
        }
        AdminOp::Freeze { .. } | AdminOp::AcceptAdmin => unreachable!(),
    }
}

/// Sets (`to` true) or clears the flags named by `mint` and `burn`, globally (`None`) or for one
/// token; an unnamed flag is left as it is.
fn set_frozen(c: &mut Config, eth_token: Option<EthAddress>, mint: bool, burn: bool, to: bool) {
    assert!(mint || burn, "name minting, redemption or both");
    let (mint_flag, burn_flag) = match eth_token {
        None => (&mut c.mint_frozen, &mut c.burn_frozen),
        Some(t) => {
            let t = c.token_mut(&t);
            (&mut t.mint_frozen, &mut t.burn_frozen)
        }
    };
    if mint {
        *mint_flag = to;
    }
    if burn {
        *burn_flag = to;
    }
}

/// A burn reported by the token program: record the exit.
fn burn_notice(input: &ReceiveInput, from: Actor, notice: BurnNotice) -> Response {
    let program = input.receiver.program_account_id;
    // Fixed texts only: none names the exit commitment (the memo), the burner or an amount.
    assert!(input.receiver.account_id == config_address(&program), "burn notices go to the config");
    let mut c = Config::decode(&input.pre_state);
    assert!(from.program_account_id == c.token_program, "a burn notice comes from the token program");
    assert!(!c.burn_frozen, "{BURN_FROZEN}");
    let t = c
        .tokens
        .iter_mut()
        .find(|t| t.definition == notice.definition)
        .unwrap_or_else(|| panic!("not a bridged token"));
    assert!(!t.burn_frozen, "{BURN_FROZEN}");
    let i = t
        .denoms
        .iter()
        .position(|d| *d == notice.amount)
        .unwrap_or_else(|| panic!("a burn is exactly one denomination"));
    t.burns[i] = t.burns[i].checked_add(1).unwrap_or_else(|| panic!("burn count overflow"));
    // By value, not per denomination: wrapped tokens are fungible, so any mix of denominations
    // may exit as long as the token's total burned value stays within its total claimed value.
    t.burned_value = t.burned_value.checked_add(notice.amount).unwrap_or_else(|| panic!("burned value overflow"));
    assert!(t.burned_value <= t.claimed_value, "burns exceed claims");
    let ev = Exited {
        eth_token: t.eth_token,
        denom_index: u32::try_from(i).unwrap_or_else(|_| unreachable!()),
        exit_commitment: notice.memo,
    };
    Response::set_state(c.encode()).event(event("Exited", &ev))
}
