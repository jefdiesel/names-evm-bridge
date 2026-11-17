//! What G2 reads from a transaction: the part of the bridge config it needs (the view), and the
//! claims, burns, stand-in roots and nullifier spends of one batch.
//!
//! # Where the bridge's state changes can come from
//!
//! - A **public transaction**: its single root call. Every bridge instruction is a root call to
//!   the signer's own actor of the bridge program (`instruct` asserts `from.is_none()`), and a
//!   burn is a root call `BurnWithNotice { notify: config }` to the token program
//!   (`require_root_signer`). Effects to the config come only from the bridge's own actors
//!   (`from_own_program`), and burn notices only from the token program, so nothing deeper in a
//!   public transaction can change the counters without one of these two root calls.
//! - A **private transaction**: its `public_root` if it has one (treated as above), and every
//!   `PrivateToPublic` boundary delivery to an actor of the bridge program: `Effect::Claim` /
//!   `ClaimAttested` / `Init` / `Admin` / `AttestRoot` from the bridge's own (private) actor,
//!   `SpendClaim` to a nullifier PDA, or the token program's `BurnNotice` to the config.
//!
//! A public root call is re-read here: G2 decodes the instruction and applies what the bridge
//! program does with it. That rests on the sequencer having run the program honestly, as all of
//! LEZ 0.4's public execution does (it is never proven). A private boundary message is what the
//! proven private execution emitted (its receipt is verified before it is read here).
//!
//! # What is tracked
//!
//! From the bridge's `Init`/`InitAttested` (the derived cfg id, the token program, the root
//! source) and `RegisterToken`/`AddDenomination`: the map from wrapped definitions to Ethereum
//! tokens and denominations, which turns a burn (definition, amount) into the vault's (token,
//! denomIndex). Per token, the bridge's own counters (`claims`, `burns`, `claimed_value`,
//! `burned_value`), so a burn above claims is caught as the bridge catches it.
//!
//! Not tracked (liveness only, or caught on Ethereum): freezes, delisting, admin and attestor
//! changes, the sponsor float. A non-reverted transaction that a freeze should have stopped is
//! not detected here; it is the sequencer's fault like any invalid public execution, and the
//! vault's bound (burns <= claims <= deposits by value) still holds.

use crate::G2Error;
use borsh::{BorshDeserialize, BorshSerialize};
use bridge_core::{AdminOp, Effect, Instruction, Message, RootSource};
use bridge_token_core::{BurnNotice, Message as TokenMessage};
use lee_core::{
    ProvenExecution, RootCall,
    account::{AccountId, Actor},
    execution_state::BoundaryStep,
};

pub type EthAddress = [u8; 20];

/// One registered token, as the bridge config holds it (the parts G2 needs).
#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct TokenView {
    pub eth_token: EthAddress,
    /// The wrapped token's definition account in the token program.
    pub definition: [u8; 32],
    pub denoms: Vec<u128>,
    pub claims: Vec<u64>,
    pub burns: Vec<u64>,
    pub claimed_value: u128,
    pub burned_value: u128,
}

/// The bridge config, as far as G2 needs it. Committed in the G2 state by its hash.
#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct BridgeView {
    pub initialized: bool,
    /// The cfg id the bridge derived at init (`bridge_core::cfg_id` with its own program id).
    pub cfg_id: [u8; 32],
    pub token_program: [u8; 32],
    /// Root source: true for the stand-in attestor, false for an RFP-021 instance.
    pub stand_in: bool,
    pub tokens: Vec<TokenView>,
}

impl BridgeView {
    #[must_use]
    pub fn hash(&self) -> [u8; 32] {
        crate::sha256(&[b"RFP022/g2-view/v1", &borsh::to_vec(self).unwrap_or_default()])
    }
}

/// A claim count for the vault (`BridgeVault.ClaimDelta`).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ClaimDelta {
    pub token: EthAddress,
    pub denom_index: u32,
    pub count: u64,
}

/// An exit for the vault (`BridgeVault.Exit`).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Exit {
    pub token: EthAddress,
    pub denom_index: u32,
    pub commitment: [u8; 32],
}

/// What one batch adds, in LEZ order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BatchOut {
    /// Stand-in roots recorded by `AttestRoot` (deduplicated). Every stand-in claim asserts a
    /// recorded root PDA, so these cover every root a claim could have used; the vault checks
    /// each is one of its deposit roots. Empty on the RFP-021 path (see `journal`).
    pub roots: Vec<[u8; 32]>,
    /// Per (token, denomIndex), in order of first appearance.
    pub claims: Vec<ClaimDelta>,
    pub exits: Vec<Exit>,
    /// Claim-nullifier PDA ids spent, in LEZ order (the accumulator keys).
    pub nullifiers: Vec<[u8; 32]>,
}

/// Where a transaction sits, for errors.
#[derive(Clone, Copy, Debug)]
pub struct At {
    pub block_id: u64,
    pub tx: usize,
}

impl At {
    fn bad(self, what: &'static str) -> G2Error {
        G2Error::Inconsistent { block_id: self.block_id, tx: self.tx, what }
    }
}

/// Applies the bridge's state changes of one transaction to the view and the batch output.
pub struct Extractor<'v> {
    pub bridge: AccountId,
    pub config: AccountId,
    pub view: &'v mut BridgeView,
    pub out: &'v mut BatchOut,
}

impl<'v> Extractor<'v> {
    pub fn new(bridge: AccountId, view: &'v mut BridgeView, out: &'v mut BatchOut) -> Self {
        Self { bridge, config: bridge_core::config_address(&bridge), view, out }
    }

    fn token_program(&self) -> Option<AccountId> {
        self.view.initialized.then(|| AccountId::new(self.view.token_program))
    }

    fn config_actor(&self) -> Actor {
        Actor::new(self.config, self.bridge)
    }

    /// Whether a root call reaches our bridge or is a burn notified to our config (so a private
    /// transaction carrying it needs its receipt checked).
    #[must_use]
    pub fn root_touches(&self, root: &RootCall) -> bool {
        root.to.program_account_id == self.bridge || self.burn_of(root).is_some()
    }

    /// Whether a private execution delivers anything to an actor of our bridge program.
    #[must_use]
    pub fn private_touches(&self, ex: &ProvenExecution) -> bool {
        ex.public_root.as_ref().is_some_and(|r| self.root_touches(r))
            || ex.boundary.iter().any(|s| {
                matches!(s, BoundaryStep::PrivateToPublic(d) if d.envelope.to.program_account_id == self.bridge)
            })
    }

    fn burn_of(&self, root: &RootCall) -> Option<(AccountId, u128, [u8; 32])> {
        let tp = self.token_program()?;
        if root.to.program_account_id != tp {
            return None;
        }
        match borsh::from_slice::<TokenMessage>(&root.message) {
            // To the config. A burn notifying another actor of the bridge is a sponsored burn: the
            // bridge refuses it unless that actor is private (`Effect::SponsoredBurn`), and it is
            // counted from that actor's boundary effect below, never from the root call.
            Ok(TokenMessage::BurnWithNotice { definition, amount, notify, memo }) if notify == self.config_actor() => {
                Some((definition, amount, memo))
            }
            _ => None,
        }
    }

    /// A public root call (of a public transaction, or a private one's `public_root`).
    pub fn root(&mut self, at: At, root: &RootCall) -> Result<(), G2Error> {
        if root.to.program_account_id == self.bridge {
            let Ok(msg) = borsh::from_slice::<Message>(&root.message) else {
                return Err(at.bad("a bridge root message that does not decode"));
            };
            match msg {
                Message::Instruction(i) => self.instruction(at, root.to.account_id, i),
                Message::Effect(_) => Err(at.bad("an effect sent as a root call")),
            }
        } else if let Some((definition, amount, memo)) = self.burn_of(root) {
            self.burn(at, &definition, amount, memo)
        } else {
            Ok(())
        }
    }

    /// `signer`: the root call's receiver account, i.e. the signer whose actor of the bridge the
    /// instruction reaches (`instruct`'s `me`; a public root call names the account itself).
    fn instruction(&mut self, at: At, signer: AccountId, i: Instruction) -> Result<(), G2Error> {
        match i {
            // Retired: refused by the bridge since the 2026-11-14 security fixes. Read with the
            // old program's semantics so blocks of the programs before them still replay (the
            // `public9` fixtures); an applied one on a fixed bridge would be the sequencer's fault.
            Instruction::Init { vault, evm_chain_id, lez_chain_id, zone_id, token_program, .. } => {
                let cfg = bridge_core::cfg_id(evm_chain_id, &vault, &lez_chain_id, &zone_id, &self.bridge);
                self.init(at, cfg, token_program, true)
            }
            Instruction::InitAttested { vault, evm_chain_id, lez_chain_id, zone_id, token_program, .. } => {
                let cfg = bridge_core::cfg_id(evm_chain_id, &vault, &lez_chain_id, &zone_id, &self.bridge);
                self.init(at, cfg, token_program, false)
            }
            Instruction::Initialize { vault, evm_chain_id, lez_chain_id, token_program, .. } => {
                if signer != self.bridge {
                    return Err(at.bad("an Initialize not signed by the bridge's header account"));
                }
                let zone = bridge_core::source_id(&RootSource::StandIn);
                let cfg = bridge_core::cfg_id(evm_chain_id, &vault, &lez_chain_id, &zone, &self.bridge);
                self.init(at, cfg, token_program, true)
            }
            Instruction::InitializeAttested { vault, evm_chain_id, lez_chain_id, token_program, source, .. } => {
                if signer != self.bridge {
                    return Err(at.bad("an Initialize not signed by the bridge's header account"));
                }
                let zone = bridge_core::source_id(&RootSource::Attest(source));
                let cfg = bridge_core::cfg_id(evm_chain_id, &vault, &lez_chain_id, &zone, &self.bridge);
                self.init(at, cfg, token_program, false)
            }
            Instruction::Admin(op) => self.admin(at, op),
            Instruction::AttestRoot { root } => self.attest_root(at, root),
            Instruction::Claim { eth_token, denom_index, denom, secret, .. } => {
                if !self.view.stand_in {
                    return Err(at.bad("a stand-in claim on an RFP-021 config"));
                }
                self.claim(at, &eth_token, denom_index, denom)?;
                let nf = bridge_core::claim_nullifier(&secret, &signer, &eth_token, denom);
                self.spend(at, bridge_core::nullifier_address(&self.bridge, &nf))
            }
            Instruction::ClaimAttested { eth_token, denom_index, denom, secret, .. } => {
                if self.view.stand_in {
                    return Err(at.bad("an attested claim on a stand-in config"));
                }
                self.claim(at, &eth_token, denom_index, denom)?;
                let nf = bridge_core::claim_nullifier(&secret, &signer, &eth_token, denom);
                self.spend(at, bridge_core::nullifier_address(&self.bridge, &nf))
            }
            // A refunded public claim: the claim as above; the refund moves native LEZ only.
            Instruction::Relayed { claim, .. } => match *claim {
                c @ (Instruction::Claim { recipient_opening: None, .. } | Instruction::ClaimAttested { recipient_opening: None, .. }) => {
                    self.instruction(at, signer, c)
                }
                _ => Err(at.bad("a relayed instruction the bridge refuses")),
            },
        }
    }

    fn init(&mut self, at: At, cfg: [u8; 32], token_program: AccountId, stand_in: bool) -> Result<(), G2Error> {
        if self.view.initialized {
            return Err(at.bad("the bridge initialised twice"));
        }
        *self.view = BridgeView {
            initialized: true,
            cfg_id: cfg,
            token_program: *token_program.value(),
            stand_in,
            tokens: Vec::new(),
        };
        Ok(())
    }

    fn admin(&mut self, at: At, op: AdminOp) -> Result<(), G2Error> {
        if !self.view.initialized {
            return Err(at.bad("an admin operation before init"));
        }
        match op {
            AdminOp::RegisterToken { eth_token, definition, denoms, .. } => {
                // The bridge refuses both since the 2026-11-14 fixes (review M3); before them it took a
            // shared definition, which this view always refused.
            if self.view.tokens.iter().any(|t| t.eth_token == eth_token || t.definition == *definition.value()) {
                    return Err(at.bad("a token registered twice"));
                }
                if denoms.is_empty() || denoms.len() > bridge_core::MAX_DENOMS {
                    return Err(at.bad("bad denominations"));
                }
                let n = denoms.len();
                self.view.tokens.push(TokenView {
                    eth_token,
                    definition: *definition.value(),
                    denoms,
                    claims: vec![0; n],
                    burns: vec![0; n],
                    claimed_value: 0,
                    burned_value: 0,
                });
            }
            AdminOp::AddDenomination { eth_token, denom } => {
                let t = self.view.tokens.iter_mut().find(|t| t.eth_token == eth_token).ok_or(at.bad("token not registered"))?;
                if denom == 0 || t.denoms.contains(&denom) || t.denoms.len() >= bridge_core::MAX_DENOMS {
                    return Err(at.bad("bad denomination"));
                }
                t.denoms.push(denom);
                t.claims.push(0);
                t.burns.push(0);
            }
            // Liveness only (see the module docs).
            AdminOp::DeregisterToken { .. }
            | AdminOp::Freeze { .. }
            | AdminOp::Unfreeze { .. }
            | AdminOp::SetFreezer { .. }
            | AdminOp::SetAttestor { .. }
            | AdminOp::ProposeAdmin { .. }
            | AdminOp::AcceptAdmin
            | AdminOp::SetSponsor { .. }
            | AdminOp::SetRelayRefund { .. }
            | AdminOp::SetBurnSponsor { .. } => {}
        }
        Ok(())
    }

    fn attest_root(&mut self, at: At, root: [u8; 32]) -> Result<(), G2Error> {
        if !self.view.initialized || !self.view.stand_in {
            return Err(at.bad("a stand-in root on a config without the stand-in source"));
        }
        if !self.out.roots.contains(&root) {
            self.out.roots.push(root);
        }
        Ok(())
    }

    fn claim(&mut self, at: At, eth_token: &EthAddress, denom_index: u32, denom: u128) -> Result<(), G2Error> {
        if !self.view.initialized {
            return Err(at.bad("a claim before init"));
        }
        let t = self.view.tokens.iter_mut().find(|t| &t.eth_token == eth_token).ok_or(at.bad("token not registered"))?;
        let i = denom_index as usize;
        if i >= t.denoms.len() {
            return Err(at.bad("bad denomination"));
        }
        if t.denoms[i] != denom {
            return Err(at.bad("wrong amount for this denomination"));
        }
        t.claims[i] += 1;
        t.claimed_value = t.claimed_value.checked_add(denom).ok_or(at.bad("claimed value overflow"))?;
        match self.out.claims.iter_mut().find(|d| &d.token == eth_token && d.denom_index == denom_index) {
            Some(d) => d.count += 1,
            None => self.out.claims.push(ClaimDelta { token: *eth_token, denom_index, count: 1 }),
        }
        Ok(())
    }

    fn spend(&mut self, at: At, pda: AccountId) -> Result<(), G2Error> {
        let key = *pda.value();
        if self.out.nullifiers.contains(&key) {
            return Err(at.bad("a claim nullifier spent twice in one batch"));
        }
        self.out.nullifiers.push(key);
        Ok(())
    }

    fn burn(&mut self, at: At, definition: &AccountId, amount: u128, memo: [u8; 32]) -> Result<(), G2Error> {
        let t = self.view.tokens.iter_mut().find(|t| t.definition == *definition.value()).ok_or(at.bad("not a bridged token"))?;
        let i = t.denoms.iter().position(|d| *d == amount).ok_or(at.bad("a burn is exactly one denomination"))?;
        t.burns[i] += 1;
        t.burned_value = t.burned_value.checked_add(amount).ok_or(at.bad("burned value overflow"))?;
        if t.burned_value > t.claimed_value {
            return Err(at.bad("burns exceed claims"));
        }
        self.out.exits.push(Exit { token: t.eth_token, denom_index: i as u32, commitment: memo });
        Ok(())
    }

    /// The `PrivateToPublic` deliveries of a private execution (call after its receipt verified).
    pub fn private(&mut self, at: At, ex: &ProvenExecution) -> Result<(), G2Error> {
        if let Some(root) = &ex.public_root {
            self.root(at, root)?;
        }
        let (mut claims, mut spends) = (0usize, 0usize);
        for step in &ex.boundary {
            let BoundaryStep::PrivateToPublic(d) = step else { continue };
            let env = &d.envelope;
            if env.to.program_account_id != self.bridge {
                continue;
            }
            if env.from.program_account_id != self.bridge {
                // The bridge accepts a foreign message across the boundary only as the token
                // program's burn notice to the config (`handle`, `burn_notice`); one to a public
                // actor of the bridge is a sponsored burn the config refuses (its forwarder is not
                // private); anything else panics there.
                let tp = self.token_program();
                if env.to.account_id != self.config || tp != Some(env.from.program_account_id) {
                    return Err(at.bad("a foreign program's message to the bridge"));
                }
                let Ok(n) = borsh::from_slice::<BurnNotice>(&env.message) else {
                    return Err(at.bad("a burn notice that does not decode"));
                };
                self.burn(at, &n.definition, n.amount, n.memo)?;
                continue;
            }
            let Ok(msg) = borsh::from_slice::<Message>(&env.message) else {
                return Err(at.bad("a bridge message that does not decode"));
            };
            let Message::Effect(e) = msg else {
                return Err(at.bad("an instruction delivered by another actor"));
            };
            let to_config = env.to.account_id == self.config;
            match e {
                Effect::Init(c) if to_config => {
                    let stand_in = matches!(c.root_source, RootSource::StandIn);
                    let cfg = bridge_core::cfg_id(c.evm_chain_id, &c.vault, &c.lez_chain_id, &c.zone_id, &self.bridge);
                    if cfg != c.cfg_id || !c.tokens.is_empty() {
                        return Err(at.bad("an init whose config the bridge would not derive"));
                    }
                    self.init(at, cfg, c.token_program, stand_in)?;
                }
                Effect::Admin { op, .. } if to_config => self.admin(at, op)?,
                Effect::AttestRoot { root, .. } if to_config => self.attest_root(at, root)?,
                Effect::Claim { eth_token, denom_index, denom, .. } if to_config => {
                    if !self.view.stand_in {
                        return Err(at.bad("a stand-in claim on an RFP-021 config"));
                    }
                    self.claim(at, &eth_token, denom_index, denom)?;
                    claims += 1;
                }
                Effect::ClaimAttested { eth_token, denom_index, denom, .. } if to_config => {
                    if self.view.stand_in {
                        return Err(at.bad("an attested claim on a stand-in config"));
                    }
                    self.claim(at, &eth_token, denom_index, denom)?;
                    claims += 1;
                }
                Effect::SpendClaim if !to_config => {
                    self.spend(at, env.to.account_id)?;
                    spends += 1;
                }
                Effect::AssertRoot | Effect::RecordRoot if !to_config => {}
                // A refunded claim's refund: native LEZ from the float, not tracked.
                Effect::Refund { .. } if to_config => {}
                // A sponsored burn forwarded by a private actor of the bridge (the only way it is
                // counted: its root call notifies that actor, not the config).
                Effect::SponsoredBurn { token_program, notice, .. } if to_config => {
                    if self.token_program() != Some(token_program) {
                        return Err(at.bad("a sponsored burn from a foreign program"));
                    }
                    self.burn(at, &notice.definition, notice.amount, notice.memo)?;
                }
                _ => return Err(at.bad("an effect the bridge refuses at that account")),
            }
        }
        if claims != spends {
            return Err(at.bad("claims and nullifier spends differ in one transaction"));
        }
        Ok(())
    }
}
