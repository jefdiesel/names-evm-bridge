//! `lez-bridge`: a client for the RFP-022 token and bridge programs on LEZ 0.4.
//!
//! It deploys both programs, creates a wrapped token minted by the bridge, initialises and
//! configures the bridge, records vault roots (stand-in attestor), claims deposits and burns
//! for exit, publicly or privately, through the stock LEZ `wallet` crate (tag v0.4.0-rc1).
//!
//! Privacy defaults for a private claimer V (`claim --from V`, `claim-attested --from V`):
//! - the mint goes to a one-time address `A = V.blinded(o)`, `o = SHA256("RFP022/alias" | k)`;
//!   the opening rides in V's witness (`address_openings`) so the credit lands in V inside the
//!   proof. `--reveal-recipient` opts out and mints to V by name;
//! - if the bridge sponsors (`sponsor_amount > 0`), the claim also pays A that amount from the
//!   bridge's float in the same transaction, and V pays its fee from it: no public account ever
//!   funds V. The float is checked before proving;
//! - without sponsoring, fund V with `shield`, which credits a fresh one-time address of V, so
//!   the funding account is linked only to that address.
//!
//! PDAs the bridge seeds (config, roots, nullifiers) are mentioned without admission evidence:
//! the program's seeded effects admit them.
//!
//! Safety: the wallet's built-in default sequencer is the public testnet. This tool always
//! overrides it with `--sequencer` and refuses any address that is not loopback, except the one
//! named public demo chain, `PUBLIC_DEMO_SEQUENCER` (https only, exact host, default port).
//!
//! `attest ...` is the client for the RFP-021 attestation program and its reference consumer
//! (see `attest.rs`).

use std::path::PathBuf;

mod attest;
mod call;
mod out;
mod recover;
mod relay;

use anyhow::{Context as _, Result, anyhow, bail};
use bridge_core::{
    self as bc, AdminOp, AttestSource, Config, EthAddress, Instruction, Message as BridgeMessage, RootSource,
    config_address, config_seed,
};
use bridge_token_core::{Authority, Message as TokenMessage, TokenDefinition, TokenState};
use clap::{Parser, Subcommand};
use common::{HashType, transaction::LeeTransaction};
use eth_attest_core::{
    Anchor,
    inclusion::{AccountFields, AccountProof, AnchorWitness, StorageProof, mapping_slot},
};
use lee::{privacy_preserving_transaction::circuit::ProgramCatalog, program::Program};
use lee_core::{
    account::{AccountId, Actor, Balance},
    native_token::{self, NATIVE_TOKEN_PROGRAM_ID, decode_balance},
    program::PdaSeed,
};
use sequencer_service_rpc::SequencerClient;
use wallet::{
    AccountIdentity, AccountMention, AsyncDelivery, ExecutionFailureKind, FeeParams, WalletCore,
    config::{SequencerConnectionData, WalletConfigOverrides},
    program_facades::program_loader::ProgramLoader,
    utils::{fetch_config_path, fetch_persistent_storage_path, read_password},
};

/// `--dry-run`, for `send` (the wallet keeps its own copy private).
static DRY_RUN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// `--sequencer`, for `relay-cosign`'s submission.
static SEQUENCER: std::sync::OnceLock<String> = std::sync::OnceLock::new();
/// `--no-precheck` (hidden, tests only): skip `diagnose_claim`, so a refused claim reaches the program.
static NO_PRECHECK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `program_loader_core::MAX_SEGMENT_DATA_LEN` at v0.4.0-rc3 (unchanged since rc1).
const MAX_SEGMENT_DATA_LEN: usize = 96 * 1024;

#[derive(Parser)]
#[command(name = "lez-bridge", about = "RFP-022 bridge client for LEZ 0.4 (local chains and the public demo chain)")]
struct Cli {
    #[arg(long, env = "LEZ_BRIDGE_SEQUENCER", default_value = "http://127.0.0.1:3230")]
    sequencer: String,
    /// Simulate and report instead of sending. A private transaction stops after the wallet's
    /// simulation, before anything is proven.
    #[arg(long, global = true)]
    dry_run: bool,
    /// The most execution gas a public transaction declares, and the budget the wallet simulates
    /// it within (LEZ rc3: the wallet declares the measured cycles plus 25%, at most this; its
    /// default is 2,000,000, LEZ's cap 10,000,000). `attest init` uses the cap unless this is given.
    #[arg(long, global = true)]
    gas_limit: Option<u64>,
    /// Tests only: skip the claim commands' local pre-check, so the program's own refusal shows.
    #[arg(long, global = true, hide = true)]
    no_precheck: bool,
    /// Tests only: send a public transaction even though the wallet's simulation reverts (LEZ rc3
    /// refuses such a send by default); it is included, reverted, and pays its whole gas limit.
    #[arg(long, global = true, hide = true)]
    allow_revert: bool,
    /// Machine-readable output: one JSON document (reads) or an NDJSON event stream ending in
    /// one `result` or `error` event (sends) on stdout; every human line goes to stderr
    /// (modules/CLI-CONTRACT.md).
    #[arg(long, global = true)]
    json: bool,
    /// Derive bridge secrets as the Names Wallet's `app_secret` gives them to the module named
    /// APP (`lez_bridge` for the bridge mini-app) instead of the CLI's own v1 KDF: for
    /// `recover`, `positions`, `deposit-request`, `--key-index` claims, derived burns and
    /// `eth-key` with this wallet restored from the Names Wallet's recovery phrase.
    #[arg(long, global = true, env = "LEZ_BRIDGE_NAMES_APP", value_name = "APP")]
    names_app: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// The LEZ wallet's own command line (accounts, native transfers, pending messages).
    Wallet {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Deploy a program built into this binary (token, bridge, the RFP-021 attestation program
    /// `attest`, or its reference consumer `attest-probe`); prints its account ID.
    Deploy {
        #[arg(long, value_parser = ["token", "bridge", "attest", "attest-probe"])]
        which: String,
        #[arg(long)]
        payer: AccountId,
        #[arg(long)]
        elf: Option<PathBuf>,
        /// Leave the program replaceable by the deploy key. Local tests only: a mutable bridge or
        /// token program can be swapped for code that mints at will, so real deployments are
        /// immutable (the default).
        #[arg(long)]
        mutable: bool,
    },
    /// Create a wrapped token whose mint authority is the bridge program; prints the definition.
    CreateWrapped {
        #[arg(long)]
        token_program: AccountId,
        #[arg(long)]
        bridge: AccountId,
        #[arg(long)]
        name: String,
        #[arg(long)]
        payer: AccountId,
    },
    /// Initialise the bridge (`Initialize`): signed by the bridge's own header account (the
    /// program id; its key is in this wallet after `deploy`), paid by `--payer`. `--admin`,
    /// `--freezer` and `--attestor` get the roles. The bridge derives its cfg_id from these
    /// inputs, its root source and its own program id (see `cfg-id`, `source-id`).
    Init {
        #[arg(long)]
        bridge: AccountId,
        #[arg(long)]
        token_program: AccountId,
        #[arg(long)]
        admin: AccountId,
        #[arg(long)]
        vault: String,
        /// The EVM chain id the vault is on.
        #[arg(long)]
        chain_id: u64,
        /// This LEZ chain's Bedrock channel id (32 bytes of hex). Default: read from the chain
        /// (`chain-id`); if given, it must equal the chain's.
        #[arg(long)]
        lez_chain_id: Option<String>,
        #[arg(long)]
        freezer: AccountId,
        #[arg(long)]
        attestor: AccountId,
        /// Pays the fee (the header account holds no native LEZ); an account of this wallet.
        #[arg(long)]
        payer: AccountId,
    },
    /// Register an Ethereum token with its wrapped definition and the vault's denominations.
    Register {
        #[arg(long)]
        bridge: AccountId,
        #[arg(long)]
        admin: AccountId,
        #[arg(long)]
        eth_token: String,
        #[arg(long)]
        definition: AccountId,
        #[arg(long)]
        decimals: u8,
        /// Comma-separated, in the vault's order.
        #[arg(long, value_delimiter = ',')]
        denoms: Vec<Balance>,
    },
    /// Freeze minting (`--mint`, claims) and/or redemption (`--burn`) for one token
    /// (`--eth-token`) or everything; freezer or admin. A flag not given is left as it is.
    Freeze {
        #[arg(long)]
        bridge: AccountId,
        #[arg(long)]
        by: AccountId,
        #[arg(long)]
        eth_token: Option<String>,
        #[command(flatten)]
        what: FreezeFlags,
    },
    /// Unfreeze minting (`--mint`) and/or redemption (`--burn`) for one token (`--eth-token`) or
    /// globally; admin only. A flag not given is left as it is.
    Unfreeze {
        #[arg(long)]
        bridge: AccountId,
        #[arg(long)]
        admin: AccountId,
        #[arg(long)]
        eth_token: Option<String>,
        #[command(flatten)]
        what: FreezeFlags,
    },
    /// Stand-in attestation: record a vault deposit root (`--bridge --attestor --root`). With a
    /// subcommand instead: the RFP-021 Ethereum attestation program (`attest init|update|show|
    /// prove-slot|id`).
    #[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
    Attest {
        #[command(subcommand)]
        rfp021: Option<attest::AttestCommand>,
        #[arg(long, required = true)]
        bridge: Option<AccountId>,
        #[arg(long, required = true)]
        attestor: Option<AccountId>,
        #[arg(long, required = true)]
        root: Option<String>,
    },
    /// The cfg_id the bridge derives at init and the vault must hold (computed here, nothing
    /// sent). Prints `CFG_ID=0x..` and `PROGRAM_ID=0x..`.
    CfgId {
        #[arg(long)]
        vault: String,
        /// The EVM chain id the vault is on.
        #[arg(long)]
        chain_id: u64,
        #[arg(long)]
        lez_chain_id: String,
        #[arg(long)]
        zone_id: String,
        /// The bridge program's account id.
        #[arg(long)]
        bridge: AccountId,
    },
    /// The root-source id the bridge derives at init, the vault's `zoneId` (nothing sent):
    /// the stand-in without arguments, or `--attest-program --instance [--deposit-root-slot]`.
    /// Prints `ZONE_ID=0x..`.
    SourceId {
        #[arg(long, requires = "instance")]
        attest_program: Option<AccountId>,
        #[arg(long, requires = "attest_program")]
        instance: Option<String>,
        #[arg(long, default_value_t = bc::VAULT_DEPOSIT_ROOT_SLOT)]
        deposit_root_slot: u64,
    },
    /// This chain's Bedrock channel id, from its `sequencer_stake` config (the vault's
    /// `lezChainId`). Prints `LEZ_CHAIN_ID=0x..`.
    ChainId,
    /// Tests only: send `--message-hex` as a root call to `--signer`'s own actor of `--program`,
    /// signed by `--signer` (an account of this wallet), touching the `--also ACCOUNT@PROGRAM`
    /// actors (never signed). For negative tests of the programs (e.g. the header key's mint).
    #[command(hide = true)]
    RawCall {
        #[arg(long)]
        program: AccountId,
        #[arg(long)]
        signer: AccountId,
        #[arg(long)]
        message_hex: String,
        #[arg(long, value_delimiter = ',')]
        also: Vec<String>,
        #[arg(long)]
        payer: Option<AccountId>,
    },
    /// A program's account id as 0x-hex bytes32, the vault's `PROGRAM_ID` (nothing sent).
    ProgramId {
        #[arg(long)]
        bridge: AccountId,
    },
    /// The deposit commitment C for a vault deposit (computed here, nothing sent).
    Commitment {
        #[command(flatten)]
        secret: RawSecret,
        #[arg(long)]
        dest: AccountId,
        #[arg(long)]
        eth_token: String,
        #[arg(long)]
        denom: Balance,
        #[arg(long)]
        cfg_id: String,
    },
    /// The exit commitment E for a burn (computed here, nothing sent).
    ExitCommitment {
        #[command(flatten)]
        secret: RawSecret,
        #[arg(long)]
        eth_dest: String,
        #[arg(long)]
        eth_token: String,
        #[arg(long)]
        denom: Balance,
        #[arg(long)]
        max_relay_fee: Balance,
        #[arg(long)]
        cfg_id: String,
    },
    /// Claim a vault deposit into `--from` (a private account of this wallet, or a public one).
    /// `--key-index N` derives the deposit secret from `--from`'s key (see `deposit-request`);
    /// `--secret` takes a raw secret (older scripts).
    Claim {
        #[arg(long)]
        bridge: AccountId,
        #[arg(long)]
        token_program: AccountId,
        #[arg(long)]
        from: AccountId,
        #[arg(long)]
        eth_token: String,
        #[arg(long)]
        denom_index: u32,
        #[arg(long)]
        denom: Balance,
        #[command(flatten)]
        source: SecretSource,
        #[command(flatten)]
        chain: recover::ChainArgs,
        /// Default: the bridge config's.
        #[arg(long)]
        cfg_id: Option<String>,
        /// The vault's deposit leaves in order, one hex leaf per line (default: rebuilt from the
        /// vault's `Deposit` logs up to Ethereum's finalized block, through `--eth-rpc`).
        #[arg(long)]
        leaves: Option<PathBuf>,
        /// The deposit's leaf index (default: the leaf that matches this deposit).
        #[arg(long)]
        index: Option<usize>,
        #[command(flatten)]
        privacy: ClaimPrivacy,
        #[command(flatten)]
        emit: EmitCall,
        /// A relayed public claim: `--from` (a public account of this wallet, any balance, also
        /// never used) signs the claim and the relay co-signs as its fee payer, so the claimer
        /// needs no native LEZ. With no value (or `auto`): the relay behind `--sequencer`'s own
        /// endpoint (the demo chain serves it as JSON-RPC methods); or a loopback
        /// `http://…/relay`. With `--dry-run` the relay only checks it.
        #[arg(long, value_name = "URL", num_args = 0..=1, default_missing_value = "auto", conflicts_with = "emit_call")]
        relay: Option<String>,
    },
    /// Operator side of `claim --relay` (run by the relayer service, not by hand): check a
    /// half-signed public claim from `--tx-file` (hex), and with `--submit` co-sign it as
    /// `--payer` (a key in this wallet) and send it. Prints one `RELAY {json}` line.
    #[command(hide = true)]
    RelayCosign {
        #[arg(long)]
        tx_file: PathBuf,
        #[arg(long)]
        bridge: AccountId,
        #[arg(long)]
        token_program: AccountId,
        #[arg(long)]
        payer: AccountId,
        #[arg(long)]
        max_gas: u64,
        #[arg(long)]
        max_fee: Balance,
        #[arg(long)]
        submit: bool,
        /// `burn`: a relayed public burn to this bridge (`BurnWithNotice`), not a claim.
        #[arg(long, default_value = "claim", value_parser = ["claim", "burn"])]
        kind: String,
    },
    /// Initialise the bridge with roots from an RFP-021 attestation instance
    /// (`InitializeAttested`): signed by the bridge's header account, as `init`.
    InitAttested {
        #[arg(long)]
        bridge: AccountId,
        #[arg(long)]
        token_program: AccountId,
        #[arg(long)]
        admin: AccountId,
        #[arg(long)]
        vault: String,
        #[arg(long)]
        chain_id: u64,
        /// As for `init`.
        #[arg(long)]
        lez_chain_id: Option<String>,
        #[arg(long)]
        freezer: AccountId,
        /// Pays the fee; an account of this wallet.
        #[arg(long)]
        payer: AccountId,
        /// The deployed RFP-021 attestation program.
        #[arg(long)]
        attest_program: AccountId,
        /// The attestation instance id (32 bytes of hex); its chain must be `--chain-id`.
        #[arg(long)]
        instance: String,
        /// Storage slot index of `isDepositRoot` in the vault.
        #[arg(long, default_value_t = bc::VAULT_DEPOSIT_ROOT_SLOT)]
        deposit_root_slot: u64,
    },
    /// Claim through RFP-021: prove the vault's `isDepositRoot[root]` under a finalized anchor.
    /// The root is the one the leaves give (the vault's current root).
    ClaimAttested {
        #[arg(long)]
        bridge: AccountId,
        #[arg(long)]
        token_program: AccountId,
        #[arg(long)]
        from: AccountId,
        #[arg(long)]
        eth_token: String,
        #[arg(long)]
        denom_index: u32,
        #[arg(long)]
        denom: Balance,
        #[command(flatten)]
        source: SecretSource,
        #[command(flatten)]
        chain: recover::ChainArgs,
        #[arg(long)]
        cfg_id: Option<String>,
        #[arg(long)]
        leaves: Option<PathBuf>,
        #[arg(long)]
        index: Option<usize>,
        /// `eth_getProof(vault, [keccak256(abi.encode(root, N))], block)` as JSON (the bare result
        /// or the JSON-RPC response).
        #[arg(long)]
        proof: PathBuf,
        /// The anchor is this execution state root (32 bytes of hex)...
        #[arg(long, conflicts_with = "anchor_block_hash", required_unless_present = "anchor_block_hash")]
        anchor_state_root: Option<String>,
        /// ...or this execution block hash, with `--header-rlp`.
        #[arg(long, requires = "header_rlp")]
        anchor_block_hash: Option<String>,
        /// The block's RLP header, as hex text or raw bytes.
        #[arg(long)]
        header_rlp: Option<PathBuf>,
        #[command(flatten)]
        privacy: ClaimPrivacy,
        #[command(flatten)]
        emit: EmitCall,
    },
    /// Set the native LEZ the bridge pays from its float to each one-time claim recipient
    /// (0 turns sponsoring off); admin only.
    SetSponsor {
        #[arg(long)]
        bridge: AccountId,
        #[arg(long)]
        admin: AccountId,
        #[arg(long)]
        amount: Balance,
    },
    /// Top up the bridge's float: a native transfer from `--from` (public) to the config account.
    /// Anyone may.
    FundFloat {
        #[arg(long)]
        bridge: AccountId,
        #[arg(long)]
        from: AccountId,
        #[arg(long)]
        amount: Balance,
    },
    /// Fund a private account from a public one for fees when the bridge does not sponsor: the
    /// credit goes to a fresh one-time address of `--to` (random opening, kept only in the
    /// proof), so `--from` is publicly linked to that address alone, not to `--to`.
    Shield {
        #[arg(long)]
        from: AccountId,
        #[arg(long)]
        to: AccountId,
        #[arg(long)]
        amount: Balance,
    },
    /// Burn one denomination for exit. Without `--exit`, the exit secret, the release address
    /// (seed-derived unless `--eth-dest`) and `E` are derived from `--from`'s key at the next
    /// free index, after scanning both chains; `--exit E` takes a commitment made elsewhere.
    Burn {
        #[arg(long)]
        bridge: AccountId,
        #[arg(long)]
        token_program: AccountId,
        #[arg(long)]
        from: AccountId,
        #[arg(long)]
        definition: AccountId,
        #[arg(long)]
        amount: Balance,
        #[arg(long)]
        exit: Option<String>,
        /// Release to this Ethereum address instead of a seed-derived one (recovery will then
        /// need it named again: `recover --eth-dest`).
        #[arg(long, conflicts_with = "exit")]
        eth_dest: Option<String>,
        /// Default: the 2% cap, which admits every relay fee a menu can hold.
        #[arg(long, conflicts_with = "exit")]
        max_relay_fee: Option<Balance>,
        /// Use this index instead of the next free one.
        #[arg(long, conflicts_with = "exit")]
        key_index: Option<u64>,
        #[command(flatten)]
        chain: recover::ChainArgs,
        #[command(flatten)]
        emit: EmitCall,
        /// A relayed public burn ("Send back to Ethereum"): `--from` (a public account of this
        /// wallet, no native LEZ needed) signs and the bridge's relay co-signs as fee payer
        /// (`bridge_relayBurn`). No value (or `auto`): the relay behind `--sequencer`; or a loopback
        /// `http://…/relay`. With `--dry-run` the relay only checks it (check_only).
        #[arg(long, value_name = "URL", num_args = 0..=1, default_missing_value = "auto", conflicts_with = "emit_call")]
        relay: Option<String>,
    },
    /// The next deposit commitment of `--from` (derived from its key) and a deposit request for
    /// the Ethereum side (format v1 of web/deposit). Scans the vault first; prints no secret.
    DepositRequest(recover::DepositRequestArgs),
    /// Rediscover every deposit and exit of this wallet's accounts from their keys and the two
    /// chains alone (no local state needed), and say which are claimable or unreleased.
    Recover(recover::RecoverArgs),
    /// The last `recover` result, from the local cache (no network).
    Positions {
        #[arg(long)]
        bridge: AccountId,
    },
    /// Write a seed-derived release address's private key to a file (mode 600).
    EthKey(recover::EthKeyArgs),
    /// Write a seed-derived exit's secret (the release proof's witness) to a file (mode 600) and
    /// print its public exit commitment, to compare with the burn's.
    ExitSecret(recover::ExitSecretArgs),
    /// Every wrapped token this wallet holds.
    Balances {
        #[arg(long)]
        token_program: AccountId,
    },
    /// The bridge config: tokens, counters, roles.
    Show {
        #[arg(long)]
        bridge: AccountId,
    },
    /// A wrapped token's definition (name, supply, authority).
    Definition {
        #[arg(long)]
        token_program: AccountId,
        #[arg(long)]
        definition: AccountId,
    },
}

impl Command {
    /// A send (or the plan of one): its `--json` output is the NDJSON event stream of
    /// CLI-CONTRACT.md §3.
    const fn is_job(&self) -> bool {
        matches!(
            self,
            Self::Deploy { .. }
                | Self::CreateWrapped { .. }
                | Self::Init { .. }
                | Self::InitAttested { .. }
                | Self::RawCall { .. }
                | Self::Register { .. }
                | Self::Freeze { .. }
                | Self::Unfreeze { .. }
                | Self::Attest { rfp021: None, .. }
                | Self::Claim { .. }
                | Self::ClaimAttested { .. }
                | Self::SetSponsor { .. }
                | Self::FundFloat { .. }
                | Self::Shield { .. }
                | Self::Burn { .. }
        )
    }
}

/// `wallet account list` with `--json`: the wallet's accounts from its key chain (no chain
/// read, nothing written).
fn account_list(wallet: &WalletCore<SequencerClient>) -> Result<()> {
    let kc = wallet.storage().key_chain();
    let mut list = Vec::new();
    for (id, index) in kc.public_account_ids() {
        list.push(serde_json::json!({ "account": id.to_string(), "kind": "public", "path": index.map(|i| format!("{i}")), "label": null }));
    }
    for (id, index) in kc.private_account_ids() {
        list.push(serde_json::json!({ "account": id.to_string(), "kind": "private", "path": index.map(|i| format!("{i}")), "label": null }));
    }
    out::doc(&serde_json::json!({ "accounts": list }));
    Ok(())
}

/// Where a claim's deposit secret comes from.
#[derive(clap::Args)]
#[group(required = true, multiple = false)]
struct SecretSource {
    /// Derive it: the `N`th deposit of this token and denomination into `--from`.
    #[arg(long)]
    key_index: Option<u64>,
    /// A raw secret (older scripts; deposits made this way have no master secret and will not be
    /// reclaimable through the future Ethereum-side failsafe). Visible to `ps`: prefer
    /// `--secret-file`.
    #[arg(long)]
    secret: Option<String>,
    /// The raw secret from a file only its owner can read (64 hex characters; `-` reads
    /// standard input). Never on the command line.
    #[arg(long)]
    secret_file: Option<PathBuf>,
}

/// A raw secret for the pure computations (`commitment`, `exit-commitment`).
#[derive(clap::Args)]
#[group(required = true, multiple = false)]
struct RawSecret {
    /// On the command line (visible to `ps`; tests and old scripts).
    #[arg(long)]
    secret: Option<String>,
    /// From a file only its owner can read (64 hex characters; `-` reads standard input).
    #[arg(long)]
    secret_file: Option<PathBuf>,
}

impl RawSecret {
    fn get(&self) -> Result<[u8; 32]> {
        match (&self.secret, &self.secret_file) {
            (Some(s), None) => hex32(s),
            (None, Some(p)) => call::read_secret(p),
            _ => bail!("give --secret or --secret-file"),
        }
    }
}

/// Build the Names Wallet private call instead of sending (claims and burns).
#[derive(clap::Args)]
struct EmitCall {
    /// Don't send: write the `propose_private_call` JSON for `--from`, a private account of the
    /// Names Wallet, to this NEW file (mode 600: a claim's message carries its secret), after
    /// the same local checks a send makes. The wallet simulates, shows and proves it.
    #[arg(long)]
    emit_call: Option<PathBuf>,
    /// With `--emit-call`: the most the fee may reach (`declared.max_fee`). Default: the sponsor
    /// amount for a sponsored claim, else 20000000.
    #[arg(long, requires = "emit_call")]
    max_fee: Option<Balance>,
}

/// How a claim names its recipient.
#[derive(clap::Args)]
struct ClaimPrivacy {
    /// Mint to `--from` by name instead of a one-time address (and so take no sponsorship).
    /// Only meaningful for a private `--from`; a public claimer always mints to itself.
    #[arg(long)]
    reveal_recipient: bool,
}

/// Which side of the bridge a freeze or unfreeze names: at least one, both allowed.
#[derive(clap::Args)]
#[group(required = true, multiple = true)]
struct FreezeFlags {
    /// Minting: claims of deposits (and, globally, stand-in root attestation).
    #[arg(long)]
    mint: bool,
    /// Redemption: burns for exit.
    #[arg(long)]
    burn: bool,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    out::init(cli.json);
    if let Some(app) = &cli.names_app
        && let Err(e) = recover::set_names_app(app)
    {
        out::fail(&e);
    }
    match real_main(cli).await {
        Ok(()) => {
            if out::in_job() {
                out::finish();
            }
        }
        Err(e) => out::fail(&e),
    }
}

async fn real_main(cli: Cli) -> Result<()> {
    let mut overrides = sequencer_overrides(&cli.sequencer)?;
    let is_init = matches!(&cli.command, Command::Attest { rfp021: Some(attest::AttestCommand::Init { .. }), .. });
    overrides.gas_limit = cli.gas_limit.or(is_init.then_some(attest::INIT_GAS_LIMIT));
    match cli.command {
        Command::Wallet { args } if out::json() && args.iter().map(String::as_str).eq(["account", "list"]) => {
            account_list(&open_wallet(overrides)?)
        }
        Command::Wallet { args } => run_wallet(args, overrides).await,
        Command::CfgId { vault, chain_id, lez_chain_id, zone_id, bridge } => {
            let id = bc::cfg_id(chain_id, &eth(&vault)?, &hex32(&lez_chain_id)?, &hex32(&zone_id)?, &bridge);
            println!("CFG_ID=0x{}", to_hex(&id));
            println!("PROGRAM_ID=0x{}", to_hex(bridge.value()));
            out::doc(&serde_json::json!({ "cfg_id": format!("0x{}", to_hex(&id)), "program_id": format!("0x{}", to_hex(bridge.value())) }));
            Ok(())
        }
        Command::SourceId { attest_program, instance, deposit_root_slot } => {
            let source = match (attest_program, instance) {
                (Some(attest_program), Some(instance)) => {
                    RootSource::Attest(AttestSource { attest_program, attest_instance: hex32(&instance)?, deposit_root_slot })
                }
                _ => RootSource::StandIn,
            };
            let id = bc::source_id(&source);
            println!("ZONE_ID=0x{}", to_hex(&id));
            out::doc(&serde_json::json!({ "zone_id": format!("0x{}", to_hex(&id)) }));
            Ok(())
        }
        Command::ProgramId { bridge } => {
            println!("PROGRAM_ID=0x{}", to_hex(bridge.value()));
            println!("CONFIG={}", config_address(&bridge));
            out::doc(&serde_json::json!({ "program_id": format!("0x{}", to_hex(bridge.value())) }));
            Ok(())
        }
        Command::Commitment { secret, dest, eth_token, denom, cfg_id } => {
            let c = bc::deposit_commitment(&secret.get()?, &dest, &eth(&eth_token)?, denom, &hex32(&cfg_id)?);
            println!("0x{}", to_hex(&c));
            out::doc(&serde_json::json!({ "commitment": format!("0x{}", to_hex(&c)) }));
            Ok(())
        }
        Command::ExitCommitment { secret, eth_dest, eth_token, denom, max_relay_fee, cfg_id } => {
            let k = secret.get()?;
            let e = bc::exit_commitment(&k, &eth(&eth_dest)?, &eth(&eth_token)?, denom, max_relay_fee, &hex32(&cfg_id)?);
            let nf = bc::release_nullifier(&k);
            println!("0x{}", to_hex(&e));
            println!("release nullifier 0x{}", to_hex(&nf));
            out::doc(&serde_json::json!({ "exit_commitment": format!("0x{}", to_hex(&e)), "release_nullifier": format!("0x{}", to_hex(&nf)) }));
            Ok(())
        }
        command => {
            if command.is_job() {
                out::job_start();
            }
            let mut wallet = open_wallet(overrides)?;
            wallet.set_dry_run(cli.dry_run);
            wallet.set_allow_revert(cli.allow_revert);
            DRY_RUN.store(cli.dry_run, std::sync::atomic::Ordering::Relaxed);
            let _ = SEQUENCER.set(allowed_sequencer(&cli.sequencer)?.to_string());
            NO_PRECHECK.store(cli.no_precheck, std::sync::atomic::Ordering::Relaxed);
            if let Command::Attest { rfp021: Some(command), .. } = command {
                return attest::run(&mut wallet, &cli.sequencer, command).await;
            }
            run(&mut wallet, command).await
        }
    }
}

/// The public demo chain (`public-demo/PLAN.md` (not published)): a LEZ 0.4 sequencer run by the bridge operator
/// behind an RPC filter, until the official testnet runs LEZ 0.4. The only non-loopback sequencer
/// this client talks to.
const PUBLIC_DEMO_SEQUENCER: &str = "https://lez.nameswallet.app/";

/// `--sequencer` as the wallet's override, if it is allowed (see [`allowed_sequencer`]).
fn sequencer_overrides(sequencer: &str) -> Result<WalletConfigOverrides> {
    let url = allowed_sequencer(sequencer)?;
    Ok(WalletConfigOverrides {
        sequencer: Some(SequencerConnectionData { sequencer_addr: url, basic_auth: None }),
        ..WalletConfigOverrides::default()
    })
}

/// A loopback sequencer (any port), or exactly [`PUBLIC_DEMO_SEQUENCER`]: https, that host, the
/// default port, no credentials, path, query or fragment. Anything else is refused, so a typo or
/// a look-alike host never sends a transaction (or a public claim's secret) somewhere else.
fn allowed_sequencer(sequencer: &str) -> Result<url::Url> {
    let url: url::Url = sequencer.parse().context("--sequencer is not a URL")?;
    let host = url.host_str().unwrap_or_default();
    if matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1") {
        return Ok(url);
    }
    let demo: url::Url = PUBLIC_DEMO_SEQUENCER.parse().expect("a valid URL");
    let exact = url.scheme() == "https"
        && url.host_str() == demo.host_str()
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none();
    if !exact {
        bail!(
            "{sequencer} is neither loopback nor the public demo chain {PUBLIC_DEMO_SEQUENCER}; \
             this client talks to no other sequencer"
        );
    }
    Ok(url)
}

fn open_wallet(overrides: WalletConfigOverrides) -> Result<WalletCore<SequencerClient>> {
    let storage_path = fetch_persistent_storage_path()?;
    if !storage_path.exists() {
        bail!("no wallet yet: run `lez-bridge wallet check-health` first to create one");
    }
    let password = read_password(&storage_path)?;
    WalletCore::new_update_chain(fetch_config_path()?, storage_path, Some(overrides), &password)
}

async fn run_wallet(args: Vec<String>, overrides: WalletConfigOverrides) -> Result<()> {
    use clap::Parser as _;
    use wallet::cli::{Args, execute_subcommand};

    let Args { command, dry_run, allow_revert, gas_limit, max_fee, tip, fee_headroom, fee_payer, refund_to, .. } =
        Args::parse_from(std::iter::once("lez-bridge wallet".to_owned()).chain(args));
    let config_path = fetch_config_path()?;
    let storage_path = fetch_persistent_storage_path()?;
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let command = command.ok_or_else(|| anyhow!("give a wallet command"))?;
    let mut wallet = if storage_path.exists() {
        let password = read_password(&storage_path)?;
        WalletCore::new_update_chain(config_path, storage_path, Some(overrides), &password)?
    } else {
        let password = std::env::var(wallet::PASSWORD_ENV_VAR).unwrap_or_default();
        let (mut wallet, mnemonic) =
            WalletCore::new_init_storage(config_path, storage_path, Some(overrides), &password)?;
        // Tests only (scripts/e2e-recovery.sh restores a second wallet from this phrase): write
        // the new wallet's recovery phrase to a NEW file, mode 600. Never printed.
        if let Some(path) = std::env::var_os("LEZ_BRIDGE_TEST_MNEMONIC_FILE") {
            use std::{io::Write as _, os::unix::fs::OpenOptionsExt as _};
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
            writeln!(f, "{mnemonic}")?;
        }
        wallet.start_at_chain_tip().await.ok();
        wallet.store_persistent_data()?;
        println!("created a new wallet (recovery phrase not shown)");
        wallet
    };
    wallet.set_dry_run(dry_run);
    wallet.set_allow_revert(allow_revert);
    // LEZ rc3: one set of fee arguments for every send (the wallet's own `--gas-limit`, `--max-fee`,
    // `--tip`, `--fee-headroom`, `--fee-payer`, `--refund-to`), as LEZ's wallet binary passes them.
    wallet.set_fee_params(FeeParams {
        gas_limit,
        max_fee,
        tip,
        headroom_blocks: fee_headroom,
        payer: fee_payer,
        refund_to,
    });
    match execute_subcommand(&mut wallet, command).await {
        // A dry run stops the command with its report (rc3 returns it instead of printing it).
        Err(error) => match error.root_cause().downcast_ref() {
            Some(ExecutionFailureKind::DryRun(report)) => {
                println!("{report}");
                Ok(())
            }
            _ => Err(error),
        },
        Ok(_) => Ok(()),
    }
}

async fn run(wallet: &mut WalletCore<SequencerClient>, command: Command) -> Result<()> {
    match command {
        Command::Wallet { .. }
        | Command::CfgId { .. }
        | Command::ProgramId { .. }
        | Command::SourceId { .. }
        | Command::Commitment { .. }
        | Command::ExitCommitment { .. }
        | Command::Attest { rfp021: Some(_), .. } => unreachable!(),
        Command::Deploy { which, payer, elf, mutable } => {
            let bytecode = match (elf, which.as_str()) {
                (Some(path), _) => std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?,
                (None, "token") => bridge_methods::TOKEN_GUEST_ELF.to_vec(),
                (None, "attest") => attest_methods::ATTEST_GUEST_ELF.to_vec(),
                (None, "attest-probe") => attest_methods::ATTEST_PROBE_GUEST_ELF.to_vec(),
                (None, _) => bridge_methods::BRIDGE_GUEST_ELF.to_vec(),
            };
            let program = Program::new(bytecode.clone().into()).map_err(|e| anyhow!("bad ELF: {e}"))?;
            println!("image id {}", words_hex(&program.id()));
            let header = wallet.create_new_account_public(None).0;
            // The loader uploads only the user ELF (lez/wallet/src/program_facades/program_loader.rs).
            let user_elf_len = risc0_binfmt::ProgramBinary::decode(&bytecode)
                .map_err(|e| anyhow!("not a RISC Zero program binary: {e}"))?
                .user_elf
                .len();
            let segments: Vec<AccountId> = std::iter::repeat_with(|| wallet.create_new_account_public(None).0)
                .take(user_elf_len.div_ceil(MAX_SEGMENT_DATA_LEN))
                .collect();
            wallet.store_persistent_data()?;
            println!("deploying {} bytes in {} segment(s) at header {header}", bytecode.len(), segments.len());
            ProgramLoader(wallet).deploy(header, &segments, bytecode, !mutable, Some(payer)).await?;
            println!("{}", if mutable { "MUTABLE: the deploy key can replace this program" } else { "immutable" });
            let deployed = ProgramLoader(wallet)
                .program(header)
                .await?
                .ok_or_else(|| anyhow!("{header} holds no program after the deploy"))?;
            if deployed.id() != program.id() {
                bail!("{header} holds image {} instead", words_hex(&deployed.id()));
            }
            println!("program {header}");
            Ok(())
        }
        Command::CreateWrapped { token_program, bridge, name, payer } => {
            let definition = wallet.create_new_account_public(None).0;
            wallet.store_persistent_data()?;
            println!("definition account {definition}");
            let message = TokenMessage::NewFungibleDefinition {
                name,
                total_supply: 0,
                // The bridge program's code, never a signature by the account with its id.
                mint_authority: Some(Authority::Program(bridge)),
                supply_to: payer,
            };
            let accounts = vec![AccountIdentity::Public(definition).select_program_actor_state(token_program)];
            let hash = wallet
                .send_pub_tx_paid_by(accounts, 0, borsh::to_vec(&message)?, Some(payer))
                .await?;
            finish(wallet, hash).await?;
            println!("token {definition}");
            Ok(())
        }
        Command::ChainId => {
            let id = chain_channel_id(wallet).await?;
            println!("LEZ_CHAIN_ID=0x{}", to_hex(&id));
            out::doc(&serde_json::json!({ "lez_chain_id": format!("0x{}", to_hex(&id)) }));
            Ok(())
        }
        Command::RawCall { program, signer: who, message_hex, also, payer } => {
            let message = hex_bytes(&message_hex)?;
            let mut accounts = vec![signer(wallet, who, program)];
            for a in &also {
                let (account, of) = a.split_once('@').ok_or_else(|| anyhow!("--also takes ACCOUNT@PROGRAM"))?;
                let (account, of): (AccountId, AccountId) =
                    (account.parse().map_err(|_| anyhow!("bad account {account}"))?, of.parse().map_err(|_| anyhow!("bad program {of}"))?);
                accounts.push(AccountIdentity::PublicNoSign(account).select_program_actor_state(of));
            }
            let hash = wallet.send_pub_tx_paid_by(accounts, 0, message, payer).await?;
            finish(wallet, hash).await
        }
        Command::Init { bridge, token_program, admin, vault, chain_id, lez_chain_id, freezer, attestor, payer } => {
            let (vault, lez_chain_id) = (eth(&vault)?, init_chain_id(wallet, lez_chain_id.as_deref()).await?);
            let zone = bc::source_id(&RootSource::StandIn);
            let id = bc::cfg_id(chain_id, &vault, &lez_chain_id, &zone, &bridge);
            println!("lez chain id 0x{}\nzone id (root source) 0x{}", to_hex(&lez_chain_id), to_hex(&zone));
            println!("cfg_id the bridge will derive 0x{}", to_hex(&id));
            header_signs(wallet, bridge)?;
            let message = BridgeMessage::Instruction(Instruction::Initialize {
                admin,
                vault,
                evm_chain_id: chain_id,
                lez_chain_id,
                token_program,
                freezer,
                attestor,
            });
            let accounts = vec![signer(wallet, bridge, bridge), pda(bridge, config_seed())];
            let hash = wallet.send_pub_tx_paid_by(accounts, 0, borsh::to_vec(&message)?, Some(payer)).await?;
            finish(wallet, hash).await
        }
        Command::InitAttested {
            bridge,
            token_program,
            admin,
            vault,
            chain_id,
            lez_chain_id,
            freezer,
            payer,
            attest_program,
            instance,
            deposit_root_slot,
        } => {
            let (vault, lez_chain_id, instance) = (eth(&vault)?, init_chain_id(wallet, lez_chain_id.as_deref()).await?, hex32(&instance)?);
            let source = AttestSource { attest_program, attest_instance: instance, deposit_root_slot };
            let zone = bc::source_id(&RootSource::Attest(source));
            let id = bc::cfg_id(chain_id, &vault, &lez_chain_id, &zone, &bridge);
            println!("lez chain id 0x{}\nzone id (root source) 0x{}", to_hex(&lez_chain_id), to_hex(&zone));
            println!("cfg_id the bridge will derive 0x{}", to_hex(&id));
            header_signs(wallet, bridge)?;
            let message = BridgeMessage::Instruction(Instruction::InitializeAttested {
                admin,
                vault,
                evm_chain_id: chain_id,
                lez_chain_id,
                token_program,
                freezer,
                source,
            });
            let accounts = vec![
                signer(wallet, bridge, bridge),
                pda(bridge, config_seed()),
                AccountIdentity::PublicNoSign(eth_attest_core::config_address(&attest_program, &instance))
                    .select_program_actor_state(attest_program),
            ];
            let hash = wallet.send_pub_tx_paid_by(accounts, 0, borsh::to_vec(&message)?, Some(payer)).await?;
            finish(wallet, hash).await
        }
        Command::Register { bridge, admin, eth_token, definition, decimals, denoms } => {
            // The bridge can't read a definition; a definition minted by anything but the bridge
            // program's code (an account authority, even the account with the bridge's id) would
            // let a key mint unbacked tokens (security review 2026-11-14, C1).
            let config: Config = borsh::from_slice(&public_state(wallet, config_address(&bridge), bridge).await?)
                .map_err(|_| anyhow!("{bridge} has no bridge config"))?;
            match TokenState::decode(&public_state(wallet, definition, config.token_program).await?) {
                Some(TokenState::Definition(TokenDefinition::Fungible { authority: Some(Authority::Program(p)), .. })) if p == bridge => {}
                Some(TokenState::Definition(TokenDefinition::Fungible { authority, .. })) => {
                    bail!("{definition}'s mint authority is {authority:?}, not the bridge program {bridge}: refusing to register it")
                }
                _ => bail!("{definition} is not a token definition of the bridge's token program"),
            }
            let message = BridgeMessage::Instruction(Instruction::Admin(AdminOp::RegisterToken {
                eth_token: eth(&eth_token)?,
                definition,
                decimals,
                denoms,
            }));
            let accounts = vec![signer(wallet, admin, bridge), pda(bridge, config_seed())];
            send(wallet, &[bridge], accounts, &borsh::to_vec(&message)?, None).await
        }
        Command::Freeze { bridge, by, eth_token, what: FreezeFlags { mint, burn } } => {
            let eth_token = eth_token.map(|t| eth(&t)).transpose()?;
            let message = BridgeMessage::Instruction(Instruction::Admin(AdminOp::Freeze { eth_token, mint, burn }));
            let accounts = vec![signer(wallet, by, bridge), pda(bridge, config_seed())];
            send(wallet, &[bridge], accounts, &borsh::to_vec(&message)?, None).await
        }
        Command::Unfreeze { bridge, admin, eth_token, what: FreezeFlags { mint, burn } } => {
            let eth_token = eth_token.map(|t| eth(&t)).transpose()?;
            let message = BridgeMessage::Instruction(Instruction::Admin(AdminOp::Unfreeze { eth_token, mint, burn }));
            let accounts = vec![signer(wallet, admin, bridge), pda(bridge, config_seed())];
            send(wallet, &[bridge], accounts, &borsh::to_vec(&message)?, None).await
        }
        Command::Attest { rfp021: None, bridge, attestor, root } => {
            let (Some(bridge), Some(attestor), Some(root)) = (bridge, attestor, root) else {
                bail!("give --bridge, --attestor and --root (or an RFP-021 subcommand)");
            };
            let root = hex32(&root)?;
            let message = BridgeMessage::Instruction(Instruction::AttestRoot { root });
            let accounts = vec![signer(wallet, attestor, bridge), pda(bridge, config_seed()), pda(bridge, bc::root_seed(&root))];
            send(wallet, &[bridge], accounts, &borsh::to_vec(&message)?, None).await
        }
        Command::SetSponsor { bridge, admin, amount } => {
            let message = BridgeMessage::Instruction(Instruction::Admin(AdminOp::SetSponsor { amount }));
            let accounts = vec![signer(wallet, admin, bridge), pda(bridge, config_seed())];
            send(wallet, &[bridge], accounts, &borsh::to_vec(&message)?, None).await
        }
        Command::FundFloat { bridge, from, amount } => {
            let config = config_address(&bridge);
            if public_state(wallet, config, bridge).await?.is_empty() {
                bail!("{bridge} has no bridge config");
            }
            if identity(wallet, from).is_private() {
                bail!("fund the float from a public account");
            }
            println!("float before {}", float(wallet, &bridge).await?);
            let message = native_token::Message::Transfer { to: config, amount };
            let accounts = vec![
                AccountIdentity::Public(from).balance(),
                AccountIdentity::PublicNoSign(config).balance(),
            ];
            send(wallet, &[], accounts, &borsh::to_vec(&message)?, None).await?;
            println!("float {}", float(wallet, &bridge).await?);
            Ok(())
        }
        Command::Shield { from, to, amount } => {
            let receiver = identity(wallet, to);
            if !receiver.is_private() {
                bail!("{to} is not a private account of this wallet");
            }
            if identity(wallet, from).is_private() {
                bail!("shield from a public account (a private sender can transfer privately instead)");
            }
            let opening = random32()?;
            let alias = to.blinded(&opening);
            println!("crediting {amount} to the one-time address {alias} of {to} (the opening stays here)");
            let message = native_token::Message::Transfer { to: alias, amount };
            let mut receiving = receiver.select_program_actor_state(NATIVE_TOKEN_PROGRAM_ID).receiving();
            receiving.address_openings.insert(opening);
            let accounts = vec![AccountIdentity::Public(from).balance(), receiving];
            send(wallet, &[], accounts, &borsh::to_vec(&message)?, Some(to)).await
        }
        Command::Claim { bridge, token_program, from, eth_token, denom_index, denom, source, chain, cfg_id, leaves, index, privacy, emit, relay } => {
            let sequencer = SEQUENCER.get().cloned().unwrap_or_default();
            let relay = relay.map(|r| relay::allowed_relay(&r, &sequencer)).transpose()?;
            if relay.is_some() && !matches!(identity(wallet, from), AccountIdentity::Public(_)) {
                bail!("--relay takes a public account of this wallet as --from (a private claim is sponsored by the float instead)");
            }
            let (secret, leaves, index) = claim_inputs(wallet, bridge, from, &eth_token, denom, source, &chain, leaves, index).await?;
            let plan = plan_claim(wallet, bridge, from, &eth_token, denom, secret, cfg_id.as_deref(), leaves, index, &privacy, emit.emit_call.is_some()).await?;
            let index = plan.leaf_index;
            if plan.config.root_source != RootSource::StandIn {
                bail!("this bridge takes roots from an RFP-021 attestation: use claim-attested");
            }
            diagnose_claim(wallet, bridge, &plan, index as u64, denom_index, denom, None).await?;
            let message = BridgeMessage::Instruction(Instruction::Claim {
                eth_token: plan.eth_token,
                denom_index,
                denom,
                root: plan.root,
                leaf_index: index as u64,
                path: plan.path.clone(),
                secret: plan.secret,
                cfg_id: plan.cfg_id,
                recipient_opening: plan.opening,
            });
            if let Some(path) = &emit.emit_call {
                let proof = call::root_entry(&bridge, &plan.root)?;
                return emit_claim(&plan, path, &[bridge, token_program], bridge, token_program, proof, &borsh::to_vec(&message)?, emit.max_fee);
            }
            let accounts = plan.mentions(wallet, bridge, token_program, pda(bridge, bc::root_seed(&plan.root)));
            if let Some(relay) = relay {
                return relayed_send(wallet, &relay, accounts, borsh::to_vec(&message)?, "claim").await.map(drop);
            }
            send(wallet, &[bridge, token_program], accounts, &borsh::to_vec(&message)?, plan.fee_payer()).await
        }
        Command::RelayCosign { tx_file, bridge, token_program, payer, max_gas, max_fee, submit, kind } => {
            if NO_PRECHECK.load(std::sync::atomic::Ordering::Relaxed) || DRY_RUN.load(std::sync::atomic::Ordering::Relaxed) {
                bail!("relay-cosign takes neither --no-precheck nor --dry-run (leave out --submit to only check)");
            }
            let sequencer = SEQUENCER.get().cloned().unwrap_or_default();
            relay::run_cosign(wallet, &sequencer, &tx_file, bridge, token_program, payer, max_gas, max_fee, submit, kind == "burn").await
        }
        Command::ClaimAttested {
            bridge,
            token_program,
            from,
            eth_token,
            denom_index,
            denom,
            source,
            chain,
            cfg_id,
            leaves,
            index,
            proof,
            anchor_state_root,
            anchor_block_hash,
            header_rlp,
            privacy,
            emit,
        } => {
            let (secret, leaves, index) = claim_inputs(wallet, bridge, from, &eth_token, denom, source, &chain, leaves, index).await?;
            let plan = plan_claim(wallet, bridge, from, &eth_token, denom, secret, cfg_id.as_deref(), leaves, index, &privacy, emit.emit_call.is_some()).await?;
            let index = plan.leaf_index;
            let RootSource::Attest(source) = plan.config.root_source else {
                bail!("this bridge uses the stand-in attestor: use claim");
            };
            let anchor = match (anchor_state_root, anchor_block_hash, header_rlp) {
                (Some(root), None, _) => AnchorWitness { anchor: Anchor::StateRoot(hex32(&root)?), header_rlp: None },
                (None, Some(hash), Some(path)) => {
                    let raw = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
                    let rlp = std::str::from_utf8(&raw).ok().and_then(|t| hex_bytes(t).ok()).unwrap_or(raw);
                    AnchorWitness { anchor: Anchor::BlockHash(hex32(&hash)?), header_rlp: Some(rlp) }
                }
                _ => bail!("give --anchor-state-root, or --anchor-block-hash with --header-rlp"),
            };
            let slot = mapping_slot(&plan.root, source.deposit_root_slot);
            let proof = storage_proof(&proof, &plan.config.vault, &slot)?;
            println!("isDepositRoot[root] slot 0x{} value 0x{}", to_hex(&slot), to_hex(&proof.value));
            diagnose_claim(wallet, bridge, &plan, index as u64, denom_index, denom, Some((&source, &anchor, &proof))).await?;
            let anchor_pda = eth_attest_core::anchor_address(&source.attest_program, &source.attest_instance, &anchor.anchor);
            let anchor_entry = call::anchor_entry(&source.attest_program, &source.attest_instance, &anchor.anchor)?;
            let attest_program = source.attest_program;
            let message = BridgeMessage::Instruction(Instruction::ClaimAttested {
                eth_token: plan.eth_token,
                denom_index,
                denom,
                root: plan.root,
                leaf_index: index as u64,
                path: plan.path.clone(),
                secret: plan.secret,
                cfg_id: plan.cfg_id,
                source,
                vault: plan.config.vault,
                evm_chain_id: plan.config.evm_chain_id,
                anchor,
                proof,
                recipient_opening: plan.opening,
            });
            if let Some(path) = &emit.emit_call {
                let programs = [bridge, token_program, attest_program];
                return emit_claim(&plan, path, &programs, bridge, token_program, anchor_entry, &borsh::to_vec(&message)?, emit.max_fee);
            }
            let accounts = plan.mentions(
                wallet,
                bridge,
                token_program,
                AccountIdentity::PublicNoSign(anchor_pda).select_program_actor_state(attest_program),
            );
            send(wallet, &[bridge, token_program, attest_program], accounts, &borsh::to_vec(&message)?, plan.fee_payer()).await
        }
        Command::Burn { bridge, token_program, from, definition, amount, exit, eth_dest, max_relay_fee, key_index, chain, emit, relay } => {
            let relay = relay.map(|r| relay::allowed_relay(&r, &SEQUENCER.get().cloned().unwrap_or_default())).transpose()?;
            if relay.is_some() && !matches!(identity(wallet, from), AccountIdentity::Public(_)) {
                bail!("--relay takes a public account of this wallet as --from");
            }
            let (memo, derived) = match exit {
                Some(e) if emit.emit_call.is_some() => {
                    let e = hex32(&e)?;
                    recover::refuse_burned_exit(wallet, bridge, &chain, &e).await?;
                    (e, None)
                }
                Some(e) => (hex32(&e)?, None),
                None => {
                    let (e, key, j) = recover::derived_exit(
                        wallet,
                        bridge,
                        from,
                        definition,
                        amount,
                        eth_dest.as_deref(),
                        max_relay_fee,
                        key_index,
                        &chain,
                    )
                    .await?;
                    println!("exit commitment 0x{}", to_hex(&e));
                    (e, Some((key, j)))
                }
            };
            let message = TokenMessage::BurnWithNotice { definition, amount, notify: Actor::new(config_address(&bridge), bridge), memo };
            if let Some(path) = &emit.emit_call {
                return emit_burn(wallet, bridge, token_program, definition, amount, &memo, &borsh::to_vec(&message)?, path, emit.max_fee).await;
            }
            let accounts = vec![
                signer(wallet, from, token_program),
                AccountIdentity::PublicNoSign(definition).select_program_actor_state(token_program),
                pda(bridge, config_seed()),
            ];
            if let Some(relay) = relay {
                println!("exit commitment 0x{}", to_hex(&memo));
                let landed = relayed_send(wallet, &relay, accounts, borsh::to_vec(&message)?, "burn").await?;
                if let (Some((key, j)), Some(_)) = (derived, landed) {
                    recover::record_exit(&recover::bridge_config(wallet, bridge).await?.cfg_id, &key, j)?;
                }
                return Ok(());
            }
            let applied = send_tx(wallet, &[token_program, bridge], accounts, &borsh::to_vec(&message)?, None).await?;
            if let (Some((key, j)), Some(_)) = (derived, applied) {
                recover::record_exit(&recover::bridge_config(wallet, bridge).await?.cfg_id, &key, j)?;
            }
            Ok(())
        }
        Command::DepositRequest(args) => recover::deposit_request(wallet, args).await,
        Command::Recover(args) => recover::recover(wallet, args).await,
        Command::Positions { bridge } => recover::positions(wallet, bridge).await,
        Command::EthKey(args) => recover::eth_key(wallet, args).await,
        Command::ExitSecret(args) => recover::exit_secret(wallet, args).await,
        Command::Balances { token_program } => {
            wallet.sync_to_latest_block().await?;
            let synced_to = wallet.get_last_block_id().await.ok();
            let mut holdings = Vec::new();
            let mut accounts = Vec::new();
            let key_chain = wallet.storage().key_chain();
            let public: Vec<AccountId> = key_chain.public_account_ids().map(|(id, _)| id).collect();
            let private: Vec<AccountId> = key_chain.private_account_ids().map(|(id, _)| id).collect();
            for (prefix, ids) in [("Public", public), ("Private", private)] {
                for id in ids {
                    let who = identity(wallet, id);
                    let state = if who.is_private() {
                        wallet
                            .get_account_private(id)
                            .map(|found| found.data.actor_state(token_program).to_vec())
                            .unwrap_or_default()
                    } else {
                        public_state(wallet, id, token_program).await?
                    };
                    let kind = prefix.to_lowercase();
                    accounts.push(serde_json::json!({ "account": id.to_string(), "kind": kind, "label": null }));
                    if let Some(TokenState::Holdings(h)) = TokenState::decode(&state) {
                        for (definition, balance) in h {
                            println!("{prefix}/{id}  {definition}  {balance}");
                            holdings.push(serde_json::json!({ "account": id.to_string(), "kind": kind, "definition": definition.to_string(), "balance": balance.to_string() }));
                        }
                    }
                }
            }
            out::doc(&serde_json::json!({ "synced_to": synced_to, "holdings": holdings, "accounts": accounts }));
            Ok(())
        }
        Command::Show { bridge } => {
            let c: Config = borsh::from_slice(&public_state(wallet, config_address(&bridge), bridge).await?)
                .context("no bridge config")?;
            println!("cfg_id 0x{}\nvault 0x{}\nchain {}", to_hex(&c.cfg_id), to_hex(&c.vault), c.evm_chain_id);
            println!("lez_chain_id 0x{}\nzone_id 0x{}", to_hex(&c.lez_chain_id), to_hex(&c.zone_id));
            println!("admin {}\nfreezer {}\nattestor {}", c.admin, c.freezer, c.attestor);
            println!("mint_frozen {}\nburn_frozen {}", c.mint_frozen, c.burn_frozen);
            match c.root_source {
                RootSource::StandIn => println!("root_source stand-in"),
                RootSource::Attest(a) => println!(
                    "root_source rfp-021 program {} instance 0x{} slot {}",
                    a.attest_program,
                    to_hex(&a.attest_instance),
                    a.deposit_root_slot
                ),
            }
            let float_now = float(wallet, &bridge).await?;
            println!("sponsor_amount {}\nfloat {}", c.sponsor_amount, float_now);
            out::doc(&show_json(&c, float_now));
            for t in c.tokens {
                println!(
                    "token 0x{} def {} denoms {:?} claims {:?} burns {:?} claimed_value {} burned_value {} registered {} mint_frozen {} burn_frozen {}",
                    to_hex(&t.eth_token),
                    t.definition,
                    t.denoms,
                    t.claims,
                    t.burns,
                    t.claimed_value,
                    t.burned_value,
                    t.registered,
                    t.mint_frozen,
                    t.burn_frozen
                );
            }
            Ok(())
        }
        Command::Definition { token_program, definition } => {
            match TokenState::decode(&public_state(wallet, definition, token_program).await?) {
                Some(TokenState::Definition(TokenDefinition::Fungible { name, total_supply, authority, .. })) => {
                    let (kind, id) = match authority {
                        Some(Authority::Program(p)) => (Some("program"), Some(p.to_string())),
                        Some(Authority::Account(a)) => (Some("account"), Some(a.to_string())),
                        None => (None, None),
                    };
                    // `authority Some(<id>)` as before the typed authority (the core module parses it),
                    // then the kind on its own line.
                    println!("name {name}\nsupply {total_supply}\nauthority {}", id.as_ref().map_or_else(|| "None".to_owned(), |i| format!("Some({i})")));
                    println!("authority_kind {}", kind.unwrap_or("none"));
                    out::doc(&serde_json::json!({ "name": name, "supply": total_supply.to_string(), "authority": id, "authority_kind": kind }));
                    Ok(())
                }
                _ => bail!("{definition} is not a token definition"),
            }
        }
    }
}

/// `show --json` (CLI-CONTRACT.md §2.1): amounts as decimal text.
fn show_json(c: &Config, float_now: Balance) -> serde_json::Value {
    use serde_json::json;
    let hx = |b: &[u8]| format!("0x{}", to_hex(b));
    let source = match c.root_source {
        RootSource::StandIn => json!({ "kind": "stand-in" }),
        RootSource::Attest(a) => json!({ "kind": "rfp-021", "program": a.attest_program.to_string(), "instance": hx(&a.attest_instance), "slot": a.deposit_root_slot }),
    };
    json!({
        "cfg_id": hx(&c.cfg_id), "vault": hx(&c.vault), "evm_chain_id": c.evm_chain_id,
        "lez_chain_id": hx(&c.lez_chain_id), "zone_id": hx(&c.zone_id), "token_program": c.token_program.to_string(),
        "admin": c.admin.to_string(), "pending_admin": c.pending_admin.map(|a| a.to_string()), "freezer": c.freezer.to_string(),
        "attestor": c.attestor.to_string(), "mint_frozen": c.mint_frozen, "burn_frozen": c.burn_frozen,
        "root_source": source, "sponsor_amount": c.sponsor_amount.to_string(), "float": float_now.to_string(),
        "tokens": c.tokens.iter().map(|t| json!({
            "eth_token": hx(&t.eth_token), "definition": t.definition.to_string(), "decimals": t.decimals,
            "denoms": t.denoms.iter().map(ToString::to_string).collect::<Vec<_>>(), "claims": t.claims, "burns": t.burns,
            "claimed_value": t.claimed_value.to_string(), "burned_value": t.burned_value.to_string(),
            "registered": t.registered, "mint_frozen": t.mint_frozen, "burn_frozen": t.burn_frozen,
        })).collect::<Vec<_>>(),
    })
}

/// `claim --emit-call` / `claim-attested --emit-call`: the claim as a Names Wallet private
/// call from the private account `--from` (`call.rs`). The accounts are [`ClaimPlan::mentions`]'
/// in the same order, the root actor (the claimer's bridge actor) implicit.
#[expect(clippy::too_many_arguments, reason = "the claim's parts")]
fn emit_claim(
    plan: &ClaimPlan,
    path: &std::path::Path,
    programs: &[AccountId],
    bridge: AccountId,
    token_program: AccountId,
    proof_entry: serde_json::Value,
    message: &[u8],
    max_fee: Option<Balance>,
) -> Result<()> {
    let tp = token_program.to_string();
    let mut accounts = vec![
        proof_entry,
        call::nullifier_entry(&bridge, &plan.nullifier)?,
        call::config_entry(&bridge, None)?,
        call::public(&plan.definition, &token_program),
        call::own(&tp, plan.opening.is_some()),
    ];
    if plan.sponsored {
        accounts.push(call::config_entry(&bridge, Some("native"))?);
        accounts.push(call::own("native", true));
    }
    let max_fee = max_fee.unwrap_or(if plan.sponsored { plan.config.sponsor_amount } else { call::DEFAULT_MAX_FEE });
    let fee_payer = if plan.sponsored { "account" } else { "wallet" };
    let openings: Vec<[u8; 32]> = plan.opening.into_iter().collect();
    let c = call::call(programs, accounts, message, &openings, fee_payer, vec![call::amount(&plan.definition, plan.denom)], vec![], max_fee)?;
    call::write(path, &c)?;
    println!("private call written to {} ({} byte message; it holds the claim secret: mode 600)", path.display(), message.len());
    out::set_result(serde_json::json!({
        "emitted": path.display().to_string(), "message_bytes": message.len(), "programs": c["programs"], "fee_payer": fee_payer,
        "declared": c["declared"], "recipient": plan.opening.map_or_else(|| plan.from.to_string(), |o| plan.from.blinded(&o).to_string()),
        "recipient_kind": if plan.opening.is_some() { "one_time" } else { "by_name" }, "sponsored": plan.sponsored,
        "sponsor": plan.config.sponsor_amount.to_string(), "root": format!("0x{}", to_hex(&plan.root)), "dry_run": true,
    }));
    Ok(())
}

/// `burn --emit-call`: `BurnWithNotice` as a Names Wallet private call from `--from`: programs
/// `[token, bridge]`, root actor its token actor, then the definition and the bridge's config
/// PDA; it declares the burned denomination as its only debit.
#[expect(clippy::too_many_arguments, reason = "the burn's parts")]
async fn emit_burn(
    wallet: &WalletCore<SequencerClient>,
    bridge: AccountId,
    token_program: AccountId,
    definition: AccountId,
    amount: Balance,
    memo: &[u8; 32],
    message: &[u8],
    path: &std::path::Path,
    max_fee: Option<Balance>,
) -> Result<()> {
    let config: Config = borsh::from_slice(&public_state(wallet, config_address(&bridge), bridge).await?).context("the bridge config")?;
    let t = config.tokens.iter().find(|t| t.definition == definition).ok_or_else(|| anyhow!("{definition} is not a bridged token of this bridge"))?;
    if !t.denoms.contains(&amount) {
        bail!("a burn is exactly one denomination: {:?}; nothing sent", t.denoms);
    }
    if config.burn_frozen || t.burn_frozen {
        bail!("redemption is frozen on this bridge{}: retry once the admin unfreezes it; nothing sent", if t.burn_frozen { " for this token" } else { "" });
    }
    let bp = bridge.to_string();
    let accounts = vec![call::public(&definition, &token_program), call::config_entry(&bridge, Some(&bp))?];
    let max_fee = max_fee.unwrap_or(call::DEFAULT_MAX_FEE);
    let c = call::call(&[token_program, bridge], accounts, message, &[], "wallet", vec![], vec![call::amount(&definition, amount)], max_fee)?;
    call::write(path, &c)?;
    println!("private call written to {} ({} byte message)", path.display(), message.len());
    out::set_result(serde_json::json!({
        "emitted": path.display().to_string(), "message_bytes": message.len(), "programs": c["programs"], "fee_payer": "wallet",
        "declared": c["declared"], "exit_commitment": format!("0x{}", to_hex(memo)), "dry_run": true,
    }));
    Ok(())
}

fn identity(wallet: &WalletCore<SequencerClient>, account: AccountId) -> AccountIdentity {
    if let Some(private) = wallet.resolve_private_account(account) {
        return private;
    }
    if wallet.get_account_public_signing_key(account).is_some() {
        AccountIdentity::Public(account)
    } else {
        AccountIdentity::PublicNoSign(account)
    }
}

fn signer(wallet: &WalletCore<SequencerClient>, account: AccountId, program: AccountId) -> wallet::AccountMention {
    identity(wallet, account).select_program_actor_state(program)
}

/// A PDA of `program` mentioned without admission evidence: the program's seeded effect admits
/// it (LEZ `execution_state.rs` `admit`). Mentioned with `PublicPda`, the wallet would attach
/// unsigned evidence a forwarder could strip.
fn pda(program: AccountId, seed: PdaSeed) -> AccountMention {
    AccountIdentity::PublicNoSign(AccountId::for_public_pda(&program, &seed)).select_program_actor_state(program)
}

/// The chain's Bedrock channel id, from the `sequencer_stake` program's config actor (written once
/// at genesis by `InitChannelParams`). No program can read it on LEZ rc3; this client can.
async fn chain_channel_id(wallet: &WalletCore<SequencerClient>) -> Result<[u8; 32]> {
    let program = sequencer_stake_core::sequencer_stake_account_id();
    let state = public_state(wallet, sequencer_stake_core::sequencer_stake_config_account_id(program), program).await?;
    let config: sequencer_stake_core::SequencerStakeConfig =
        borsh::from_slice(&state).map_err(|_| anyhow!("this chain's sequencer_stake config does not decode"))?;
    config.channel_id.ok_or_else(|| anyhow!("this chain records no channel id"))
}

/// `--lez-chain-id` for `init`: the chain's own channel id; a given value must equal it.
async fn init_chain_id(wallet: &WalletCore<SequencerClient>, given: Option<&str>) -> Result<[u8; 32]> {
    let chain = chain_channel_id(wallet).await?;
    if let Some(given) = given {
        let given = hex32(given)?;
        if given != chain {
            bail!("--lez-chain-id 0x{} is not this chain's channel id 0x{}", to_hex(&given), to_hex(&chain));
        }
    }
    Ok(chain)
}

/// `Initialize` is signed by the bridge's header account (the program id): its key must be here.
fn header_signs(wallet: &WalletCore<SequencerClient>, bridge: AccountId) -> Result<()> {
    if wallet.get_account_public_signing_key(bridge).is_none() {
        bail!("{bridge} (the bridge's header account) is not a public account of this wallet: init is signed by the deployer's header key");
    }
    Ok(())
}

/// The bridge's float: the config account's native balance.
async fn float(wallet: &WalletCore<SequencerClient>, bridge: &AccountId) -> Result<Balance> {
    let state = public_state(wallet, config_address(bridge), NATIVE_TOKEN_PROGRAM_ID).await?;
    decode_balance(&state).map_err(|e| anyhow!("the float: {e}"))
}

fn random32() -> Result<[u8; 32]> {
    use std::io::Read as _;
    let mut out = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut out)?;
    Ok(out)
}

/// What both claim commands work out before building the instruction.
struct ClaimPlan {
    from: AccountId,
    eth_token: EthAddress,
    denom: Balance,
    secret: [u8; 32],
    cfg_id: [u8; 32],
    root: [u8; 32],
    path: Vec<[u8; 32]>,
    nullifier: [u8; 32],
    definition: AccountId,
    config: Config,
    /// The one-time recipient's opening; `None` mints to `from` by name.
    opening: Option<[u8; 32]>,
    /// The float pays the one-time recipient in this transaction.
    sponsored: bool,
    /// The deposit's leaf index.
    leaf_index: usize,
}

impl ClaimPlan {
    /// The transaction's accounts: the claimer's bridge actor (root) first, then `proof_pda`
    /// (the root PDA or the attestation anchor PDA), the nullifier and config PDAs, the
    /// definition, and the claimer's token actor (with the opening, so its wallet receives
    /// through the alias); for a sponsored claim also the config's and the claimer's native
    /// actors.
    fn mentions(&self, wallet: &WalletCore<SequencerClient>, bridge: AccountId, token_program: AccountId, proof_pda: AccountMention) -> Vec<AccountMention> {
        claim_mentions(&identity(wallet, self.from), self.opening, self.sponsored, &self.nullifier, self.definition, bridge, token_program, proof_pda)
    }

    /// A sponsored claimer pays its own fee from the sponsored credit; name it, or the wallet may
    /// pick another funded private account of the same wallet.
    fn fee_payer(&self) -> Option<AccountId> {
        self.sponsored.then_some(self.from)
    }
}

/// [`ClaimPlan::mentions`] with the claimer's identity given (no wallet): the relayed-claim
/// vector test in `relay.rs` builds `claim --relay`'s accounts with it.
#[expect(clippy::too_many_arguments, reason = "the claim's parts")]
fn claim_mentions(
    who: &AccountIdentity,
    opening: Option<[u8; 32]>,
    sponsored: bool,
    nullifier: &[u8; 32],
    definition: AccountId,
    bridge: AccountId,
    token_program: AccountId,
    proof_pda: AccountMention,
) -> Vec<AccountMention> {
    let with_opening = |mut mention: AccountMention| {
        mention.address_openings.extend(opening);
        mention
    };
    let mut accounts = vec![
        who.clone().select_program_actor_state(bridge),
        proof_pda,
        pda(bridge, bc::nullifier_seed(nullifier)),
        pda(bridge, config_seed()),
        AccountIdentity::PublicNoSign(definition).select_program_actor_state(token_program),
        // Same `authorizes` as the signer mention above: the wallet refuses an account
        // mentioned with conflicting identities (a private claimer would otherwise fail).
        with_opening(who.clone().select_program_actor_state(token_program)),
    ];
    if sponsored {
        // The config's native actor: the float the custody transfer debits.
        accounts.push(AccountIdentity::PublicNoSign(config_address(&bridge)).select_program_actor_state(NATIVE_TOKEN_PROGRAM_ID));
        accounts.push(with_opening(who.clone().select_program_actor_state(NATIVE_TOKEN_PROGRAM_ID)));
    }
    accounts
}

fn read_leaves(path: &PathBuf) -> Result<Vec<[u8; 32]>> {
    std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(hex32)
        .collect()
}

/// The claim secret, the vault's leaves and the leaf index: derived from `--from`'s key with
/// `--key-index` (leaves and index from the vault's logs unless given), or the raw `--secret`
/// (leaves from `--leaves`, else from the vault's logs up to the finalized block; the index from
/// `--index`, else found by [`plan_claim`]).
#[expect(clippy::too_many_arguments, reason = "the claim's command-line inputs")]
async fn claim_inputs(
    wallet: &WalletCore<SequencerClient>,
    bridge: AccountId,
    from: AccountId,
    eth_token: &str,
    denom: Balance,
    source: SecretSource,
    chain: &recover::ChainArgs,
    leaves: Option<PathBuf>,
    index: Option<usize>,
) -> Result<([u8; 32], Vec<[u8; 32]>, Option<usize>)> {
    let raw = match (&source.secret, &source.secret_file) {
        (Some(s), None) => Some(hex32(s)?),
        (None, Some(p)) => Some(call::read_secret(p)?),
        _ => None,
    };
    match (raw, source.key_index) {
        (Some(secret), None) => {
            let leaves = match leaves {
                Some(p) => read_leaves(&p)?,
                None => recover::finalized_leaves(wallet, bridge, chain).await?,
            };
            Ok((secret, leaves, index))
        }
        (None, Some(j)) => {
            let (secret, all, found) = recover::derived_claim(wallet, bridge, from, eth(eth_token)?, denom, j, chain).await?;
            if index.is_some_and(|i| i != found) {
                bail!("--index {} is not this deposit's leaf ({found})", index.unwrap_or_default());
            }
            let leaves = match leaves {
                Some(p) => read_leaves(&p)?,
                None => all,
            };
            Ok((secret, leaves, Some(found)))
        }
        _ => bail!("give --key-index N (derived from this wallet's key), or --secret-file (raw)"),
    }
}

#[expect(clippy::too_many_arguments, reason = "the claim's command-line inputs")]
async fn plan_claim(
    wallet: &WalletCore<SequencerClient>,
    bridge: AccountId,
    from: AccountId,
    eth_token: &str,
    denom: Balance,
    secret: [u8; 32],
    cfg_id: Option<&str>,
    leaves: Vec<[u8; 32]>,
    index: Option<usize>,
    privacy: &ClaimPrivacy,
    // `--emit-call`: `from` is a private account of the Names Wallet, not of this wallet.
    names_private: bool,
) -> Result<ClaimPlan> {
    let config: Config = borsh::from_slice(&public_state(wallet, config_address(&bridge), bridge).await?)
        .context("the bridge config")?;
    // A config made by `Initialize` names its chain (review M1); a program can't check it, so the
    // claimer's client does: a bridge initialised for another chain is a copy, not the deployment.
    if config.lez_chain_id != [0; 32] {
        let chain = chain_channel_id(wallet).await?;
        if config.lez_chain_id != chain {
            bail!("this bridge was initialised for LEZ chain 0x{}, not this chain (0x{}): refusing to claim on a copy", to_hex(&config.lez_chain_id), to_hex(&chain));
        }
    }
    let cfg_id = cfg_id.map(hex32).transpose()?.unwrap_or(config.cfg_id);
    let eth_token = eth(eth_token)?;
    let c = bc::deposit_commitment(&secret, &from, &eth_token, denom, &cfg_id);
    let leaf = bc::deposit_leaf(&cfg_id, &eth_token, denom, &c);
    let index = deposit_leaf_index(&leaves, &leaf, index).map_err(|e| anyhow!("{e} (commitment 0x{} for {from})", to_hex(&c)))?;
    println!("deposit leaf {index} of {}", leaves.len());
    let path = bc::merkle_path(&leaves, index);
    let root = bc::merkle_root(&leaf, index as u64, &path);
    let nullifier = bc::claim_nullifier(&secret, &from, &eth_token, denom);
    println!("root 0x{}", to_hex(&root));
    println!("claim nullifier 0x{}", to_hex(&nullifier));
    let definition = config
        .tokens
        .iter()
        .find(|t| t.eth_token == eth_token)
        .ok_or_else(|| anyhow!("token not registered"))?
        .definition;
    let private = names_private || identity(wallet, from).is_private();
    let opening = (private && !privacy.reveal_recipient).then(|| bc::recipient_opening(&secret));
    let sponsored = opening.is_some() && config.sponsor_amount > 0;
    match opening {
        Some(o) => println!("recipient {} (one-time address of {from}; the opening stays on this machine)", from.blinded(&o)),
        None if private => println!("recipient {from} BY NAME (--reveal-recipient): the mint publishes this private account"),
        None => println!("recipient {from} (public claimer)"),
    }
    if sponsored {
        let have = float(wallet, &bridge).await?;
        println!("sponsor {} from the bridge float (float {have})", config.sponsor_amount);
        if have < config.sponsor_amount {
            bail!(
                "the bridge float holds {have}, less than the sponsor amount {}: this claim would fail. \
                 Top it up (`lez-bridge fund-float --bridge {bridge} --from <public account> --amount N`) and retry; \
                 nothing was proven or sent",
                config.sponsor_amount
            );
        }
    } else if private {
        println!(
            "no sponsoring (sponsor_amount {}): {from} pays this transaction's fee from its own balance \
             (fund it with `lez-bridge shield`, which credits a one-time address)",
            config.sponsor_amount
        );
    }
    out::event(
        "plan",
        serde_json::json!({
            "root": format!("0x{}", to_hex(&root)),
            "recipient": opening.map_or_else(|| from.to_string(), |o| from.blinded(&o).to_string()),
            "recipient_kind": if opening.is_some() { "one_time" } else if private { "by_name" } else { "public" },
            "sponsor": if sponsored { config.sponsor_amount.to_string() } else { "0".to_owned() },
            "float": if sponsored { Some(float(wallet, &bridge).await?.to_string()) } else { None },
        }),
    );
    Ok(ClaimPlan { from, eth_token, denom, secret, cfg_id, root, path, nullifier, definition, config, opening, sponsored, leaf_index: index })
}

/// The deposit's leaf index: `given` if that leaf is the deposit's, else (with none given) the
/// first leaf equal to it. Commitments are unique in the vault (`CommitmentUsed`), so is the leaf.
fn deposit_leaf_index(leaves: &[[u8; 32]], leaf: &[u8; 32], given: Option<usize>) -> std::result::Result<usize, String> {
    match given {
        Some(i) if leaves.get(i) == Some(leaf) => Ok(i),
        Some(i) => Err(format!("leaf {i} is not this deposit")),
        None => leaves.iter().position(|l| l == leaf).ok_or_else(|| {
            format!(
                "no leaf of the {} read is this deposit: a wrong secret, account, token or amount, \
                 or the deposit is not finalized on Ethereum yet (about 15 minutes)",
                leaves.len()
            )
        }),
    }
}

/// Tells the user, before anything is proven or sent, which refusal this claim would meet. On
/// chain every deposit-dependent failure is the one text `claim refused` (RFP-022 Privacy #5:
/// a rejected, a repeated and a capped claim must not be told apart); here the same checks run
/// natively and against public state, so the user still learns which case applies (Usability
/// #8). These messages stay on this machine. The PDAs read (config, root or anchor, nullifier)
/// are the ones the claim's own simulation reads from the same sequencer anyway.
async fn diagnose_claim(
    wallet: &WalletCore<SequencerClient>,
    bridge: AccountId,
    plan: &ClaimPlan,
    leaf_index: u64,
    denom_index: u32,
    denom: Balance,
    attested: Option<(&AttestSource, &AnchorWitness, &StorageProof)>,
) -> Result<()> {
    if NO_PRECHECK.load(std::sync::atomic::Ordering::Relaxed) {
        println!("pre-check SKIPPED (--no-precheck, tests only)");
        return Ok(());
    }
    let c = &plan.config;
    if plan.cfg_id != c.cfg_id {
        bail!(
            "claim for another deployment: the deposit's cfg id 0x{} is not this bridge's 0x{}; nothing sent",
            to_hex(&plan.cfg_id),
            to_hex(&c.cfg_id)
        );
    }
    let token = c
        .tokens
        .iter()
        .find(|t| t.eth_token == plan.eth_token)
        .ok_or_else(|| anyhow!("token not registered on this bridge; nothing sent"))?;
    if c.mint_frozen {
        bail!("minting is frozen on this bridge (all tokens): retry once the admin unfreezes it; nothing sent");
    }
    if token.mint_frozen {
        bail!("minting is frozen for this token: retry once the admin unfreezes it; nothing sent");
    }
    match token.denoms.get(denom_index as usize) {
        None => bail!("bad denomination: index {denom_index}, the token has {:?}; nothing sent", token.denoms),
        Some(&d) if d != denom => bail!("wrong amount: denomination {denom_index} is {d}, not {denom}; nothing sent"),
        Some(_) => {}
    }
    bc::check_deposit_in_root(&plan.secret, &plan.from, &plan.eth_token, denom, &plan.cfg_id, &plan.root, leaf_index, &plan.path)
        .map_err(|e| anyhow!("{e}; nothing sent"))?;
    match attested {
        None => {
            if public_state(wallet, bc::root_address(&bridge, &plan.root), bridge).await?.is_empty() {
                bail!(
                    "root 0x{} is not attested on this bridge yet: wait for the attestor to record it, then retry; nothing sent",
                    to_hex(&plan.root)
                );
            }
        }
        Some((source, anchor, proof)) => {
            bc::check_attested_root(source, &c.vault, c.evm_chain_id, &plan.root, anchor, proof)
                .map_err(|e| anyhow!("{e}; nothing sent"))?;
            let pda = eth_attest_core::anchor_address(&source.attest_program, &source.attest_instance, &anchor.anchor);
            if public_state(wallet, pda, source.attest_program).await?.is_empty() {
                bail!(
                    "insufficient finality: the attestation instance has not finalized this anchor (yet); \
                     wait for a later finality update, or prove against an anchor it recorded; nothing sent"
                );
            }
        }
    }
    if !public_state(wallet, bc::nullifier_address(&bridge, &plan.nullifier), bridge).await?.is_empty() {
        // Same words as the old on-chain text, which scripts/e2e-private-steps.sh greps for.
        bail!("deposit already claimed: its nullifier is spent; nothing sent");
    }
    println!("pre-check passed: attested root, unspent nullifier, minting open, denomination {denom_index} = {denom}");
    Ok(())
}

/// An `eth_getProof` result (bare, or inside a JSON-RPC response) for `vault`'s `slot`.
fn storage_proof(path: &PathBuf, vault: &EthAddress, slot: &[u8; 32]) -> Result<StorageProof> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let json: serde_json::Value = serde_json::from_str(&text).context("the proof is not JSON")?;
    storage_proof_json(&json, vault, slot)
}

/// [`storage_proof`] on parsed JSON; `vault` is whichever contract the proof must be for.
fn storage_proof_json(json: &serde_json::Value, vault: &EthAddress, slot: &[u8; 32]) -> Result<StorageProof> {
    let pf = json.get("result").unwrap_or(json);
    let field = |v: &serde_json::Value, k: &str| -> Result<String> {
        v.get(k).and_then(|x| x.as_str()).map(str::to_owned).ok_or_else(|| anyhow!("eth_getProof: no `{k}`"))
    };
    let nodes = |v: &serde_json::Value, k: &str| -> Result<Vec<Vec<u8>>> {
        v.get(k)
            .and_then(|x| x.as_array())
            .ok_or_else(|| anyhow!("eth_getProof: no `{k}`"))?
            .iter()
            .map(|n| hex_bytes(n.as_str().unwrap_or_default()))
            .collect()
    };
    let address: [u8; 20] = hex_bytes(&field(pf, "address")?)?.try_into().map_err(|_| anyhow!("bad address"))?;
    if &address != vault {
        bail!("the proof is for 0x{}, expected 0x{}", to_hex(&address), to_hex(vault));
    }
    let sp = pf
        .get("storageProof")
        .and_then(|x| x.as_array())
        .and_then(|a| a.first())
        .ok_or_else(|| anyhow!("eth_getProof: no storageProof"))?;
    if word32(&field(sp, "key")?)? != *slot {
        bail!("the storage proof is for another slot; expected keccak256(abi.encode(root, N)) = 0x{}", to_hex(slot));
    }
    let nonce = u64::from_str_radix(field(pf, "nonce")?.trim_start_matches("0x"), 16).context("nonce")?;
    Ok(StorageProof {
        account: AccountProof {
            fields: AccountFields {
                nonce,
                balance: word32(&field(pf, "balance")?)?,
                storage_root: word32(&field(pf, "storageHash")?)?,
                code_hash: word32(&field(pf, "codeHash")?)?,
            },
            nodes: nodes(pf, "accountProof")?,
        },
        value: word32(&field(sp, "value")?)?,
        nodes: nodes(sp, "proof")?,
    })
}

/// A JSON-RPC quantity or data word, left-padded to 32 bytes.
fn word32(text: &str) -> Result<[u8; 32]> {
    let t = text.trim().trim_start_matches("0x");
    if t.len() > 64 {
        bail!("more than 32 bytes: {text}");
    }
    let padded = format!("{t:0>64}");
    hex32(&padded)
}

async fn public_state(wallet: &WalletCore<SequencerClient>, account: AccountId, program: AccountId) -> Result<Vec<u8>> {
    let view = wallet.get_account_view(Actor::new(account, program)).await?;
    Ok(view.map_or_else(Vec::new, |found| found.data.actor_state(program).to_vec()))
}

async fn catalog(wallet: &WalletCore<SequencerClient>, programs: &[AccountId]) -> Result<ProgramCatalog> {
    let mut entries = Vec::new();
    for program in programs {
        let deployed = ProgramLoader(wallet)
            .program(*program)
            .await?
            .ok_or_else(|| anyhow!("{program} is not a deployed program"))?;
        entries.push((*program, deployed));
    }
    Ok(entries.into_iter().collect::<ProgramCatalog>())
}

/// Sends `message` to the first account's actor; `fee_payer` names a private transaction's fee
/// payer (else the wallet picks one).
async fn send(
    wallet: &mut WalletCore<SequencerClient>,
    programs: &[AccountId],
    accounts: Vec<AccountMention>,
    message: &[u8],
    fee_payer: Option<AccountId>,
) -> Result<()> {
    send_tx(wallet, programs, accounts, message, fee_payer).await.map(drop)
}

/// [`send`], returning the included transaction and its block (`None` on a dry run).
async fn send_tx(
    wallet: &mut WalletCore<SequencerClient>,
    programs: &[AccountId],
    accounts: Vec<AccountMention>,
    message: &[u8],
    fee_payer: Option<AccountId>,
) -> Result<Option<(LeeTransaction, u64)>> {
    let private = accounts.iter().any(|mention| mention.identity.is_private());
    let catalog = catalog(wallet, programs).await?;
    let dry_run = DRY_RUN.load(std::sync::atomic::Ordering::Relaxed);
    out::phase("simulate", serde_json::json!({}));
    if private && !dry_run {
        println!("privacy-preserving: proving on this computer");
        // The wallet simulates, then proves, inside one call: `prove` starts with it.
        out::phase("prove", serde_json::json!({ "threads": std::env::var("RAYON_NUM_THREADS").ok().and_then(|t| t.parse::<u32>().ok()) }));
    }
    let started = std::time::Instant::now();
    let sent = wallet
        .send_tx_with_pre_check(accounts, 0, message.to_vec(), &catalog, None, AsyncDelivery::default(), fee_payer, |_| Ok(()))
        .await;
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let (hash, _) = match sent {
        Err(ExecutionFailureKind::DryRun(report)) => {
            // The wallet's cost lines, as rc1 printed them itself (scripts parse them).
            println!("{report}");
            println!("dry run: simulated in {:.1?}; nothing proven or sent", started.elapsed());
            out::event("simulated", serde_json::json!({ "elapsed_ms": elapsed_ms, "private": private }));
            out::set_result(serde_json::json!({ "dry_run": true }));
            return Ok(None);
        }
        other => other?,
    };
    println!("{} in {:.1?}", if private { "proved and submitted" } else { "submitted" }, started.elapsed());
    out::phase("submit", serde_json::json!({}));
    out::event("submitted", serde_json::json!({ "tx": hash.to_string(), "elapsed_ms": elapsed_ms }));
    let landed = finish_tx(wallet, hash).await?;
    out::set_result(serde_json::json!({ "block": landed.1, "elapsed_s": started.elapsed().as_secs_f64(), "dry_run": false }));
    Ok(Some(landed))
}

/// `claim --relay`: build and sign the claim with the relay's payer as fee payer, have the relay
/// co-sign and submit it (up to RELAY_TRIES tries when the payer's nonce moved or the relay is busy
/// with another claim; `relay::post` waits out a busy answer), then confirm it as
/// any other claim. `--dry-run`: the relay checks it and signs nothing.
/// Tries of a relayed claim: each busy answer waits up to 30 s (the relay holds one claim in flight).
const RELAY_TRIES: u32 = 6;

/// `what`: "claim" or "burn" (`bridge_relayBurn`). The block it landed in, or `None` for a dry run.
/// `--no-precheck` (tests and the relay's refusal probes only): built on the relay's caps without
/// measuring, so a call that would revert reaches the relay, whose own simulation refuses it.
async fn relayed_send(wallet: &mut WalletCore<SequencerClient>, relay: &relay::Relay, accounts: Vec<AccountMention>, message: Vec<u8>, what: &str) -> Result<Option<u64>> {
    let dry_run = DRY_RUN.load(std::sync::atomic::Ordering::Relaxed);
    let unmeasured = NO_PRECHECK.load(std::sync::atomic::Ordering::Relaxed);
    let burn = what == "burn";
    let (payer, max_gas, max_fee) = relay::info(relay).await?;
    println!("relay {relay}: fee payer {payer}, caps gas {max_gas}, max_fee {max_fee}");
    let started = std::time::Instant::now();
    for attempt in 1..=RELAY_TRIES {
        let half = if unmeasured {
            relay::build_unmeasured(wallet, accounts.clone(), message.clone(), payer, max_gas, max_fee).await?
        } else {
            relay::build(wallet, accounts.clone(), message.clone(), payer, max_fee, dry_run).await?
        };
        println!("relayed {what}: declared gas {}, max_fee {} (measured {} cycles), signed by the sender", half.gas, half.max_fee, half.cycles);
        if half.gas > max_gas || half.max_fee > max_fee {
            bail!("this {what} needs gas {} / max_fee {}, above the relay's caps {max_gas} / {max_fee}; nothing sent", half.gas, half.max_fee);
        }
        out::phase("relay", serde_json::json!({ "relay": relay.to_string(), "attempt": attempt }));
        match relay::post(relay, &half.tx, dry_run, burn).await? {
            Ok(None) => {
                println!("dry run: the relay checked the {what} and would co-sign it; nothing signed by the relay or sent");
                out::set_result(serde_json::json!({ "dry_run": true, "relay": true }));
                return Ok(None);
            }
            Ok(Some(hash)) => {
                println!("relayed and submitted in {:.1?}", started.elapsed());
                out::event("submitted", serde_json::json!({ "tx": hash.to_string(), "relay": true }));
                let landed = finish_tx(wallet, hash).await?;
                out::set_result(serde_json::json!({ "block": landed.1, "elapsed_s": started.elapsed().as_secs_f64(), "dry_run": false, "relay": true }));
                return Ok(Some(landed.1));
            }
            Err((reason, true)) if attempt < RELAY_TRIES => println!("relay: {reason} (try {attempt} of {RELAY_TRIES})"),
            Err((reason, _)) => bail!("the relay refused the {what}: {reason}; nothing sent"),
        }
    }
    unreachable!("the last attempt returns")
}

async fn finish(wallet: &mut WalletCore<SequencerClient>, hash: HashType) -> Result<()> {
    finish_tx(wallet, hash).await.map(drop)
}

async fn finish_tx(wallet: &mut WalletCore<SequencerClient>, hash: HashType) -> Result<(LeeTransaction, u64)> {
    println!("tx {hash}");
    out::phase("confirm", serde_json::json!({ "tx": hash.to_string() }));
    let (tx, block_id) = wallet.poll_transaction(hash).await.context("not included")?;
    let block = wallet
        .get_block(block_id)
        .await?
        .ok_or_else(|| anyhow!("block {block_id} is missing"))?;
    let reverted = block
        .body
        .transactions
        .iter()
        .position(|included| included.hash() == hash)
        .is_some_and(|position| block.body.is_reverted(position));
    if let LeeTransaction::PrivacyPreserving(tx) = &tx {
        let key_chain = wallet.storage_mut().key_chain_mut();
        let mut index = key_chain.build_latest_nullifier_index();
        key_chain.follow_transaction(&tx.message, reverted, false, &mut index);
        wallet.store_persistent_data()?;
    }
    if reverted {
        bail!("{hash} was included in block {block_id} but reverted");
    }
    println!("applied in block {block_id}");
    Ok((tx, block_id))
}

fn words_hex(words: &[u32; 8]) -> String {
    words.iter().flat_map(|w| w.to_le_bytes()).map(|b| format!("{b:02x}")).collect()
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_bytes(text: &str) -> Result<Vec<u8>> {
    let text = text.trim().trim_start_matches("0x");
    if text.len() % 2 != 0 {
        bail!("odd-length hex");
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|e| anyhow!("bad hex: {e}")))
        .collect()
}

fn hex32(text: &str) -> Result<[u8; 32]> {
    hex_bytes(text)?.try_into().map_err(|_| anyhow!("expected 32 bytes of hex"))
}

fn eth(text: &str) -> Result<EthAddress> {
    hex_bytes(text)?.try_into().map_err(|_| anyhow!("expected a 20-byte address"))
}

/// The privacy circuit this build proves with is LEZ v0.4.0-rc3's, the one the outbound guest
/// pins (`outbound_core::PRIVACY_PRESERVING_CIRCUIT_ID`). Changes with every LEZ circuit change.
#[cfg(test)]
mod circuit_tests {
    #[test]
    fn the_privacy_circuit_is_lez_rc3s() {
        let bytes: Vec<u8> = lee::PRIVACY_PRESERVING_CIRCUIT_ID.iter().flat_map(|w| w.to_le_bytes()).collect();
        assert_eq!(super::to_hex(&bytes), "631f0c96572a86ce63d1b9c4e98c8383cabb016b3c852017101452b992cf0c38");
    }
}

#[cfg(test)]
mod leaf_tests {
    use super::deposit_leaf_index;
    use crate::recover::eth_from_block;

    #[test]
    fn the_leaf_index_is_found_or_checked() {
        let leaves = [[1u8; 32], [2; 32], [3; 32]];
        assert_eq!(deposit_leaf_index(&leaves, &[2; 32], None), Ok(1));
        assert_eq!(deposit_leaf_index(&leaves, &[3; 32], Some(2)), Ok(2));
        assert!(deposit_leaf_index(&leaves, &[3; 32], Some(1)).is_err());
        assert!(deposit_leaf_index(&leaves, &[3; 32], Some(9)).is_err());
        assert!(deposit_leaf_index(&leaves, &[4; 32], None).is_err());
        assert!(deposit_leaf_index(&[], &[4; 32], None).is_err());
    }

    #[test]
    fn the_demo_vault_logs_start_at_its_creation_block() {
        let v2: [u8; 20] = super::eth("0x8C8b2664De28e5287158db4DB65D448638871b74").unwrap();
        assert_eq!(eth_from_block(&v2, 11_155_111, 0), 12_122_976);
        assert_eq!(eth_from_block(&v2, 11_155_111, 5), 5, "a given block wins");
        assert_eq!(eth_from_block(&v2, 1, 0), 0, "another chain");
        assert_eq!(eth_from_block(&[7; 20], 11_155_111, 0), 0, "another vault");
    }
}

#[cfg(test)]
mod sequencer_tests {
    use super::{PUBLIC_DEMO_SEQUENCER, allowed_sequencer};

    #[test]
    fn loopback_and_the_demo_chain_are_allowed() {
        for ok in [
            "http://127.0.0.1:3290",
            "http://127.0.0.1:3295/",
            "http://localhost:3250",
            "http://[::1]:3290",
            PUBLIC_DEMO_SEQUENCER,
            "https://lez.nameswallet.app",
            "https://LEZ.nameswallet.app/",
            "https://lez.nameswallet.app:443/",
        ] {
            assert!(allowed_sequencer(ok).is_ok(), "{ok} should be allowed");
        }
    }

    #[test]
    fn everything_else_is_refused() {
        for bad in [
            "http://lez.nameswallet.app/",
            "https://lez.nameswallet.app:8443/",
            "https://lez.nameswallet.app/rpc",
            "https://lez.nameswallet.app/?x=1",
            "https://user:pw@lez.nameswallet.app/",
            "https://lez.nameswallet.app.example.com/",
            "https://nameswallet.app/",
            "https://status.nameswallet.app/",
            "https://testnet.lez.logos.co/",
            "http://192.168.1.10:3290",
            "http://0.0.0.0:3290",
            "not a url",
        ] {
            assert!(allowed_sequencer(bad).is_err(), "{bad} should be refused");
        }
    }
}
