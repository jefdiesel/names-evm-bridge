// SPDX-License-Identifier: MIT OR Apache-2.0
//! The bridge code (`logosbr1…`, `modules/NAMES-SEND-BACK.md` §9): one whole bridge deployment, so
//! a wallet can add a new vault without a new build. The same bytes as the bridge page's
//! `web/deposit/src/lib/bridge-code.js`; both are checked against
//! `web/deposit/test/vectors/bridge-code-v1.json` (generator `modules/tools/make-bridge-code-vectors.py` (not published)).
//!
//! ```text
//! payload v1 (integers big-endian)
//!   version(1)=1 | chainId(8) | vault(20) | vaultCreateBlock(8) | gate(20) | desk(20)
//!   | cfgId(32) | lezChainId(32) | zoneId(32) | bridgeProgram(32) | configAccount(32)
//!   | tokenProgram(32) | release(1: 0 never, 1 proof) | minReleaseDelay(8)
//!   | genesisLezBlock(8) | genesisG2State(32) | updateImageId(32) | releaseImageId(32)
//!   | flags(1: bit 0 the endpoint relays claims, bit 1 it may settle) | endpointLen(1) endpoint
//!   | labelLen(1) label | tokenCount(1) { ethToken(20) | definition(32) | decimals(1)
//!                                        | symbolLen(1) symbol | denomCount(1) | denom(16)… }
//! code  = bech32m("logosbr", payload)
//! words = the compare words of SHA-256("logos-bridge/bridge-code/v1" | payload)
//! ```
//!
//! A reader checks the layout in byte order, then that cfgId follows from its parts (the vault's
//! `computeCfgId`) and that configAccount is the bridge program's Config PDA. Those are the checks
//! that need no network; `bridges.rs` then reads both chains before a code is accepted.

use crate::config::{Deployment, Token};
use crate::lez;
use crate::request::bech32m;
use crate::util::{self, sha256};
use crate::withdraw::{bech32m_decode, from_input, CodeError};
use serde_json::{json, Value};

pub const HRP: &str = "logosbr";
pub const VERSION: u8 = 1;
pub const TAG: &[u8] = b"logos-bridge/bridge-code/v1";
/// `keccak256("RFP022/cfg/v1")`, BridgeVault.CFG_TAG.
pub const CFG_TAG: [u8; 32] = [
    0x26, 0xe9, 0x62, 0x30, 0x2b, 0x1d, 0xc9, 0x41, 0x7c, 0x46, 0x6f, 0xf5, 0x6e, 0xf2, 0x06, 0xa5, 0x78, 0xcd, 0x05, 0x4a, 0x0a, 0x09, 0x91, 0x72, 0xcf, 0xf8, 0xb8, 0x04,
    0x60, 0xab, 0x5a, 0x45,
];
const MAX_CODE: usize = 2000;

fn err<T>(code: &'static str, message: impl Into<String>) -> Result<T, CodeError> {
    Err(CodeError { code, message: message.into() })
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodeToken {
    pub eth_token: [u8; 20],
    pub definition: [u8; 32],
    pub decimals: u8,
    pub symbol: String,
    pub denoms: Vec<u128>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BridgeCode {
    pub chain_id: u64,
    pub vault: [u8; 20],
    pub vault_create_block: u64,
    pub gate: [u8; 20],
    pub desk: [u8; 20],
    pub cfg_id: [u8; 32],
    pub lez_chain_id: [u8; 32],
    pub zone_id: [u8; 32],
    pub bridge_program: [u8; 32],
    pub config_account: [u8; 32],
    pub token_program: [u8; 32],
    /// true: the vault releases by proof; false: it can't pay out.
    pub release_proof: bool,
    pub min_release_delay: u64,
    pub genesis_lez_block: u64,
    pub genesis_g2_state: [u8; 32],
    pub update_image: [u8; 32],
    pub release_image: [u8; 32],
    pub endpoint: String,
    pub relay: bool,
    pub settle: bool,
    pub label: String,
    pub tokens: Vec<CodeToken>,
}

/// `cfgId = SHA-256(CFG_TAG | u256(chainId) | vault | lezChainId | zoneId | programId)`.
pub fn cfg_id_of(chain_id: u64, vault: &[u8; 20], lez_chain_id: &[u8; 32], zone_id: &[u8; 32], program: &[u8; 32]) -> [u8; 32] {
    let mut chain = [0u8; 32];
    chain[24..].copy_from_slice(&chain_id.to_be_bytes());
    sha256(&[&CFG_TAG, &chain, vault, lez_chain_id, zone_id, program])
}

fn endpoint_ok(s: &str) -> bool {
    s.strip_prefix("https://").is_some_and(|rest| (4..=92).contains(&rest.len()) && rest.bytes().all(|c| c.is_ascii_alphanumeric() || b".-:/_".contains(&c)))
}

fn label_ok(s: &str) -> bool {
    (1..=32).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_alphanumeric() || b" .()-".contains(&c))
}

fn symbol_ok(s: &str) -> bool {
    (1..=8).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_alphanumeric())
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], CodeError> {
        let s = self.b.get(self.at..self.at + n).ok_or(CodeError { code: "short", message: "The bridge code ends too early: copy it again, whole.".into() })?;
        self.at += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, CodeError> {
        Ok(self.take(1)?[0])
    }
    fn u64(&mut self) -> Result<u64, CodeError> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn b20(&mut self) -> Result<[u8; 20], CodeError> {
        Ok(self.take(20)?.try_into().unwrap())
    }
    fn b32(&mut self) -> Result<[u8; 32], CodeError> {
        Ok(self.take(32)?.try_into().unwrap())
    }
    fn text(&mut self) -> Result<String, CodeError> {
        let n = self.u8()? as usize;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
}

impl BridgeCode {
    pub fn payload(&self) -> Vec<u8> {
        let mut p = vec![VERSION];
        p.extend(self.chain_id.to_be_bytes());
        p.extend(self.vault);
        p.extend(self.vault_create_block.to_be_bytes());
        p.extend(self.gate);
        p.extend(self.desk);
        for b in [&self.cfg_id, &self.lez_chain_id, &self.zone_id, &self.bridge_program, &self.config_account, &self.token_program] {
            p.extend(b);
        }
        p.push(u8::from(self.release_proof));
        p.extend(self.min_release_delay.to_be_bytes());
        p.extend(self.genesis_lez_block.to_be_bytes());
        p.extend(self.genesis_g2_state);
        p.extend(self.update_image);
        p.extend(self.release_image);
        p.push(u8::from(self.relay) | (u8::from(self.settle) << 1));
        for t in [&self.endpoint, &self.label] {
            p.push(t.len().min(255) as u8);
            p.extend(t.as_bytes());
        }
        p.push(self.tokens.len().min(255) as u8);
        for t in &self.tokens {
            p.extend(t.eth_token);
            p.extend(t.definition);
            p.push(t.decimals);
            p.push(t.symbol.len().min(255) as u8);
            p.extend(t.symbol.as_bytes());
            p.push(t.denoms.len().min(255) as u8);
            for d in &t.denoms {
                p.extend(d.to_be_bytes());
            }
        }
        p
    }

    /// The code, after the same checks a reader applies.
    pub fn encode(&self) -> Result<String, CodeError> {
        let p = self.payload();
        let back = Self::from_bytes(&p)?;
        if &back != self {
            return err("field", "a field does not survive encoding (a text over 255 bytes?)");
        }
        Ok(bech32m(HRP, &p))
    }

    /// The 32 bytes the four compare words are taken over.
    pub fn digest(&self) -> [u8; 32] {
        sha256(&[TAG, &self.payload()])
    }

    pub fn words_text(&self) -> String {
        crate::compare::words_text(&self.digest())
    }

    pub fn from_bytes(b: &[u8]) -> Result<BridgeCode, CodeError> {
        if b.first() != Some(&VERSION) {
            return err("version", format!("This bridge code is version {}; this app reads version {VERSION}.", b.first().copied().unwrap_or(0)));
        }
        let mut r = Reader { b, at: 1 };
        let chain_id = r.u64()?;
        if chain_id == 0 {
            return err("chain", "The bridge code names no chain.");
        }
        let vault = r.b20()?;
        if vault == [0; 20] {
            return err("field", "The bridge code names no vault.");
        }
        let vault_create_block = r.u64()?;
        let (gate, desk) = (r.b20()?, r.b20()?);
        let (cfg_id, lez_chain_id, zone_id) = (r.b32()?, r.b32()?, r.b32()?);
        let (bridge_program, config_account, token_program) = (r.b32()?, r.b32()?, r.b32()?);
        let release = r.u8()?;
        if release > 1 {
            return err("field", "The bridge code's release kind is unknown.");
        }
        let min_release_delay = r.u64()?;
        let genesis_lez_block = r.u64()?;
        let (genesis_g2_state, update_image, release_image) = (r.b32()?, r.b32()?, r.b32()?);
        let flags = r.u8()?;
        if flags & !3 != 0 {
            return err("field", "The bridge code has a flag this app doesn't know.");
        }
        let endpoint = r.text()?;
        if !endpoint_ok(&endpoint) {
            return err("endpoint", "The bridge code's network address is not an https address.");
        }
        let label = r.text()?;
        if !label_ok(&label) {
            return err("label", "The bridge code's name has characters a name can't have.");
        }
        let count = r.u8()?;
        if !(1..=4).contains(&count) {
            return err("tokens", "A bridge code lists 1 to 4 tokens.");
        }
        let mut tokens: Vec<CodeToken> = Vec::new();
        for _ in 0..count {
            let eth_token = r.b20()?;
            let definition = r.b32()?;
            let decimals = r.u8()?;
            let symbol = r.text()?;
            let n = r.u8()?;
            if !(1..=8).contains(&n) {
                return err("tokens", "A token has 1 to 8 denominations.");
            }
            let mut denoms = Vec::new();
            for _ in 0..n {
                denoms.push(u128::from_be_bytes(r.take(16)?.try_into().unwrap()));
            }
            if decimals > 36 || !symbol_ok(&symbol) {
                return err("tokens", "A token's decimals or symbol are out of range.");
            }
            if denoms.iter().enumerate().any(|(j, d)| *d == 0 || (j > 0 && *d <= denoms[j - 1])) {
                return err("tokens", "A token's denominations must be above 0 and increasing.");
            }
            if tokens.iter().any(|t| t.eth_token == eth_token || t.definition == definition) {
                return err("tokens", "A token is listed twice.");
            }
            tokens.push(CodeToken { eth_token, definition, decimals, symbol, denoms });
        }
        if r.at != b.len() {
            return err("trailing", "The bridge code has bytes after its last token.");
        }
        if cfg_id_of(chain_id, &vault, &lez_chain_id, &zone_id, &bridge_program) != cfg_id {
            return err("cfg_id", "The bridge code's cfgId doesn't follow from its vault, chain and bridge program: it was changed or made wrong.");
        }
        if lez::pda(&bridge_program, &lez::CONFIG_SEED) != config_account {
            return err("config_account", "The bridge code's Config account is not its bridge program's.");
        }
        Ok(BridgeCode {
            chain_id, vault, vault_create_block, gate, desk, cfg_id, lez_chain_id, zone_id, bridge_program, config_account, token_program,
            release_proof: release == 1, min_release_delay, genesis_lez_block, genesis_g2_state, update_image, release_image,
            endpoint, relay: flags & 1 != 0, settle: flags & 2 != 0, label, tokens,
        })
    }

    /// A pasted code (or a link with it after `#`).
    pub fn decode(text: &str) -> Result<BridgeCode, CodeError> {
        let (hrp, bytes) = bech32m_decode(&from_input(text), MAX_CODE)?;
        if hrp != HRP {
            return err("hrp", format!("This is a \"{hrp}\" code, not a bridge code."));
        }
        Self::from_bytes(&bytes)
    }

    /// The fields as the shared vectors write them.
    pub fn fields_json(&self) -> Value {
        let a = |x: &[u8; 20]| format!("0x{}", util::to_hex(x));
        let h = |x: &[u8; 32]| format!("0x{}", util::to_hex(x));
        json!({
            "chain_id": self.chain_id, "vault": a(&self.vault), "vault_create_block": self.vault_create_block,
            "gate": a(&self.gate), "desk": a(&self.desk), "cfg_id": h(&self.cfg_id), "lez_chain_id": h(&self.lez_chain_id),
            "zone_id": h(&self.zone_id), "bridge_program": lez::b58(&self.bridge_program), "config_account": lez::b58(&self.config_account),
            "token_program": lez::b58(&self.token_program), "release": if self.release_proof { "proof" } else { "never" },
            "min_release_delay": self.min_release_delay, "genesis_lez_block": self.genesis_lez_block,
            "genesis_g2_state": h(&self.genesis_g2_state), "update_image": h(&self.update_image), "release_image": h(&self.release_image),
            "endpoint": self.endpoint, "relay": self.relay, "settle": self.settle, "label": self.label,
            "tokens": self.tokens.iter().map(|t| json!({ "eth_token": a(&t.eth_token), "definition": lez::b58(&t.definition),
                "decimals": t.decimals, "symbol": t.symbol, "denoms": t.denoms.iter().map(|d| d.to_string()).collect::<Vec<_>>() })).collect::<Vec<_>>(),
        })
    }

    /// The code of a deployment that carries the code's other fields (its `bridge` block in
    /// deployments.json, and the tokens' denominations); Err for one without (v3, v2).
    pub fn from_deployment(d: &Deployment) -> Result<BridgeCode, String> {
        let x = &d.bridge;
        if !x.is_object() {
            return Err(format!("{} has no bridge code", crate::config::plain_label(&d.name)));
        }
        let s = |k: &str| x[k].as_str().map(str::to_owned).ok_or_else(|| format!("the deployment's bridge block has no \"{k}\""));
        let n = |k: &str| x[k].as_u64().ok_or_else(|| format!("the deployment's bridge block has no number \"{k}\""));
        let addr = |t: &str| if t.is_empty() { Ok([0; 20]) } else { util::eth_address(t) };
        let tokens = d
            .tokens
            .iter()
            .map(|t| Ok(CodeToken { eth_token: t.eth_token, definition: util::lez_account(&t.definition)?, decimals: t.decimals, symbol: t.symbol.clone(), denoms: t.denoms.clone() }))
            .collect::<Result<Vec<_>, String>>()?;
        let c = BridgeCode {
            chain_id: d.chain_id,
            vault: d.vault,
            vault_create_block: d.vault_create_block,
            gate: addr(&s("gate")?)?,
            desk: addr(&d.desk)?,
            cfg_id: util::hex32(&d.cfg_id)?,
            lez_chain_id: util::hex32(&s("lez_chain_id")?)?,
            zone_id: util::hex32(&s("zone_id")?)?,
            bridge_program: util::lez_account(&d.bridge_program)?,
            config_account: util::lez_account(&s("config_account")?)?,
            token_program: util::lez_account(&d.token_program)?,
            release_proof: d.release == "proof",
            min_release_delay: n("min_release_delay")?,
            genesis_lez_block: n("genesis_lez_block")?,
            genesis_g2_state: util::hex32(&s("genesis_g2_state")?)?,
            update_image: util::hex32(&s("update_image")?)?,
            release_image: util::hex32(&s("release_image")?)?,
            endpoint: d.lez_rpc.clone(),
            relay: d.relay == "rpc",
            settle: x["settle"].as_bool().unwrap_or(false),
            label: s("label")?,
            tokens,
        };
        c.encode().map_err(|e| format!("{} doesn't make a valid bridge code: {}", crate::config::plain_label(&d.name), e.message))?;
        Ok(c)
    }

    /// The name an added deployment is stored under: from its cfgId, never from the code's text.
    pub fn added_name(&self) -> String {
        format!("added-{}", &util::to_hex(&self.cfg_id)[..12])
    }

    /// The host of the code's endpoint, for people ("lez.example.org").
    pub fn endpoint_host(&self) -> String {
        self.endpoint.trim_start_matches("https://").split(['/', ':']).next().unwrap_or("").to_owned()
    }

    /// The deployment this module acts on for an added code.
    pub fn to_deployment(&self) -> Deployment {
        let delay = self.min_release_delay;
        let release_note = if !self.release_proof {
            "This vault can't pay out on Ethereum: sending back is off for it.".to_owned()
        } else if delay == 0 {
            "Withdrawals back to Ethereum are checked by proof on Ethereum.".to_owned()
        } else {
            format!("Withdrawals back to Ethereum are checked by proof on Ethereum, then wait {} before release.", util::duration_text(delay))
        };
        let gate = if self.gate == [0; 20] { String::new() } else { util::checksum_address(&self.gate) };
        Deployment {
            name: self.added_name(),
            label: self.label.clone(),
            chain_id: self.chain_id,
            vault: self.vault,
            vault_create_block: self.vault_create_block,
            cfg_id: format!("0x{}", util::to_hex(&self.cfg_id)),
            bridge_program: lez::b58(&self.bridge_program),
            token_program: lez::b58(&self.token_program),
            attest_program: String::new(),
            attest_instance: String::new(),
            probe_program: String::new(),
            tokens: self
                .tokens
                .iter()
                .map(|t| Token { eth_token: t.eth_token, symbol: t.symbol.clone(), decimals: t.decimals, definition: lez::b58(&t.definition), denoms: t.denoms.clone() })
                .collect(),
            notes: vec![format!("Added from a bridge code: vault {} on Sepolia, read through {}.", util::checksum_address(&self.vault), self.endpoint_host())],
            network: format!("LEZ 0.4 · {}", self.endpoint_host()).chars().take(60).collect(),
            lez_rpc: self.endpoint.clone(),
            relay: if self.relay { "rpc".into() } else { String::new() },
            desk: if self.desk == [0; 20] { String::new() } else { util::checksum_address(&self.desk) },
            release: if self.release_proof { "proof".into() } else { "never".into() },
            release_note,
            release_detail: String::new(),
            bridge: json!({
                "label": self.label, "gate": gate, "lez_chain_id": format!("0x{}", util::to_hex(&self.lez_chain_id)),
                "zone_id": format!("0x{}", util::to_hex(&self.zone_id)), "config_account": lez::b58(&self.config_account),
                "min_release_delay": self.min_release_delay, "genesis_lez_block": self.genesis_lez_block,
                "genesis_g2_state": format!("0x{}", util::to_hex(&self.genesis_g2_state)),
                "update_image": format!("0x{}", util::to_hex(&self.update_image)), "release_image": format!("0x{}", util::to_hex(&self.release_image)),
                "settle": self.settle, "added": true,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shared vectors, also read by the bridge page's test.
    const VECTORS: &str = include_str!("../../../../web/deposit/test/vectors/bridge-code-v1.json");

    #[test]
    fn the_tag_and_the_cfg_tag_are_the_vectors_and_the_vault_s() {
        let v: Value = serde_json::from_str(VECTORS).unwrap();
        assert_eq!(v["hrp"], HRP);
        assert_eq!(v["tag"].as_str().unwrap().as_bytes(), TAG);
        assert_eq!(v["cfgTag"], format!("0x{}", util::to_hex(&CFG_TAG)));
        assert_eq!(CFG_TAG, util::keccak(b"RFP022/cfg/v1"));
    }

    #[test]
    fn every_valid_vector_reads_writes_and_gives_its_words() {
        let v: Value = serde_json::from_str(VECTORS).unwrap();
        for c in v["valid"].as_array().unwrap() {
            let code = c["code"].as_str().unwrap();
            let b = BridgeCode::decode(code).unwrap_or_else(|e| panic!("{}: {e:?}", c["name"]));
            assert_eq!(b.fields_json(), c["fields"], "{}", c["name"]);
            assert_eq!(format!("0x{}", util::to_hex(&b.payload())), c["payload"].as_str().unwrap());
            assert_eq!(b.encode().unwrap(), code);
            assert_eq!(format!("0x{}", util::to_hex(&b.digest())), c["digest"].as_str().unwrap());
            assert_eq!(b.words_text(), c["words_text"].as_str().unwrap(), "{}", c["name"]);
            assert_eq!(BridgeCode::decode(&code.to_ascii_uppercase()).unwrap(), b);
            assert_eq!(BridgeCode::decode(&format!(" https://nameswallet.app/bridge/#{code}\n")).unwrap(), b);
            // An added deployment made from it gives the same code back.
            assert_eq!(BridgeCode::from_deployment(&b.to_deployment()).unwrap(), b, "{}", c["name"]);
        }
    }

    #[test]
    fn every_invalid_vector_is_refused_with_its_code() {
        let v: Value = serde_json::from_str(VECTORS).unwrap();
        for c in v["invalid"].as_array().unwrap() {
            let e = BridgeCode::decode(c["code"].as_str().unwrap()).unwrap_err();
            assert_eq!(e.code, c["error"].as_str().unwrap(), "{}: {}", c["name"], e.message);
        }
    }

    /// The drift check: this build's own v5 and v4 encode to the codes the page's vectors hold (the
    /// page checks its deployments.js the same way), so the wallet and the page show the same words.
    #[test]
    fn the_built_in_v5_and_v4_are_the_vectors_codes() {
        let v: Value = serde_json::from_str(VECTORS).unwrap();
        for (name, want) in [("bridge-test-network-v5", "v5"), ("bridge-test-network-v4", "v4")] {
            let d = crate::config::builtin(name).unwrap();
            let c = BridgeCode::from_deployment(&d).unwrap();
            let vec = v["valid"].as_array().unwrap().iter().find(|x| x["name"] == want).unwrap();
            assert_eq!(c.fields_json(), vec["fields"], "{name}");
            assert_eq!(c.encode().unwrap(), vec["code"].as_str().unwrap(), "{name}");
        }
        assert!(BridgeCode::from_deployment(&crate::config::builtin("bridge-test-network-v3").unwrap()).is_err(), "v3 has no code");
    }

    #[test]
    fn an_added_name_comes_from_the_cfg_id_and_the_host_from_the_endpoint() {
        let v: Value = serde_json::from_str(VECTORS).unwrap();
        let c = BridgeCode::decode(v["valid"][2]["code"].as_str().unwrap()).unwrap();
        assert_eq!(c.added_name(), format!("added-{}", &v["valid"][2]["fields"]["cfg_id"].as_str().unwrap()[2..14]));
        assert_eq!(c.endpoint_host(), "bridge.example.org");
        let d = c.to_deployment();
        assert_eq!(d.release, "never");
        assert_eq!(d.relay, "");
        assert!(crate::config::Deployment::from_json(&d.to_json()).is_ok());
    }
}
