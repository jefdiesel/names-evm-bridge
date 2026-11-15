//! Seed-derived bridge secrets (the spec is in the crate docs, "Key derivation").
//!
//! Every bridge secret comes from the LEZ account it belongs to, through HKDF-SHA256 (RFC 5869):
//!
//! ```text
//! PRK  = HMAC-SHA256(key = "RFP022/bridge/root/v1", msg = account_secret)          (Extract)
//! info = "RFP022/bridge/key/v1" | cfgId(32) | kind(1) | accountId(32) | index(u64 BE) | ctx
//! out  = HMAC-SHA256(key = PRK, msg = info | 0x01)                                  (Expand, L = 32)
//! ```
//!
//! For kinds 1 and 2 `out` is a MASTER secret (`s`, `s'`) that never appears in a transaction.
//! The programs' "secret" is one hash further: deposit `k = SHA256("RFP022/ck" | s)`, exit
//! `k' = SHA256("RFP022/ek" | s')`. The future Ethereum-side reclaim will ask for `s`
//! (nullifier `SHA256("RFP022/nfrc" | s)`), which a published `k` does not give.
//!
//! `ctx` binds every other input of the commitment the secret goes into:
//! - kind 1, deposit: `token(20) | u256(denom)`;
//! - kind 2, exit: `token(20) | u256(denom) | ethDest(20) | u256(maxRelayFee)`;
//! - kind 3, release-address key: `token(20) | u256(denom) | u256(maxRelayFee)`.
//!
//! # Names app scheme (the Names Wallet's `app_secret`)
//!
//! Under the Names Wallet the bridge mini-app never holds an account key. It asks the wallet's
//! `app_secret(V, scope, context, tag, i, n)` (Names `basecamp-module/rust-lib/src/app_secret.rs`,
//! branch `bridge-sdk`), which derives, for the calling module `app`:
//!
//! ```text
//! PRK  = HMAC-SHA256(key = "logos-names/app-secret/v1", msg = account_secret)        (Extract)
//! info = "logos-names/app-secret/v1/info" | u8 len(app) | app | u8 len(scope) | scope
//!        | u16be len(context) | context | accountId(32) | index(u64 BE)
//! m    = HMAC-SHA256(key = PRK, msg = info | 0x01)          (Expand, L = 32; stays in the wallet)
//! value = SHA256(tag | m)                                    (what the app gets)
//! ```
//!
//! The bridge maps its kinds onto that ([`AccountRoot::names_app`]), with `context = cfgId(32) |
//! ctx` (the v1 `ctx` above, so the token is 20 bytes):
//! - deposit: scope `RFP022/deposit`, tag `RFP022/ck`: the value is `k` (`m` is the master `s`);
//! - exit: scope `RFP022/exit`, tag `RFP022/ek`: the value is `k'`;
//! - release-address key: scope `RFP022/release`, tag `RFP022/release-key`: the value is the
//!   secp256k1 key; in the 2^-128 case that it is not a valid scalar, `SHA256` of it, again until
//!   it is (the app can do that from the value alone).
//!
//! The same account secret gives unrelated values under the two schemes, so deposits made under
//! one are found only by a scan under the same one (the CLI keeps one cache per scheme).

use bridge_core::{EthAddress, u256};
use hmac::{Hmac, Mac as _};
use k256::elliptic_curve::sec1::ToEncodedPoint as _;
use lee_core::account::AccountId;
use sha2::Sha256;
use sha3::{Digest as _, Keccak256};
use zeroize::Zeroize as _;

/// HKDF-Extract salt: the domain of every bridge secret.
pub const ROOT_SALT: &[u8] = b"RFP022/bridge/root/v1";
/// First bytes of every HKDF-Expand `info`.
pub const INFO_TAG: &[u8] = b"RFP022/bridge/key/v1";
/// `BridgeVault.MAX_FEE_BPS`: no fee, relay fees included, may exceed 2% of a denomination.
pub const MAX_FEE_BPS: u128 = 200;

/// What a derived secret is for. The byte is part of `info`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A deposit secret `k` (claim nullifier `SHA256("RFP022/nfc" | k | dest | token | u256(denom))`).
    Dep = 1,
    /// An exit secret `k_j` (release nullifier `SHA256("RFP022/nfr" | k_j)`).
    Exit = 2,
    /// The secp256k1 key of a seed-derived Ethereum release address.
    Release = 3,
}

/// HMAC-SHA256.
#[must_use]
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).unwrap_or_else(|_| unreachable!("HMAC takes any key length"));
    mac.update(msg);
    mac.finalize().into_bytes().into()
}

/// Which derivation an [`AccountRoot`] follows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scheme {
    /// The bridge's own v1 KDF (this module's docs): the CLI's own wallet.
    BridgeV1,
    /// The Names Wallet's `app_secret` for the module named `app` (module docs, "Names app
    /// scheme"); the bridge mini-app's module is `lez_bridge`.
    NamesApp { app: String },
}

/// One account's pseudorandom key (the HKDF PRK). Built from the account's own secret key
/// material, held only in memory, wiped on drop, never printed.
pub struct AccountRoot {
    prk: [u8; 32],
    scheme: Scheme,
}

impl AccountRoot {
    /// `account_secret` is the account's 32-byte secret: for a public account its signing key
    /// (`UserKeyChain::pub_account_signing_key`), for a private account its authorization secret
    /// key `ask` (`KeyChain.private_key_holder.authorization_secret_key`). Both are re-derived by
    /// the wallet from the seed phrase, so restoring the phrase restores every bridge secret.
    #[must_use]
    pub fn from_account_secret(account_secret: &[u8; 32]) -> Self {
        Self { prk: hmac_sha256(ROOT_SALT, account_secret), scheme: Scheme::BridgeV1 }
    }

    /// The same account secret under the Names Wallet's `app_secret` for module `app`: what the
    /// wallet gives that module, reproduced from the account's key (for `recover` from the
    /// recovery phrase with the CLI's own wallet). `None` if `app` is not a name the Names
    /// provider accepts (1-64 of `A-Za-z0-9_-.`).
    #[must_use]
    pub fn names_app(account_secret: &[u8; 32], app: &str) -> Option<Self> {
        names::valid_app(app).then(|| Self { prk: hmac_sha256(names::SALT, account_secret), scheme: Scheme::NamesApp { app: app.to_owned() } })
    }

    #[must_use]
    pub const fn scheme(&self) -> &Scheme {
        &self.scheme
    }

    /// The `out`/`m` of `kind` for `index`: v1 HKDF-Expand with the `info` layout of the module
    /// docs, or the Names master `m` with the kind's scope and `context = cfgId | ctx`.
    #[must_use]
    pub fn derive(&self, cfg_id: &[u8; 32], kind: Kind, account: &AccountId, index: u64, ctx: &[u8]) -> [u8; 32] {
        match &self.scheme {
            Scheme::BridgeV1 => hmac_sha256(&self.prk, &info(cfg_id, kind, account, index, ctx, &[1])),
            Scheme::NamesApp { app } => {
                let context = [cfg_id.as_slice(), ctx].concat();
                names::master_from_prk(&self.prk, app, names::scope_of(kind), &context, account.value(), index)
            }
        }
    }
}

/// The Names Wallet's `app_secret` formula, byte for byte (Names `app_secret.rs`). Pinned by
/// the Names branch's own vectors in `tests/kdf.rs`.
pub mod names {
    use super::{Kind, hmac_sha256};

    /// HKDF-Extract salt.
    pub const SALT: &[u8] = b"logos-names/app-secret/v1";
    /// First bytes of every HKDF-Expand `info`.
    pub const INFO_TAG: &[u8] = b"logos-names/app-secret/v1/info";
    /// The bridge module's name in Basecamp: what the wallet keys its values to.
    pub const BRIDGE_APP: &str = "lez_bridge";
    pub const DEPOSIT_SCOPE: &str = "RFP022/deposit";
    pub const EXIT_SCOPE: &str = "RFP022/exit";
    pub const RELEASE_SCOPE: &str = "RFP022/release";
    /// `k = SHA256("RFP022/ck" | s)`: the same domain as v1's [`super::CK_DOMAIN`].
    pub const DEPOSIT_TAG: &str = "RFP022/ck";
    pub const EXIT_TAG: &str = "RFP022/ek";
    pub const RELEASE_TAG: &str = "RFP022/release-key";

    /// The provider's rule for an app name (Names `app_secret::Ask::new`).
    #[must_use]
    pub fn valid_app(app: &str) -> bool {
        !app.is_empty() && app.len() <= 64 && app.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    }

    #[must_use]
    pub const fn scope_of(kind: Kind) -> &'static str {
        match kind {
            Kind::Dep => DEPOSIT_SCOPE,
            Kind::Exit => EXIT_SCOPE,
            Kind::Release => RELEASE_SCOPE,
        }
    }

    #[must_use]
    pub const fn tag_of(kind: Kind) -> &'static str {
        match kind {
            Kind::Dep => DEPOSIT_TAG,
            Kind::Exit => EXIT_TAG,
            Kind::Release => RELEASE_TAG,
        }
    }

    /// HKDF-Expand `info` (lengths saturate as the Names code does; the bridge's are far below).
    #[must_use]
    pub fn info(app: &str, scope: &str, context: &[u8], account: &[u8; 32], index: u64) -> Vec<u8> {
        let mut info = Vec::with_capacity(INFO_TAG.len() + 2 + app.len() + scope.len() + 2 + context.len() + 40);
        info.extend_from_slice(INFO_TAG);
        info.push(u8::try_from(app.len()).unwrap_or(u8::MAX));
        info.extend_from_slice(app.as_bytes());
        info.push(u8::try_from(scope.len()).unwrap_or(u8::MAX));
        info.extend_from_slice(scope.as_bytes());
        info.extend_from_slice(&u16::try_from(context.len()).unwrap_or(u16::MAX).to_be_bytes());
        info.extend_from_slice(context);
        info.extend_from_slice(account);
        info.extend_from_slice(&index.to_be_bytes());
        info
    }

    /// `m` from an already extracted PRK.
    #[must_use]
    pub fn master_from_prk(prk: &[u8; 32], app: &str, scope: &str, context: &[u8], account: &[u8; 32], index: u64) -> [u8; 32] {
        let mut message = info(app, scope, context, account, index);
        message.push(1);
        hmac_sha256(prk, &message)
    }

    /// `m` for `index` (never given to an app; only [`value`] is).
    #[must_use]
    pub fn master(account_secret: &[u8; 32], app: &str, scope: &str, context: &[u8], account: &[u8; 32], index: u64) -> [u8; 32] {
        master_from_prk(&hmac_sha256(SALT, account_secret), app, scope, context, account, index)
    }

    /// `SHA256(tag | m)`: what `app_secret` returns.
    #[must_use]
    pub fn tagged(tag: &str, master: &[u8; 32]) -> [u8; 32] {
        bridge_core::sha256(&[tag.as_bytes(), master])
    }

    /// The values `app_secret(account, scope, context, tag, first, count)` returns.
    #[must_use]
    pub fn values(account_secret: &[u8; 32], app: &str, scope: &str, context: &[u8], tag: &str, account: &[u8; 32], first: u64, count: u64) -> Vec<[u8; 32]> {
        (first..first.saturating_add(count)).map(|i| tagged(tag, &master(account_secret, app, scope, context, account, i))).collect()
    }

    /// The `context` of a deposit value: `cfgId(32) | token(20) | u256(denom)` (84 bytes).
    #[must_use]
    pub fn deposit_context(cfg_id: &[u8; 32], token: &bridge_core::EthAddress, denom: u128) -> Vec<u8> {
        [cfg_id.as_slice(), &super::deposit_ctx(token, denom)].concat()
    }

    /// The `context` of an exit value: `cfgId | token | u256(denom) | ethDest(20) | u256(maxRelayFee)` (136 bytes).
    #[must_use]
    pub fn exit_context(cfg_id: &[u8; 32], token: &bridge_core::EthAddress, denom: u128, eth_dest: &bridge_core::EthAddress, max_relay_fee: u128) -> Vec<u8> {
        [cfg_id.as_slice(), &super::exit_ctx(token, denom, eth_dest, max_relay_fee)].concat()
    }

    /// The `context` of a release-address key: `cfgId | token | u256(denom) | u256(maxRelayFee)` (116 bytes).
    #[must_use]
    pub fn release_context(cfg_id: &[u8; 32], token: &bridge_core::EthAddress, denom: u128, max_relay_fee: u128) -> Vec<u8> {
        [cfg_id.as_slice(), &super::release_ctx(token, denom, max_relay_fee)].concat()
    }
}

impl Drop for AccountRoot {
    fn drop(&mut self) {
        self.prk.zeroize();
    }
}

impl core::fmt::Debug for AccountRoot {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("AccountRoot(..)")
    }
}

fn info(cfg_id: &[u8; 32], kind: Kind, account: &AccountId, index: u64, ctx: &[u8], tail: &[u8]) -> Vec<u8> {
    [INFO_TAG, cfg_id, &[kind as u8], account.value(), &index.to_be_bytes(), ctx, tail].concat()
}

/// `ctx` of a deposit secret.
#[must_use]
pub fn deposit_ctx(token: &EthAddress, denom: u128) -> Vec<u8> {
    [token.as_slice(), &u256(denom)].concat()
}

/// `ctx` of an exit secret.
#[must_use]
pub fn exit_ctx(token: &EthAddress, denom: u128, eth_dest: &EthAddress, max_relay_fee: u128) -> Vec<u8> {
    [token.as_slice(), &u256(denom), eth_dest, &u256(max_relay_fee)].concat()
}

/// `ctx` of a release-address key.
#[must_use]
pub fn release_ctx(token: &EthAddress, denom: u128, max_relay_fee: u128) -> Vec<u8> {
    [token.as_slice(), &u256(denom), &u256(max_relay_fee)].concat()
}

/// Domain of the claim secret `k = SHA256("RFP022/ck" | s)` under a deposit master secret `s`.
pub const CK_DOMAIN: &[u8] = b"RFP022/ck";
/// Domain of the exit secret `k' = SHA256("RFP022/ek" | s')` under an exit master secret `s'`.
pub const EK_DOMAIN: &[u8] = b"RFP022/ek";
/// Domain of the (future, Ethereum-side failsafe) reclaim nullifier `SHA256("RFP022/nfrc" | s)`.
pub const NFRC_DOMAIN: &[u8] = b"RFP022/nfrc";

/// The deposit MASTER secret `s` for the `index`-th deposit of (`token`, `denom`) into `account`.
/// It never appears in any transaction; the programs see only [`claim_secret_of`]`(s)`.
#[must_use]
pub fn deposit_master(root: &AccountRoot, cfg_id: &[u8; 32], account: &AccountId, token: &EthAddress, denom: u128, index: u64) -> [u8; 32] {
    root.derive(cfg_id, Kind::Dep, account, index, &deposit_ctx(token, denom))
}

/// `k = SHA256("RFP022/ck" | s)`: the value every program and client formula calls the deposit
/// secret (`deposit_commitment`, `claim_nullifier`, `recipient_opening`, the claim instruction's
/// `secret`). A public claim publishes `k`; `s` stays private, so `k` alone never proves
/// ownership on the future reclaim path.
#[must_use]
pub fn claim_secret_of(master: &[u8; 32]) -> [u8; 32] {
    bridge_core::sha256(&[CK_DOMAIN, master])
}

/// The claim secret `k` of the `index`-th deposit (master secret, then [`claim_secret_of`]).
#[must_use]
pub fn deposit_secret(root: &AccountRoot, cfg_id: &[u8; 32], account: &AccountId, token: &EthAddress, denom: u128, index: u64) -> [u8; 32] {
    claim_secret_of(&deposit_master(root, cfg_id, account, token, denom, index))
}

/// The reclaim nullifier `SHA256("RFP022/nfrc" | s)` of a deposit master secret, for the
/// Ethereum-side failsafe (DESIGN §10, not built: no program or vault uses it yet). A deposit
/// made with a raw `--secret k` has no master secret and will not be reclaimable that way.
#[must_use]
pub fn reclaim_nullifier(master: &[u8; 32]) -> [u8; 32] {
    bridge_core::sha256(&[NFRC_DOMAIN, master])
}

/// The exit MASTER secret `s'` for the `index`-th exit of (`token`, `denom`, `eth_dest`,
/// `max_relay_fee`) burned by `account`. Never published.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn exit_master(
    root: &AccountRoot,
    cfg_id: &[u8; 32],
    account: &AccountId,
    token: &EthAddress,
    denom: u128,
    eth_dest: &EthAddress,
    max_relay_fee: u128,
    index: u64,
) -> [u8; 32] {
    root.derive(cfg_id, Kind::Exit, account, index, &exit_ctx(token, denom, eth_dest, max_relay_fee))
}

/// `k' = SHA256("RFP022/ek" | s')`: the exit secret of `exit_commitment` and `release_nullifier`.
#[must_use]
pub fn exit_secret_of(master: &[u8; 32]) -> [u8; 32] {
    bridge_core::sha256(&[EK_DOMAIN, master])
}

/// The exit secret `k'` of the `index`-th exit ([`exit_master`], then [`exit_secret_of`]).
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn exit_secret(
    root: &AccountRoot,
    cfg_id: &[u8; 32],
    account: &AccountId,
    token: &EthAddress,
    denom: u128,
    eth_dest: &EthAddress,
    max_relay_fee: u128,
    index: u64,
) -> [u8; 32] {
    exit_secret_of(&exit_master(root, cfg_id, account, token, denom, eth_dest, max_relay_fee, index))
}

/// A seed-derived Ethereum release address and its secp256k1 secret key.
pub struct ReleaseKey {
    secret: [u8; 32],
    pub address: EthAddress,
}

impl ReleaseKey {
    /// The secret key, for exporting into an Ethereum wallet. Handle with care.
    #[must_use]
    pub const fn secret(&self) -> &[u8; 32] {
        &self.secret
    }
}

impl Drop for ReleaseKey {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

/// The release-address key for the `index`-th exit of (`token`, `denom`, `max_relay_fee`) of
/// `account` with a seed-derived destination. HKDF-Expand as for every kind; if the 32 bytes are
/// not a valid secp256k1 scalar (probability about 2^-128), `info` gets one more byte `c`
/// (1, 2, ...) before the final 0x01 until it is.
///
/// Under [`Scheme::NamesApp`] the key is the wallet's value `SHA256("RFP022/release-key" | m)`
/// instead, rehashed with SHA-256 until valid ([`release_key_from_value`]).
#[must_use]
pub fn release_key(root: &AccountRoot, cfg_id: &[u8; 32], account: &AccountId, token: &EthAddress, denom: u128, max_relay_fee: u128, index: u64) -> ReleaseKey {
    let ctx = release_ctx(token, denom, max_relay_fee);
    let mut secret = root.derive(cfg_id, Kind::Release, account, index, &ctx);
    if let Scheme::NamesApp { .. } = root.scheme {
        let value = names::tagged(names::RELEASE_TAG, &secret);
        secret.zeroize();
        return release_key_from_value(&value);
    }
    let mut counter: u8 = 0;
    loop {
        if let Some(address) = eth_address(&secret) {
            return ReleaseKey { secret, address };
        }
        counter = counter.checked_add(1).unwrap_or_else(|| unreachable!("2^-128 per try"));
        secret = hmac_sha256(&root.prk, &info(cfg_id, Kind::Release, account, index, &ctx, &[counter, 1]));
    }
}

/// The release-address key an app makes from the wallet's `RFP022/release-key` value: the value
/// itself, or `SHA256` of it until it is a valid secp256k1 scalar (about 2^-128 per try).
#[must_use]
pub fn release_key_from_value(value: &[u8; 32]) -> ReleaseKey {
    let mut secret = *value;
    loop {
        if let Some(address) = eth_address(&secret) {
            return ReleaseKey { secret, address };
        }
        secret = bridge_core::sha256(&[secret.as_slice()]);
    }
}

/// The Ethereum address of a secp256k1 secret key (`keccak256(pubkey_xy)[12..]`), or `None` if
/// the bytes are not a valid key.
#[must_use]
pub fn eth_address(secret: &[u8; 32]) -> Option<EthAddress> {
    let sk = k256::SecretKey::from_slice(secret).ok()?;
    let point = sk.public_key().to_encoded_point(false);
    let hash: [u8; 32] = Keccak256::digest(&point.as_bytes()[1..]).into();
    hash[12..].try_into().ok()
}

/// keccak256, for event topics and selectors.
#[must_use]
pub fn keccak256(bytes: &[u8]) -> [u8; 32] {
    Keccak256::digest(bytes).into()
}

/// The largest relay fee the vault can ever put on a menu for `denom`:
/// `floor(denom * MAX_FEE_BPS / 10_000)` (the vault refuses `fee * 10_000 > denom * MAX_FEE_BPS`).
/// The client's default `maxRelayFee`: every menu value, present or future, stays usable.
#[must_use]
pub const fn max_relay_fee_cap(denom: u128) -> u128 {
    (denom / 10_000) * MAX_FEE_BPS + (denom % 10_000) * MAX_FEE_BPS / 10_000
}
