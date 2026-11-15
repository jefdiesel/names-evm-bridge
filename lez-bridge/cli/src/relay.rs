//! Relayed public claims (`claim --relay URL`, and the operator's `relay-cosign`).
//!
//! A public claim's fee payer must sign it and hold the fee reserve before it runs (LEZ
//! `lez/chain_state/src/apply.rs` "Phase 1: Reserve"; `lee::is_fee_authorized`). A never-used
//! account holds nothing, and nobody can credit it by its account id alone (LEZ
//! `execution_state.rs` `admit`: an absent public account is admitted only by a signer, admission
//! evidence of its key, or its program's PDA seed). The claimer's own signature admits it; what is
//! missing is the fee. So the claim names a second signer, the relayer's payer account, in its
//! `FeeDeclaration` and nonce map, and the relayer co-signs it:
//!
//! 1. The claimer's CLI plans the claim as for a self-paid public claim, measures it with the
//!    wallet's own dry run (gas, `max_fee`, revert check), then builds the same message with the
//!    payer added (nonce, authorized account, fee payer), signs it with the claimer's key and
//!    POSTs the half-signed transaction to the relay.
//! 2. The relayer (`relay-cosign`, run by `public-demo/relayer/relayer.py`) decodes it strictly,
//!    checks that it is exactly a public stand-in claim on its bridge (the root call, the
//!    instruction, the declared actors, no admission evidence, the nonces, the fee caps), runs the
//!    claim pre-checks (attested root, unspent nullifier, denomination, the deposit in the root),
//!    simulates it (it must apply), then signs as payer and submits.
//!
//! The payer's signature covers the whole message (`Message::hash`), with its nonce: it is good for
//! this one transaction only, on every chain where the payer's nonce matches: the rc3 message hash
//! (`/LEE/v0.3/Message/Public/` + the message, `lee/state_machine/src/public_transaction/message.rs`)
//! names no chain, channel or genesis, so the relayer's payer must be a new key for each chain (the
//! relayer binds it to one chain and refuses any other). The payer is authorized in the execution
//! (LEZ requires `authorized_accounts` = signers), so the relayer refuses any message that declares
//! an actor of the payer's account, and refuses to run with a payer that holds a bridge role.
//!
//! The relayer has one co-signed claim in flight at a time: two built on the same payer nonce
//! would leave one silently dropped. While its last one is not in a block it answers "busy" with
//! `retry_after`; [`post`] waits that long (capped) and the caller rebuilds the claim on the
//! payer's fresh nonce. Texts a client sees are fixed; internal errors go to the relayer's log only
//! (`detail`). A submission whose answer was lost is reported `uncertain`, with the transaction's
//! hash, so the relayer can settle it from the chain.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{Context as _, Result, anyhow, bail};
use bridge_core::{self as bc, Config, Instruction, Message as BridgeMessage, RootSource, config_address};
use bridge_token_core::Message as TokenMessage;
use common::{HashType, transaction::LeeTransaction};
use lee::{
    FeeDeclaration, PublicKey, PublicTransaction, Signature, Simulation,
    public_transaction::{Message, WitnessSet},
};
use lee_core::account::{AccountId, Actor, ActorState};
use sequencer_service_rpc::SequencerClient;
use serde_json::{Value, json};
use wallet::{AccountMention, DryRunReport, ExecutionFailureKind, WalletCore};

/// Largest half-signed claim the relayer reads (a public claim is about 1.9 KB on rc1 and rc3).
pub const MAX_TX_BYTES: usize = 8192;

/// Where `claim --relay` sends the half-signed claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Relay {
    /// The chain's own JSON-RPC endpoint (`--relay` with no value, or `auto`): the methods
    /// `bridge_relayInfo` and `bridge_relayClaim`, which the demo chain's RPC filter serves from the
    /// relay (`public-demo/proxy/lez-rpc-proxy.py --relay`). The URL is the `--sequencer` in use.
    Rpc(url::Url),
    /// A relay's own HTTP side on loopback (`http://127.0.0.1:PORT/relay`; tests).
    Direct(url::Url),
}

impl std::fmt::Display for Relay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rpc(u) => write!(f, "{u} (bridge_relayClaim)"),
            Self::Direct(u) => write!(f, "{u}"),
        }
    }
}

/// `--relay` as given: `auto` (or no value) is the `--sequencer` endpoint, already checked by
/// `allowed_sequencer`; otherwise a loopback `http://…/relay` URL. Nothing else is accepted.
pub fn allowed_relay(text: &str, sequencer: &str) -> Result<Relay> {
    if text.is_empty() || text == "auto" {
        return Ok(Relay::Rpc(sequencer.parse().context("the sequencer URL")?));
    }
    let url: url::Url = text.parse().context("--relay is not a URL (or give it no value to use the chain's endpoint)")?;
    let host = url.host_str().unwrap_or_default();
    if matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1") && url.scheme() == "http" && url.path().trim_end_matches('/').ends_with("/relay") {
        return Ok(Relay::Direct(url));
    }
    bail!("--relay {text}: give no value (the chain's endpoint, {sequencer}) or a loopback http://…/relay URL")
}

fn http() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .user_agent("lez-bridge-relay-client/1")
        .build()
        .context("http client")
}

fn info_url(relay: &url::Url) -> url::Url {
    let mut u = relay.clone();
    let path = format!("{}/info", relay.path().trim_end_matches('/'));
    u.set_path(&path);
    u
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(text: &str) -> Result<Vec<u8>> {
    let t = text.trim().trim_start_matches("0x");
    if t.len() % 2 != 0 || !t.bytes().all(|c| c.is_ascii_hexdigit()) {
        bail!("not hex");
    }
    (0..t.len()).step_by(2).map(|i| u8::from_str_radix(&t[i..i + 2], 16).map_err(Into::into)).collect()
}

/// The declared actors of `accounts`, first mention first, without repeats (as the wallet does).
fn actors_of(accounts: &[AccountMention]) -> Vec<Actor> {
    let mut out = Vec::new();
    for m in accounts {
        let a = m.actor();
        if !out.contains(&a) {
            out.push(a);
        }
    }
    out
}

/// The wire size settlement prices: the whole signed transaction with `signers` signatures.
fn wire_bytes(message: &Message, signers: usize) -> Result<u64> {
    let bare = borsh::to_vec(&LeeTransaction::Public(PublicTransaction::new(message.clone(), WitnessSet::from_raw_parts(Vec::new()))))?;
    let witness = size_of::<Signature>() + size_of::<PublicKey>();
    Ok(u64::try_from(bare.len() + signers * witness)?)
}

/// The relayed claim's message: the root call to the first mention's actor, the declared actors,
/// both signers' nonces, the fee naming the relay's payer, no admission evidence. The one place
/// `claim --relay` builds it; the vector test below builds it here too.
pub fn relayed_message(accounts: &[AccountMention], message: Vec<u8>, nonces: BTreeMap<AccountId, lee_core::account::Nonce>, fee: FeeDeclaration) -> Result<Message> {
    let to = accounts.first().ok_or_else(|| anyhow!("no accounts"))?.actor();
    Ok(Message::new(to, message, actors_of(accounts), nonces, Some(fee), Vec::new()))
}

/// What the claimer's CLI sends: the claim signed by the claimer, the payer's signature missing.
pub struct HalfSigned {
    pub tx: PublicTransaction,
    pub gas: u64,
    pub max_fee: u128,
    pub cycles: u64,
}

/// Measures the claim (self-paid, the wallet's dry run), then builds and signs the relayed version.
pub async fn build(
    wallet: &mut WalletCore<SequencerClient>,
    accounts: Vec<AccountMention>,
    message: Vec<u8>,
    payer: AccountId,
    fee_cap: u128,
    dry_run_after: bool,
) -> Result<HalfSigned> {
    let to = accounts.first().ok_or_else(|| anyhow!("no accounts"))?.actor();
    let claimer = to.account_id;
    if claimer == payer {
        bail!("the relay's payer is the claimer itself: claim without --relay");
    }
    if wallet.get_account_public_signing_key(claimer).is_none() {
        bail!("{claimer} is not a public account of this wallet: a relayed claim is signed by the claimer");
    }
    wallet.set_dry_run(true);
    let measured = wallet.send_pub_tx_with_pre_check(accounts.clone(), 0, message.clone(), None, |_| Ok(())).await;
    wallet.set_dry_run(dry_run_after);
    let report = match measured {
        Err(ExecutionFailureKind::DryRun(report)) => report,
        Ok(hash) => bail!("internal error: the measuring dry run sent {hash}"),
        Err(e) => return Err(anyhow!(e).context("measuring the transaction")),
    };
    let DryRunReport::Public { fee, cycles, wire_bytes: measured_bytes, revert_reason, reserve_at_head, .. } = *report else {
        bail!("internal error: a public claim measured as private");
    };
    if let Some(why) = revert_reason {
        bail!("it would revert: {why}; nothing sent");
    }
    let nonces = wallet.get_accounts_nonces(&[claimer, payer]).await?;
    let nonces: BTreeMap<_, _> = [(claimer, nonces[0]), (payer, nonces[1])].into_iter().collect();
    let mut msg = relayed_message(&accounts, message, nonces, FeeDeclaration::new(payer, fee.gas_limit, fee.tip, fee.max_fee))?;
    // The payer adds a nonce, an authorized account and a witness: price the larger transaction.
    let bytes = wire_bytes(&msg, 2)?;
    let scale = |v: u128| v.saturating_mul(u128::from(bytes)).div_ceil(u128::from(measured_bytes.max(1))).max(v);
    // The wallet's max_fee prices base-fee headroom; the relay caps it. Admission needs at least
    // the reserve at head (the gas limit at today's base fees), so clamp to the cap above that.
    let reserve = scale(reserve_at_head);
    if reserve > fee_cap {
        bail!("its fee reserve {reserve} is above the relay's cap {fee_cap}; nothing sent");
    }
    let max_fee = scale(fee.max_fee).min(fee_cap);
    msg.execution.fee = Some(FeeDeclaration::new(payer, fee.gas_limit, fee.tip, max_fee));
    let key = wallet.get_account_public_signing_key(claimer).expect("checked above");
    let hash = msg.hash();
    let witness = WitnessSet::from_raw_parts(vec![(Signature::new(key, &hash), PublicKey::new_from_private_key(key))]);
    Ok(HalfSigned { tx: PublicTransaction::new(msg, witness), gas: fee.gas_limit, max_fee, cycles })
}

/// The relayed version of a call, signed by its sender, NOT measured first: the relay's own caps
/// as gas and max fee. Only for probes of the relay's refusals (`burn --relay --no-precheck`): a
/// call that would revert is refused by the relay's simulation, never sent.
pub async fn build_unmeasured(wallet: &mut WalletCore<SequencerClient>, accounts: Vec<AccountMention>, message: Vec<u8>, payer: AccountId, gas: u64, max_fee: u128) -> Result<HalfSigned> {
    let sender = accounts.first().ok_or_else(|| anyhow!("no accounts"))?.actor().account_id;
    if sender == payer {
        bail!("the relay's payer is the sender itself");
    }
    let nonces = wallet.get_accounts_nonces(&[sender, payer]).await?;
    let nonces: BTreeMap<_, _> = [(sender, nonces[0]), (payer, nonces[1])].into_iter().collect();
    let msg = relayed_message(&accounts, message, nonces, FeeDeclaration::new(payer, gas, 0, max_fee))?;
    let key = wallet.get_account_public_signing_key(sender).ok_or_else(|| anyhow!("{sender} is not a public account of this wallet"))?;
    let hash = msg.hash();
    let witness = WitnessSet::from_raw_parts(vec![(Signature::new(key, &hash), PublicKey::new_from_private_key(key))]);
    Ok(HalfSigned { tx: PublicTransaction::new(msg, witness), gas, max_fee, cycles: 0 })
}

/// The relay's payer and caps (`GET <relay>/info`).
/// One JSON-RPC call to the chain's endpoint; the result, or the error's message.
async fn rpc(url: &url::Url, method: &str, params: Value) -> Result<Value> {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }).to_string();
    let resp = http()?.post(url.clone()).header("content-type", "application/json").body(body).send().await.context("the relay")?;
    let status = resp.status();
    let text = resp.text().await?;
    let v: Value = serde_json::from_str(&text).with_context(|| format!("{method}: HTTP {status}, not JSON: {}", text.chars().take(200).collect::<String>()))?;
    if let Some(e) = v.get("error") {
        bail!("{method}: {} (HTTP {status}); is the relay served at this endpoint?", e["message"].as_str().unwrap_or("error"));
    }
    Ok(v["result"].clone())
}

/// The relay's payer and caps.
pub async fn info(relay: &Relay) -> Result<(AccountId, u64, u128)> {
    let v = match relay {
        Relay::Rpc(u) => rpc(u, "bridge_relayInfo", json!([])).await?,
        Relay::Direct(u) => {
            let text = http()?.get(info_url(u)).send().await.context("the relay")?.text().await?;
            serde_json::from_str(&text).with_context(|| format!("the relay's info is not JSON: {}", text.chars().take(200).collect::<String>()))?
        }
    };
    let payer: AccountId = v["payer"].as_str().ok_or_else(|| anyhow!("the relay names no payer"))?.parse().map_err(|e| anyhow!("relay payer: {e:?}"))?;
    let max_gas = v["max_gas"].as_u64().ok_or_else(|| anyhow!("the relay names no max_gas"))?;
    let max_fee: u128 = v["max_fee"].as_str().unwrap_or_default().parse().context("the relay's max_fee")?;
    Ok((payer, max_gas, max_fee))
}

/// Longest wait on a "busy" answer before the caller rebuilds and sends again.
pub const MAX_BUSY_WAIT_S: u64 = 30;

/// How long to wait before rebuilding, when a refusal says `retry` and carries `retry_after`
/// (the relayer's "busy": its last co-signed claim is not in a block yet).
fn busy_wait(v: &Value) -> Option<std::time::Duration> {
    if v["retry"].as_bool() != Some(true) {
        return None;
    }
    let s = v.get("retry_after")?.as_u64()?;
    Some(std::time::Duration::from_secs(s.clamp(1, MAX_BUSY_WAIT_S)))
}

/// Sends the half-signed claim. `Ok(Ok(hash))`: submitted (`None`: checked only);
/// `Ok(Err((reason, retry)))`: refused. On a "busy" answer it waits `retry_after` (at most
/// [`MAX_BUSY_WAIT_S`]) before returning, so the caller's rebuild uses the payer's new nonce.
pub async fn post(relay: &Relay, tx: &PublicTransaction, check_only: bool, burn: bool) -> Result<std::result::Result<Option<HashType>, (String, bool)>> {
    let tx_hex = hex(&borsh::to_vec(tx)?);
    let v = match relay {
        Relay::Rpc(u) => rpc(u, if burn { "bridge_relayBurn" } else { "bridge_relayClaim" }, json!([tx_hex, check_only])).await?,
        Relay::Direct(u) => {
            let body = json!({ "tx": tx_hex, "check_only": check_only }).to_string();
            let mut u = u.clone();
            if burn {
                let p = format!("{}/burn", u.path().trim_end_matches('/'));
                u.set_path(&p);
            }
            let resp = http()?.post(u).header("content-type", "application/json").body(body).send().await.context("the relay")?;
            let status = resp.status();
            let text = resp.text().await?;
            serde_json::from_str(&text).with_context(|| format!("relay answered {status}, not JSON: {}", text.chars().take(200).collect::<String>()))?
        }
    };
    if v["ok"].as_bool() == Some(true) {
        return Ok(Ok(match v["tx_hash"].as_str() {
            Some(h) => Some(h.parse().map_err(|e| anyhow!("relay tx hash: {e:?}"))?),
            None => None,
        }));
    }
    let reason = v["reason"].as_str().unwrap_or("refused").to_owned();
    if let Some(wait) = busy_wait(&v) {
        println!("relay: {reason}; waiting {} s before rebuilding", wait.as_secs());
        tokio::time::sleep(wait).await;
    }
    Ok(Err((reason, v["retry"].as_bool() == Some(true))))
}

// ------------------------------------------------------------------ relayer side

/// A refusal the relayer reports (`retry`: the claimer may rebuild and send again).
#[derive(Debug, Default)]
pub struct Refusal {
    /// What the client sees: a fixed or designed text, never an internal URL, path or error chain.
    pub reason: String,
    pub retry: bool,
    /// For the relayer's log only (never returned to a client): the internal error behind `reason`.
    pub detail: Option<String>,
    /// The co-signed transaction may have reached the sequencer (its answer was lost): its hash.
    pub uncertain: Option<String>,
}

fn refuse<T>(reason: impl Into<String>) -> std::result::Result<T, Refusal> {
    Err(Refusal { reason: reason.into(), ..Refusal::default() })
}

/// What a client sees when the relayer could not read the chain (or build its RPC client).
pub const INTERNAL_ERROR: &str = "relay error: the chain could not be read; try again";

/// An internal failure: a fixed text for the client, the error chain for the relayer's log.
fn internal(e: anyhow::Error) -> Refusal {
    Refusal { reason: INTERNAL_ERROR.into(), retry: true, detail: Some(format!("{e:#}")), uncertain: None }
}

/// The CLI's pre-check refusals are designed texts that end "nothing sent" (the client's own
/// diagnosis); any other error there is a failed chain read.
fn precheck_refusal(e: anyhow::Error) -> Refusal {
    let text = format!("{e}");
    if text.ends_with("nothing sent") { Refusal { reason: text, ..Refusal::default() } } else { internal(e) }
}

/// The `RELAY ` line's JSON for a refusal: `detail` and `uncertain` are for the relayer only.
fn refusal_json(r: &Refusal) -> Value {
    let mut v = json!({ "ok": false, "reason": r.reason, "retry": r.retry });
    if let Some(d) = &r.detail {
        v["detail"] = json!(d);
    }
    if let Some(h) = &r.uncertain {
        v["uncertain"] = json!(true);
        v["tx_hash"] = json!(h);
    }
    v
}

/// The checks every half-signed transaction the relay pays for must pass, whatever it calls: a
/// strict decode, exactly one signature (the sender's, valid), the signers and nonces exactly the
/// sender and the payer, no promotions or admission evidence, the payer's fee within the caps and
/// no tip, no declared actor of the payer's account. Returns the transaction and the sender.
fn half_signed(bytes: &[u8], payer: AccountId, max_gas: u64, max_fee: u128, who: &str) -> std::result::Result<(PublicTransaction, AccountId), Refusal> {
    if bytes.len() > MAX_TX_BYTES {
        return refuse(format!("transaction is {} bytes, more than {MAX_TX_BYTES}", bytes.len()));
    }
    let Ok(tx) = borsh::from_slice::<PublicTransaction>(bytes) else {
        return refuse("not a public transaction (strict decode failed)");
    };
    let m = tx.message();
    let w = tx.witness_set().signatures_and_public_keys();
    if w.len() != 1 {
        return refuse(format!("expected exactly the {who}'s signature, found {}", w.len()));
    }
    if !tx.witness_set().is_valid_for(m) {
        return refuse(format!("the {who}'s signature does not verify"));
    }
    let sender = AccountId::from(&w[0].1);
    if sender == payer {
        return refuse(format!("the {who} is the relay's payer"));
    }
    let both: BTreeSet<AccountId> = [sender, payer].into_iter().collect();
    if !m.public_context.promotions.is_empty() {
        return refuse("promotions are not allowed");
    }
    if m.public_context.authorized_accounts != both || m.nonces.keys().copied().collect::<BTreeSet<_>>() != both {
        return refuse(format!("the signers must be exactly the {who} and the relay's payer"));
    }
    if !m.admission_evidence.is_empty() {
        return refuse("admission evidence is not allowed");
    }
    let Some(fee) = m.execution.fee else { return refuse("no fee declaration") };
    if fee.payer != payer {
        return refuse("the fee payer is not the relay's payer");
    }
    if fee.tip != 0 {
        return refuse("a tip is not allowed");
    }
    if fee.gas_limit == 0 || fee.gas_limit > max_gas {
        return refuse(format!("declared gas {} is outside 1..={max_gas}", fee.gas_limit));
    }
    if fee.max_fee > max_fee {
        return refuse(format!("max_fee {} is more than {max_fee}", fee.max_fee));
    }
    if m.public_context.actors.iter().any(|a| a.account_id == payer) {
        return refuse("the transaction declares an actor of the relay's payer");
    }
    Ok((tx, sender))
}

/// Strict decode of a half-signed claim; checks that need no chain state.
pub fn decode(bytes: &[u8], bridge: AccountId, payer: AccountId, max_gas: u64, max_fee: u128) -> std::result::Result<(PublicTransaction, AccountId, Instruction), Refusal> {
    let (tx, claimer) = half_signed(bytes, payer, max_gas, max_fee, "claimer")?;
    let m = tx.message();
    if m.execution.root.to != Actor::new(claimer, bridge) {
        return refuse("the transaction does not call the claimer's actor of this bridge");
    }
    let Ok(BridgeMessage::Instruction(ins)) = borsh::from_slice::<BridgeMessage>(&m.execution.root.message) else {
        return refuse("the root message is not a bridge instruction (strict decode failed)");
    };
    match &ins {
        Instruction::Claim { recipient_opening: None, .. } => {}
        Instruction::Claim { .. } => return refuse("a one-time recipient is a private claim; the relay takes public claims only"),
        _ => return refuse("the instruction is not a claim"),
    }
    Ok((tx, claimer, ins))
}

/// A relayed public burn as decoded: the burner, the definition, the amount, the exit commitment.
pub struct Burn {
    pub tx: PublicTransaction,
    pub burner: AccountId,
    pub definition: AccountId,
    pub amount: u128,
    pub exit: [u8; 32],
}

/// Strict decode of a half-signed public burn ("Send back to Ethereum"); checks that need no chain
/// state. The root call is the burner's actor of the token program with exactly
/// `BurnWithNotice { definition, amount, notify: Actor(config PDA of this bridge, this bridge), memo }`,
/// and the declared actors are exactly the burner's token actor, the definition's and the config's.
pub fn decode_burn(bytes: &[u8], bridge: AccountId, token_program: AccountId, payer: AccountId, max_gas: u64, max_fee: u128) -> std::result::Result<Burn, Refusal> {
    let (tx, burner) = half_signed(bytes, payer, max_gas, max_fee, "burner")?;
    let m = tx.message();
    if m.execution.root.to != Actor::new(burner, token_program) {
        return refuse("the transaction does not call the burner's actor of this bridge's token program");
    }
    let Ok(TokenMessage::BurnWithNotice { definition, amount, notify, memo }) = borsh::from_slice::<TokenMessage>(&m.execution.root.message) else {
        return refuse("the root message is not a burn with notice (strict decode failed)");
    };
    if notify != Actor::new(config_address(&bridge), bridge) {
        return refuse("the burn does not notify this bridge's config");
    }
    let expected: BTreeSet<Actor> = [Actor::new(burner, token_program), Actor::new(definition, token_program), Actor::new(config_address(&bridge), bridge)].into_iter().collect();
    if m.public_context.actors != expected {
        return refuse("the declared accounts are not exactly a public burn's");
    }
    Ok(Burn { tx, burner, definition, amount, exit: memo })
}

/// The burn's checks against the chain, then (with `submit`) the payer's signature and the submission.
#[expect(clippy::too_many_arguments, reason = "the relayer's configuration")]
pub async fn cosign_burn(
    wallet: &WalletCore<SequencerClient>,
    sequencer: &str,
    bytes: &[u8],
    bridge: AccountId,
    token_program: AccountId,
    payer: AccountId,
    max_gas: u64,
    max_fee: u128,
    submit: bool,
) -> std::result::Result<Value, Refusal> {
    let b = decode_burn(bytes, bridge, token_program, payer, max_gas, max_fee)?;
    let m = b.tx.message().clone();
    let fee = m.execution.fee.expect("checked");
    let config: Config = borsh::from_slice(&crate::public_state(wallet, config_address(&bridge), bridge).await.map_err(internal)?)
        .map_err(|_| Refusal { reason: "the bridge has no config".into(), ..Refusal::default() })?;
    if config.token_program != token_program {
        return refuse("the relay's token program is not the bridge's");
    }
    if [config.admin, config.freezer, config.attestor].contains(&payer) || config.pending_admin == Some(payer) {
        return refuse("relay misconfigured: its payer holds a bridge role; use a dedicated payer account");
    }
    let Some(token) = config.tokens.iter().find(|t| t.definition == b.definition) else { return refuse("this token is not registered on the bridge") };
    if !token.denoms.contains(&b.amount) {
        return refuse("a burn is exactly one denomination; nothing sent");
    }
    if config.burn_frozen {
        return refuse("burns are frozen on this bridge; nothing sent");
    }
    if token.burn_frozen {
        return refuse("burns of this token are frozen on this bridge; nothing sent");
    }
    let chain = wallet.get_accounts_nonces(&[b.burner, payer]).await.map_err(internal)?;
    if m.nonces.get(&b.burner) != Some(&chain[0]) || m.nonces.get(&payer) != Some(&chain[1]) {
        return Err(Refusal { reason: "stale nonce: rebuild the burn and send it again".into(), retry: true, ..Refusal::default() });
    }
    let mut states = HashMap::new();
    let mut admitted: BTreeSet<AccountId> = [b.burner, payer].into_iter().collect();
    for actor in &m.public_context.actors {
        let view = wallet.get_account_view(*actor).await.map_err(internal)?;
        if view.is_some() {
            admitted.insert(actor.account_id);
        }
        states.insert(*actor, view.map_or_else(ActorState::empty, |a| a.data.actor_state(actor.program_account_id).clone()));
    }
    let programs = crate::catalog(wallet, &[token_program, bridge]).await.map_err(internal)?;
    let simulation = Simulation { public_actor_states: states, admitted_accounts: Some(admitted) };
    let (cycles, result) = lee::simulate_public_transaction(&b.tx, &simulation, &programs, fee.gas_limit);
    if let Err(e) = result {
        return refuse(format!("the burn does not apply in simulation: {:#}", anyhow::Error::from(e)));
    }
    let payer_balance = crate::public_state(wallet, payer, lee_core::native_token::NATIVE_TOKEN_PROGRAM_ID)
        .await
        .map_err(internal)
        .and_then(|s| lee_core::native_token::decode_balance(&s).map_err(|e| internal(anyhow!("{e}"))))?;
    if payer_balance < fee.max_fee {
        return refuse(format!("the relay's payer holds {payer_balance}, less than this burn's max_fee {}", fee.max_fee));
    }
    let mut summary = json!({
        "kind": "burn",
        "burner": b.burner.to_string(),
        "exit_commitment": format!("0x{}", hex(&b.exit)),
        "eth_token": format!("0x{}", hex(&token.eth_token)),
        "denom": b.amount.to_string(),
        "gas_limit": fee.gas_limit,
        "max_fee": fee.max_fee.to_string(),
        "cycles": cycles,
        "payer_balance": payer_balance.to_string(),
        "burner_nonce": chain[0].0.to_string(),
        "payer_nonce": chain[1].0.to_string(),
        "bytes": bytes.len(),
    });
    if !submit {
        return Ok(summary);
    }
    summary["tx_hash"] = json!(sign_and_send(wallet, sequencer, &b.tx, payer).await?);
    Ok(summary)
}

/// The payer's signature added to a checked half-signed transaction, then the submission.
async fn sign_and_send(wallet: &WalletCore<SequencerClient>, sequencer: &str, tx: &PublicTransaction, payer: AccountId) -> std::result::Result<String, Refusal> {
    let Some(key) = wallet.get_account_public_signing_key(payer) else {
        return Err(Refusal { reason: "relay misconfigured: the payer's key is not in its wallet".into(), ..Refusal::default() });
    };
    let m = tx.message().clone();
    let hash = m.hash();
    let mut sigs = tx.witness_set().signatures_and_public_keys().to_vec();
    sigs.push((Signature::new(key, &hash), PublicKey::new_from_private_key(key)));
    let full = PublicTransaction::new(m, WitnessSet::from_raw_parts(sigs));
    if full.check_stateless().is_err() {
        return Err(Refusal { reason: "relay error: the co-signed transaction fails the stateless check".into(), ..Refusal::default() });
    }
    let client = sequencer_service_rpc::SequencerClientBuilder::default().build(sequencer).map_err(|e| internal(anyhow!("{e}")))?;
    let full = LeeTransaction::Public(full);
    let local_hash = full.hash();
    match sequencer_service_rpc::RpcClient::send_transaction(&client, full).await {
        Ok(hash) => Ok(hash.to_string()),
        Err(e) => Err(send_refusal(e, &local_hash)),
    }
}

/// The full check, then (with `submit`) the payer's signature and the submission.
#[expect(clippy::too_many_arguments, reason = "the relayer's configuration")]
pub async fn cosign(
    wallet: &WalletCore<SequencerClient>,
    sequencer: &str,
    bytes: &[u8],
    bridge: AccountId,
    token_program: AccountId,
    payer: AccountId,
    max_gas: u64,
    max_fee: u128,
    submit: bool,
) -> std::result::Result<Value, Refusal> {
    let (tx, claimer, ins) = decode(bytes, bridge, payer, max_gas, max_fee)?;
    let Instruction::Claim { eth_token, denom_index, denom, root, leaf_index, path, secret, cfg_id, .. } = ins else { unreachable!() };
    let m = tx.message().clone();
    let fee = m.execution.fee.expect("checked");

    let config: Config = borsh::from_slice(&crate::public_state(wallet, config_address(&bridge), bridge).await.map_err(internal)?)
        .map_err(|_| Refusal { reason: "the bridge has no config".into(), ..Refusal::default() })?;
    if config.root_source != RootSource::StandIn {
        return refuse("this bridge takes roots from an RFP-021 attestation; the relay takes stand-in claims only");
    }
    if config.token_program != token_program {
        return refuse("the relay's token program is not the bridge's");
    }
    if [config.admin, config.freezer, config.attestor].contains(&payer) || config.pending_admin == Some(payer) {
        return refuse("relay misconfigured: its payer holds a bridge role; use a dedicated payer account");
    }
    let Some(token) = config.tokens.iter().find(|t| t.eth_token == eth_token) else { return refuse("token not registered") };
    let nullifier = bc::claim_nullifier(&secret, &claimer, &eth_token, denom);
    let expected: BTreeSet<Actor> = [
        Actor::new(claimer, bridge),
        Actor::new(bc::root_address(&bridge, &root), bridge),
        Actor::new(bc::nullifier_address(&bridge, &nullifier), bridge),
        Actor::new(config_address(&bridge), bridge),
        Actor::new(token.definition, token_program),
        Actor::new(claimer, token_program),
    ]
    .into_iter()
    .collect();
    if m.public_context.actors != expected {
        return refuse("the declared accounts are not exactly a public claim's");
    }

    // Nonces: both must be the chain's current ones (a stale payer nonce means rebuild).
    let chain = wallet.get_accounts_nonces(&[claimer, payer]).await.map_err(internal)?;
    if m.nonces.get(&claimer) != Some(&chain[0]) || m.nonces.get(&payer) != Some(&chain[1]) {
        return Err(Refusal { reason: "stale nonce: rebuild the claim and send it again".into(), retry: true, ..Refusal::default() });
    }

    // The CLI's own pre-checks (the same words a claimer sees).
    let plan = crate::ClaimPlan {
        from: claimer,
        eth_token,
        denom,
        secret,
        cfg_id,
        root,
        path: path.clone(),
        nullifier,
        definition: token.definition,
        config: config.clone(),
        opening: None,
        sponsored: false,
        leaf_index: usize::try_from(leaf_index).unwrap_or(usize::MAX),
    };
    if let Err(e) = crate::diagnose_claim(wallet, bridge, &plan, leaf_index, denom_index, denom, None).await {
        return Err(precheck_refusal(e));
    }
    let commitment = bc::deposit_commitment(&secret, &claimer, &eth_token, denom, &cfg_id);

    // Simulation, as the wallet measures a public send: every declared actor's state, the
    // accounts settlement admits (signers and accounts already on chain), the programs deployed.
    let mut states = HashMap::new();
    let mut admitted: BTreeSet<AccountId> = [claimer, payer].into_iter().collect();
    for actor in &m.public_context.actors {
        let view = wallet.get_account_view(*actor).await.map_err(internal)?;
        if view.is_some() {
            admitted.insert(actor.account_id);
        }
        let state = view.map_or_else(ActorState::empty, |a| a.data.actor_state(actor.program_account_id).clone());
        states.insert(*actor, state);
    }
    let programs = crate::catalog(wallet, &[bridge, token_program]).await.map_err(internal)?;
    let simulation = Simulation { public_actor_states: states, admitted_accounts: Some(admitted) };
    let (cycles, result) = lee::simulate_public_transaction(&tx, &simulation, &programs, fee.gas_limit);
    if let Err(e) = result {
        return refuse(format!("the claim does not apply in simulation: {:#}", anyhow::Error::from(e)));
    }
    let payer_balance = crate::public_state(wallet, payer, lee_core::native_token::NATIVE_TOKEN_PROGRAM_ID)
        .await
        .map_err(internal)
        .and_then(|s| lee_core::native_token::decode_balance(&s).map_err(|e| internal(anyhow!("{e}"))))?;
    if payer_balance < fee.max_fee {
        return refuse(format!("the relay's payer holds {payer_balance}, less than this claim's max_fee {}", fee.max_fee));
    }
    let mut summary = json!({
        "claimer": claimer.to_string(),
        "nullifier": format!("0x{}", hex(&nullifier)),
        "commitment": format!("0x{}", hex(&commitment)),
        "leaf_index": leaf_index,
        "eth_token": format!("0x{}", hex(&eth_token)),
        "denom": denom.to_string(),
        "gas_limit": fee.gas_limit,
        "max_fee": fee.max_fee.to_string(),
        "cycles": cycles,
        "payer_balance": payer_balance.to_string(),
        // The nonces this transaction carries (equal to the chain's, checked above): the relayer
        // settles its submission from them (in a block, or never: a nonce moved past it).
        "claimer_nonce": chain[0].0.to_string(),
        "payer_nonce": chain[1].0.to_string(),
        "bytes": bytes.len(),
    });
    if !submit {
        return Ok(summary);
    }
    let Some(key) = wallet.get_account_public_signing_key(payer) else {
        return Err(Refusal { reason: "relay misconfigured: the payer's key is not in its wallet".into(), ..Refusal::default() });
    };
    let hash = m.hash();
    let mut sigs = tx.witness_set().signatures_and_public_keys().to_vec();
    sigs.push((Signature::new(key, &hash), PublicKey::new_from_private_key(key)));
    let full = PublicTransaction::new(m, WitnessSet::from_raw_parts(sigs));
    if full.check_stateless().is_err() {
        return Err(Refusal { reason: "relay error: the co-signed transaction fails the stateless check".into(), ..Refusal::default() });
    }
    let client = sequencer_service_rpc::SequencerClientBuilder::default().build(sequencer).map_err(|e| internal(anyhow!("{e}")))?;
    let full = LeeTransaction::Public(full);
    let local_hash = full.hash();
    let tx_hash = match sequencer_service_rpc::RpcClient::send_transaction(&client, full).await {
        Ok(hash) => hash,
        Err(e) => return Err(send_refusal(e, &local_hash)),
    };
    summary["tx_hash"] = json!(tx_hash.to_string());
    Ok(summary)
}

/// A failed `sendTransaction`. A JSON-RPC error answer means the sequencer did not take it
/// (nothing charged; a full mempool may be retried). Anything else (transport error, timeout, an
/// unreadable answer) may come after the sequencer took it: `uncertain`, with the hash, so the
/// relayer settles it from the chain instead of treating it as never sent.
fn send_refusal(e: sequencer_service_rpc::ClientError, local_hash: &HashType) -> Refusal {
    match e {
        sequencer_service_rpc::ClientError::Call(err) => {
            let full = err.code() == sequencer_service_rpc::MEMPOOL_IS_FULL_ERROR_CODE;
            Refusal {
                reason: if full { "the chain's mempool is full; try again shortly" } else { "the sequencer refused the co-signed transaction; nothing was charged" }
                    .into(),
                retry: full,
                detail: Some(format!("code {}: {}", err.code(), err.message())),
                uncertain: None,
            }
        }
        other => Refusal {
            reason: "relay error: the sequencer's answer was lost; the operator checks it".into(),
            retry: false,
            detail: Some(format!("{other}")),
            uncertain: Some(local_hash.to_string()),
        },
    }
}

/// `relay-cosign`: one JSON line on stdout, prefixed `RELAY `, for the relayer service.
#[expect(clippy::too_many_arguments, reason = "the relayer's configuration")]
pub async fn run_cosign(
    wallet: &WalletCore<SequencerClient>,
    sequencer: &str,
    tx_file: &std::path::Path,
    bridge: AccountId,
    token_program: AccountId,
    payer: AccountId,
    max_gas: u64,
    max_fee: u128,
    submit: bool,
    burn: bool,
) -> Result<()> {
    let text = std::fs::read_to_string(tx_file).with_context(|| format!("reading {}", tx_file.display()))?;
    let out = match unhex(&text) {
        Err(_) => json!({ "ok": false, "reason": "the transaction is not hex", "retry": false }),
        Ok(bytes) => match if burn {
            cosign_burn(wallet, sequencer, &bytes, bridge, token_program, payer, max_gas, max_fee, submit).await
        } else {
            cosign(wallet, sequencer, &bytes, bridge, token_program, payer, max_gas, max_fee, submit).await
        } {
            Ok(mut v) => {
                v["ok"] = json!(true);
                v
            }
            Err(r) => refusal_json(&r),
        },
    };
    println!("RELAY {out}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEMO: &str = "https://lez.nameswallet.app/";

    #[test]
    fn relay_is_the_chain_endpoint_or_a_loopback_relay() {
        assert_eq!(allowed_relay("auto", DEMO).unwrap(), Relay::Rpc(DEMO.parse().unwrap()));
        assert_eq!(allowed_relay("", DEMO).unwrap(), Relay::Rpc(DEMO.parse().unwrap()));
        assert!(matches!(allowed_relay("http://127.0.0.1:3297/relay", DEMO).unwrap(), Relay::Direct(_)));
        for bad in [
            "https://lez.nameswallet.app/",
            "https://lez.nameswallet.app/relay",
            "http://127.0.0.1:3297/",
            "https://127.0.0.1:3297/relay",
            "https://evil.example/relay",
            "http://lez.nameswallet.app.evil.example/relay",
        ] {
            assert!(allowed_relay(bad, DEMO).is_err(), "{bad}");
        }
        assert_eq!(info_url(&"https://lez.nameswallet.app/relay".parse().unwrap()).as_str(), "https://lez.nameswallet.app/relay/info");
    }

    #[test]
    fn garbage_and_oversized_input_is_refused_before_any_read() {
        let b = AccountId::new([1; 32]);
        let p = AccountId::new([2; 32]);
        assert!(decode(&[0u8; 10], b, p, 1, 1).is_err());
        assert!(decode(&vec![0u8; MAX_TX_BYTES + 1], b, p, 1, 1).unwrap_err().reason.contains("bytes"));
        assert_eq!(unhex("0x0aff").unwrap(), vec![10, 255]);
        assert!(unhex("0xzz").is_err());
    }

    fn keyed(seed: u8) -> (lee::PrivateKey, AccountId) {
        let k = lee::PrivateKey::try_new([seed; 32]).unwrap();
        let id = AccountId::from(&PublicKey::new_from_private_key(&k));
        (k, id)
    }

    fn claim_message(claimer: AccountId, payer: AccountId, bridge: AccountId, opening: Option<[u8; 32]>, extra_actor: Option<Actor>, gas: u64) -> Message {
        let ins = BridgeMessage::Instruction(Instruction::Claim {
            eth_token: [0; 20],
            denom_index: 0,
            denom: 1,
            root: [0; 32],
            leaf_index: 0,
            path: vec![],
            secret: [7; 32],
            cfg_id: [0; 32],
            recipient_opening: opening,
        });
        let mut actors = vec![Actor::new(claimer, bridge)];
        actors.extend(extra_actor);
        let nonces = [(claimer, Default::default()), (payer, Default::default())].into_iter().collect();
        Message::new(Actor::new(claimer, bridge), borsh::to_vec(&ins).unwrap(), actors, nonces, Some(FeeDeclaration::new(payer, gas, 0, 1000)), Vec::new())
    }

    fn signed(m: Message, keys: &[&lee::PrivateKey]) -> Vec<u8> {
        borsh::to_vec(&PublicTransaction::new(m.clone(), WitnessSet::for_message(&m, keys))).unwrap()
    }

    const VECTOR: &str = include_str!("../../../modules/lez_bridge/rust-lib/testdata/relayed-claim-v1.json");

    /// The Names Wallet path and `claim --relay` give the SAME transaction. The vector
    /// (`modules/tools/make-relayed-claim-vector.py` (not published)) holds the call JSON the `lez_bridge` core
    /// module hands the wallet's `propose_relayed_call` (the module pins its own build to it).
    /// Here the claim's message and accounts are built the way `claim --relay` builds them
    /// (`Command::Claim`, `claim_mentions`, [`relayed_message`]), and the call JSON is turned
    /// into a message the way the names CLI does (`public_call.rs`, `CallInput::build` and
    /// `Built::message`: the signer's actor of the program as root, then each declared entry's
    /// actor in order without repeats, admission evidence only for entries marked `new`). Same
    /// nonces and fee: the hashes, which both signatures cover, must be equal. The relay's own
    /// decoder must take it too.
    #[test]
    fn the_wallet_s_build_of_the_module_s_call_is_claim_relay_s_transaction() {
        use lee_core::program::PdaSeed;
        let v: Value = serde_json::from_str(VECTOR).unwrap();
        let (i, e) = (&v["inputs"], &v["expected"]);
        let id = |x: &Value| -> AccountId { x.as_str().unwrap().trim_start_matches("Public/").parse().unwrap() };
        let h32 = |x: &Value| -> [u8; 32] { unhex(x.as_str().unwrap()).unwrap().try_into().unwrap() };
        let (bridge, token_program, definition, claimer, payer) =
            (id(&i["bridge_program"]), id(&i["token_program"]), id(&i["definition"]), id(&i["claimer"]), id(&i["payer"]));
        let (cfg, k) = (h32(&i["cfg_id"]), h32(&i["claim_key"]));
        let eth_token: [u8; 20] = unhex(i["eth_token"].as_str().unwrap()).unwrap().try_into().unwrap();
        let denom: u128 = i["denom"].as_str().unwrap().parse().unwrap();
        let index = i["leaf_index"].as_u64().unwrap();
        let leaves: Vec<[u8; 32]> = i["leaves"].as_array().unwrap().iter().map(h32).collect();

        // `claim --relay`'s side.
        let c = bc::deposit_commitment(&k, &claimer, &eth_token, denom, &cfg);
        let leaf = bc::deposit_leaf(&cfg, &eth_token, denom, &c);
        let path = bc::merkle_path(&leaves, usize::try_from(index).unwrap());
        let root = bc::merkle_root(&leaf, index, &path);
        let nullifier = bc::claim_nullifier(&k, &claimer, &eth_token, denom);
        let message = borsh::to_vec(&BridgeMessage::Instruction(Instruction::Claim {
            eth_token,
            denom_index: u32::try_from(i["denom_index"].as_u64().unwrap()).unwrap(),
            denom,
            root,
            leaf_index: index,
            path,
            secret: k,
            cfg_id: cfg,
            recipient_opening: None,
        }))
        .unwrap();
        let accounts = crate::claim_mentions(
            &wallet::AccountIdentity::Public(claimer),
            None,
            false,
            &nullifier,
            definition,
            bridge,
            token_program,
            crate::pda(bridge, bc::root_seed(&root)),
        );
        let nonces: BTreeMap<_, _> = [(claimer, Default::default()), (payer, Default::default())].into_iter().collect();
        let gas = i["gas_limit"].as_u64().unwrap();
        let max_fee: u128 = i["max_fee"].as_str().unwrap().parse().unwrap();
        let cli = relayed_message(&accounts, message.clone(), nonces.clone(), FeeDeclaration::new(payer, gas, 0, max_fee)).unwrap();

        // The wallet's side, from the module's call JSON alone.
        let call = &e["call"];
        assert_eq!(unhex(call["message"].as_str().unwrap()).unwrap(), message, "the module's message bytes are claim's");
        let program = id(&call["program"]);
        let root_actor = Actor::new(claimer, program);
        let mut actors = vec![root_actor];
        for entry in call["accounts"].as_array().unwrap() {
            assert_ne!(entry["new"], true, "no admission evidence: the relay refuses it");
            let actor_program = entry.get("program").map_or(program, id);
            let account = match entry.get("seed") {
                Some(seed) => {
                    assert!(entry.get("account").is_none(), "the wallet's app-facing parser takes a seed or an address, not both");
                    AccountId::for_public_pda(&id(&entry["of"]), &PdaSeed::new(h32(seed)))
                }
                None => id(&entry["account"]),
            };
            let actor = Actor::new(account, actor_program);
            if !actors.contains(&actor) {
                actors.push(actor);
            }
        }
        let fee = &call["fee"];
        let wallet_fee = FeeDeclaration::new(
            id(&fee["payer"]),
            fee["gas_limit"].as_u64().unwrap(),
            fee["tip"].as_str().unwrap().parse().unwrap(),
            fee["max_fee"].as_str().unwrap().parse().unwrap(),
        );
        let built = Message::new(root_actor, unhex(call["message"].as_str().unwrap()).unwrap(), actors, nonces, Some(wallet_fee), Vec::new());
        assert_eq!(built.hash(), cli.hash(), "the wallet signs claim --relay's transaction");
        assert_eq!(borsh::to_vec(&built).unwrap(), borsh::to_vec(&cli).unwrap());
    }

    #[test]
    fn the_decoder_takes_only_a_half_signed_public_claim_paid_by_the_relay() {
        let (ck, c) = keyed(3);
        let (pk, p) = keyed(4);
        let b = AccountId::new([9; 32]);
        let ok = signed(claim_message(c, p, b, None, None, 100), &[&ck]);
        assert!(decode(&ok, b, p, 100, 1000).is_ok());
        let why = |bytes: &[u8]| decode(bytes, b, p, 100, 1000).unwrap_err().reason;
        assert!(why(&signed(claim_message(c, p, b, None, None, 101), &[&ck])).contains("declared gas"));
        assert!(decode(&ok, b, p, 100, 999).unwrap_err().reason.contains("max_fee"));
        assert!(why(&signed(claim_message(c, p, b, Some([1; 32]), None, 100), &[&ck])).contains("private claim"));
        assert!(why(&signed(claim_message(c, p, b, None, Some(Actor::new(p, AccountId::new([0; 32]))), 100), &[&ck])).contains("payer"));
        assert!(why(&signed(claim_message(c, p, b, None, None, 100), &[&ck, &pk])).contains("exactly the claimer"));
        // Signed by someone other than the claimer named in the message: the signer set differs.
        let (sk, _) = keyed(5);
        assert!(why(&signed(claim_message(c, p, b, None, None, 100), &[&sk])).contains("signers"));
        // Trailing bytes after a valid transaction.
        let mut long = ok.clone();
        long.push(0);
        assert!(why(&long).contains("strict decode"));
        // Another bridge.
        assert!(decode(&ok, AccountId::new([8; 32]), p, 100, 1000).is_err());
    }

    #[test]
    fn a_busy_answer_waits_a_capped_time_and_other_refusals_do_not() {
        let secs = |v: Value| busy_wait(&v).map(|d| d.as_secs());
        assert_eq!(secs(json!({ "ok": false, "reason": "busy", "retry": true, "retry_after": 15 })), Some(15));
        assert_eq!(secs(json!({ "ok": false, "reason": "busy", "retry": true, "retry_after": 3600 })), Some(MAX_BUSY_WAIT_S));
        assert_eq!(secs(json!({ "ok": false, "reason": "busy", "retry": true, "retry_after": 0 })), Some(1));
        assert_eq!(secs(json!({ "ok": false, "reason": "stale nonce", "retry": true })), None);
        assert_eq!(secs(json!({ "ok": false, "reason": "no", "retry": false, "retry_after": 15 })), None);
        assert_eq!(secs(json!({ "ok": false, "reason": "busy", "retry": true, "retry_after": "15" })), None);
    }

    #[test]
    fn clients_see_fixed_texts_and_the_details_stay_in_the_relayers_log() {
        let r = internal(anyhow!("error sending request for url (http://127.0.0.1:3290/): connection refused"));
        assert_eq!(r.reason, INTERNAL_ERROR);
        assert!(r.retry);
        assert!(r.detail.as_deref().unwrap().contains("127.0.0.1"));
        let v = refusal_json(&r);
        assert!(!v["reason"].as_str().unwrap().contains("http"));
        assert!(v.get("uncertain").is_none());
        // The pre-check's own refusals pass as they are; a failed read inside it does not.
        let designed = precheck_refusal(anyhow!("deposit already claimed: its nullifier is spent; nothing sent"));
        assert_eq!((designed.reason.as_str(), designed.retry, designed.detail.is_none()), ("deposit already claimed: its nullifier is spent; nothing sent", false, true));
        let read = precheck_refusal(anyhow!("error sending request for url (http://127.0.0.1:3290/)"));
        assert_eq!(read.reason, INTERNAL_ERROR);
    }

    #[test]
    fn a_lost_send_answer_is_uncertain_with_its_hash_and_a_refusal_is_not() {
        let h = HashType([0xab; 32]);
        let refused = send_refusal(
            sequencer_service_rpc::ClientError::Call(sequencer_service_rpc::ErrorObjectOwned::owned(-32000, "fee too low", None::<()>)),
            &h,
        );
        assert!(refused.uncertain.is_none() && !refused.retry);
        assert!(!refused.reason.contains("fee too low") && refused.detail.as_deref().unwrap().contains("fee too low"));
        let full = send_refusal(
            sequencer_service_rpc::ClientError::Call(sequencer_service_rpc::ErrorObjectOwned::owned(
                sequencer_service_rpc::MEMPOOL_IS_FULL_ERROR_CODE,
                "mempool is full",
                None::<()>,
            )),
            &h,
        );
        assert!(full.retry && full.uncertain.is_none());
        let lost = send_refusal(sequencer_service_rpc::ClientError::RequestTimeout, &h);
        assert_eq!(lost.uncertain.as_deref(), Some("ab".repeat(32).as_str()));
        let v = refusal_json(&lost);
        assert_eq!((v["uncertain"].as_bool(), v["retry"].as_bool()), (Some(true), Some(false)));
        assert_eq!(v["tx_hash"].as_str(), Some("ab".repeat(32).as_str()));
    }

    const BURN_VECTOR: &str = include_str!("../../../modules/lez_bridge/rust-lib/testdata/public-burn-v1.json");

    /// The burn the `lez_bridge` module hands the Names Wallet (`send_back_build`, the vector's
    /// call JSON), built into a message the way the wallet builds a relayed call and signed by a
    /// burner, is a burn the relay's decoder takes, with the vector's exit; changed shapes are not.
    #[test]
    fn the_wallet_s_build_of_the_module_s_burn_is_a_burn_the_relay_takes() {
        use lee_core::program::PdaSeed;
        let v: Value = serde_json::from_str(BURN_VECTOR).unwrap();
        let (i, e) = (&v["inputs"], &v["expected"]);
        let id = |x: &Value| -> AccountId { x.as_str().unwrap().trim_start_matches("Public/").parse().unwrap() };
        let h32 = |x: &Value| -> [u8; 32] { unhex(x.as_str().unwrap()).unwrap().try_into().unwrap() };
        let (bridge, tp, payer) = (id(&i["bridge_program"]), id(&i["token_program"]), id(&i["payer"]));
        let call = &e["call"];
        let message = unhex(call["message"].as_str().unwrap()).unwrap();
        let definition = id(&i["definition"]);
        let denom: u128 = i["denom"].as_str().unwrap().parse().unwrap();
        let exit = h32(&e["exit_commitment"]);
        assert_eq!(
            borsh::to_vec(&TokenMessage::BurnWithNotice { definition, amount: denom, notify: Actor::new(config_address(&bridge), bridge), memo: exit }).unwrap(),
            message,
            "the module's message is the token program's BurnWithNotice"
        );
        assert_eq!(config_address(&bridge).to_string(), e["config_pda"].as_str().unwrap());
        let (bk, burner) = keyed(6);
        let build = |msg: Vec<u8>, entries: &[Value], root_program: AccountId| -> Message {
            let root = Actor::new(burner, root_program);
            let mut actors = vec![root];
            for entry in entries {
                let p = entry.get("program").map_or(root_program, id);
                let account = match entry.get("seed") {
                    Some(seed) => AccountId::for_public_pda(&id(&entry["of"]), &PdaSeed::new(h32(seed))),
                    None => id(&entry["account"]),
                };
                let a = Actor::new(account, p);
                if !actors.contains(&a) {
                    actors.push(a);
                }
            }
            let fee = &call["fee"];
            let nonces = [(burner, Default::default()), (payer, Default::default())].into_iter().collect();
            Message::new(root, msg, actors, nonces, Some(FeeDeclaration::new(payer, fee["gas_limit"].as_u64().unwrap(), 0, fee["max_fee"].as_str().unwrap().parse().unwrap())), Vec::new())
        };
        let entries = call["accounts"].as_array().unwrap().clone();
        let ok = signed(build(message.clone(), &entries, id(&call["program"])), &[&bk]);
        let b = decode_burn(&ok, bridge, tp, payer, 250_000, 2_500_000).unwrap_or_else(|r| panic!("{}", r.reason));
        assert_eq!((b.burner, b.definition, b.amount, b.exit), (burner, definition, denom, exit));
        let why = |bytes: &[u8]| decode_burn(bytes, bridge, tp, payer, 250_000, 2_500_000).err().map(|r| r.reason).unwrap_or_default();
        // To another bridge's config.
        let other = AccountId::new([5; 32]);
        let m2 = borsh::to_vec(&TokenMessage::BurnWithNotice { definition, amount: denom, notify: Actor::new(config_address(&other), other), memo: exit }).unwrap();
        assert!(why(&signed(build(m2, &entries, tp), &[&bk])).contains("notify"));
        // A plain transfer, an extra actor, the bridge as root, a second signature, the payer's caps.
        let m3 = borsh::to_vec(&TokenMessage::Transfer { definition, to: other, amount: denom }).unwrap();
        assert!(why(&signed(build(m3, &entries, tp), &[&bk])).contains("burn with notice"));
        let mut more = entries.clone();
        more.push(json!({ "account": other.to_string(), "program": tp.to_string() }));
        assert!(why(&signed(build(message.clone(), &more, tp), &[&bk])).contains("exactly a public burn"));
        assert!(why(&signed(build(message.clone(), &entries, bridge), &[&bk])).contains("token program"));
        let (pk, _) = keyed(7);
        assert!(why(&signed(build(message.clone(), &entries, tp), &[&bk, &pk])).contains("exactly"));
        assert!(decode_burn(&ok, bridge, tp, payer, 1000, 2_500_000).is_err());
        assert!(decode_burn(&ok, bridge, tp, payer, 250_000, 10).is_err());
        // A claim is not a burn, and a burn is not a claim.
        assert!(decode(&ok, bridge, payer, 250_000, 2_500_000).is_err());
        // Trailing bytes, a tip, an actor of the payer, a missing actor.
        let mut long = ok.clone();
        long.push(0);
        assert!(why(&long).contains("strict decode"));
        let mut tipped = build(message.clone(), &entries, tp);
        tipped.execution.fee = Some(FeeDeclaration::new(payer, 120_000, 1, 2_500_000));
        assert!(why(&signed(tipped, &[&bk])).contains("tip"));
        let mut with_payer = entries.clone();
        with_payer.push(json!({ "account": payer.to_string(), "program": "11111111111111111111111111111111" }));
        assert!(why(&signed(build(message.clone(), &with_payer, tp), &[&bk])).contains("payer"));
        assert!(why(&signed(build(message.clone(), &entries[..1], tp), &[&bk])).contains("exactly a public burn"));
    }

}
