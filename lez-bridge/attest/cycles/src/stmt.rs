// SPDX-License-Identifier: MIT OR Apache-2.0
//! Statements on REAL data and the reference consumers (branch `public-update`), EXECUTOR ONLY:
//! builds the messages from JSON-RPC answers saved in a directory, checks them natively, runs them
//! in the executor, and writes each message as hex for `lez-bridge raw-call`.
//!
//!   attest_cycles stmt-log ATTEST PROBE SIGNER INSTANCE CHAIN DIR TX LOG
//!       DIR/block.json (eth_getBlockByHash, hashes only) and DIR/receipts.json
//!       (eth_getBlockReceipts): the header RLP is rebuilt and must hash to the block hash; the
//!       receipts are encoded and their trie root must equal the header's receipts root. Writes
//!       DIR/probe-log-TX-LOG.hex (reference probe, `Probe::Log`).
//!   attest_cycles stmt-account ATTEST PROBE SIGNER INSTANCE CHAIN SLOTDIR FIELD
//!       SLOTDIR = `lez-bridge attest prove-slot --fetch-only --save-dir` (eth_getProof.json,
//!       header.rlp.hex, anchor.hex); FIELD = nonce|balance|storage-root|code-hash; the predicate is
//!       Eq to the proved value. Writes SLOTDIR/probe-account-FIELD.hex.
//!   attest_cycles consumer CONSUMER SIGNER ATTEST INSTANCE CHAIN KIND A [B] DIR TX LOG
//!       KIND erc20 (A = token, B = recipient contract) or eth (A = the contract); writes
//!       DIR/consumer-KIND-CHAIN-init.hex and DIR/consumer-KIND-CHAIN-claim-TX-LOG.hex and prints
//!       the config and nullifier PDAs.

use super::*;
use eth_attest_core::{
    Anchor,
    inclusion::{AnchorWitness, ReceiptProof, StatementContext, verify_receipt_log},
    testkit::receipts_trie,
};
use eth_consumer::{Config, Kind, Msg as CMsg};

fn rlp_len(len: usize, offset: u8) -> Vec<u8> {
    if len <= 55 {
        return vec![offset + u8::try_from(len).unwrap()];
    }
    let be = len.to_be_bytes();
    let skip = be.iter().take_while(|b| **b == 0).count();
    let mut out = vec![offset + 55 + u8::try_from(be.len() - skip).unwrap()];
    out.extend_from_slice(&be[skip..]);
    out
}

fn rlp_bytes(x: &[u8]) -> Vec<u8> {
    if x.len() == 1 && x[0] < 0x80 {
        return x.to_vec();
    }
    let mut out = rlp_len(x.len(), 0x80);
    out.extend_from_slice(x);
    out
}

fn rlp_list(items: &[Vec<u8>]) -> Vec<u8> {
    let payload: Vec<u8> = items.concat();
    let mut out = rlp_len(payload.len(), 0xc0);
    out.extend_from_slice(&payload);
    out
}

fn hex_field(v: &Value, k: &str) -> Result<Vec<u8>> {
    Ok(hx(v[k].as_str().with_context(|| format!("no {k}"))?))
}

fn int_field(v: &Value, k: &str) -> Result<Vec<u8>> {
    let t = v[k].as_str().with_context(|| format!("no {k}"))?.trim_start_matches("0x").trim_start_matches('0');
    let t = if t.len() % 2 == 1 { format!("0{t}") } else { t.to_owned() };
    Ok(hex::decode(t)?)
}

/// The execution header's RLP from `eth_getBlockByHash` JSON (the CLI's `rebuild_header`, field
/// order through Amsterdam; an absent field is absent from the header).
fn header_rlp(b: &Value) -> Result<Vec<u8>> {
    let mut f = Vec::new();
    for k in ["parentHash", "sha3Uncles", "miner", "stateRoot", "transactionsRoot", "receiptsRoot", "logsBloom"] {
        f.push(rlp_bytes(&hex_field(b, k)?));
    }
    for k in ["difficulty", "number", "gasLimit", "gasUsed", "timestamp"] {
        f.push(rlp_bytes(&int_field(b, k)?));
    }
    for k in ["extraData", "mixHash", "nonce"] {
        f.push(rlp_bytes(&hex_field(b, k)?));
    }
    f.push(rlp_bytes(&int_field(b, "baseFeePerGas")?));
    for (k, is_int) in [
        ("withdrawalsRoot", false),
        ("blobGasUsed", true),
        ("excessBlobGas", true),
        ("parentBeaconBlockRoot", false),
        ("requestsHash", false),
        ("blockAccessListHash", false),
        ("slotNumber", true),
    ] {
        if b.get(k).is_some_and(|v| !v.is_null()) {
            f.push(rlp_bytes(&if is_int { int_field(b, k)? } else { hex_field(b, k)? }));
        }
    }
    Ok(rlp_list(&f))
}

/// A receipt as the receipts trie holds it: `type ‖ rlp([status, cumulativeGasUsed, logsBloom, logs])`
/// (legacy: no type byte).
fn receipt_bytes(r: &Value) -> Result<Vec<u8>> {
    let logs: Vec<Vec<u8>> = r["logs"]
        .as_array()
        .context("logs")?
        .iter()
        .map(|l| -> Result<Vec<u8>> {
            let topics: Vec<Vec<u8>> = l["topics"].as_array().context("topics")?.iter().map(|t| rlp_bytes(&hx(t.as_str().unwrap()))).collect();
            Ok(rlp_list(&[rlp_bytes(&hex_field(l, "address")?), rlp_list(&topics), rlp_bytes(&hex_field(l, "data")?)]))
        })
        .collect::<Result<_>>()?;
    let body = rlp_list(&[rlp_bytes(&int_field(r, "status")?), rlp_bytes(&int_field(r, "cumulativeGasUsed")?), rlp_bytes(&hex_field(r, "logsBloom")?), rlp_list(&logs)]);
    let ty = int_field(r, "type")?;
    Ok(match ty.as_slice() {
        [] => body,
        [t] => [vec![*t], body].concat(),
        _ => return Err(anyhow!("receipt type")),
    })
}

/// The anchor witness and the receipt proof for receipt `tx` of the block saved in `dir`.
fn receipt_material(dir: &Path, tx: u64) -> Result<(AnchorWitness, ReceiptProof)> {
    let block: Value = serde_json::from_slice(&std::fs::read(dir.join("block.json"))?)?;
    let block = if block.get("result").is_some() { block["result"].clone() } else { block };
    let receipts: Value = serde_json::from_slice(&std::fs::read(dir.join("receipts.json"))?)?;
    let receipts = if receipts.get("result").is_some() { receipts["result"].clone() } else { receipts };
    let header = header_rlp(&block)?;
    let hash = h32(block["hash"].as_str().context("hash")?);
    ensure!(keccak(&header) == hash, "the rebuilt header does not hash to the block hash");
    let enc: Vec<Vec<u8>> = receipts.as_array().context("receipts")?.iter().map(receipt_bytes).collect::<Result<_>>()?;
    let (root, proof) = receipts_trie(&enc, tx);
    ensure!(root == h32(block["receiptsRoot"].as_str().context("receiptsRoot")?), "the receipts do not give the header's receipts root");
    println!("  block {} (0x{}), {} receipts, receipts root matches the header", u64::from_str_radix(block["number"].as_str().unwrap().trim_start_matches("0x"), 16)?, hex::encode(hash), enc.len());
    Ok((AnchorWitness { anchor: Anchor::BlockHash(hash), header_rlp: Some(header) }, proof))
}

fn id(s: &str) -> Result<AccountId> {
    s.parse().map_err(|_| anyhow!("bad account id {s}"))
}

fn exec_one(label: &str, elf: &[u8], native: fn(&ReceiveInput) -> Response, input: &ReceiveInput) -> Result<u64> {
    let mut rows = Vec::new();
    exec(&mut rows, label, elf, native, input)?;
    Ok(rows[0].user)
}

fn anchor_query(attest: AccountId, instance: [u8; 32], chain: u64, anchor: &Anchor, rec: &AnchorRecord, from: Actor) -> ReceiveInput {
    ReceiveInput {
        receiver: Actor::new(anchor_address(&attest, &instance, anchor), attest),
        from: Some(from),
        is_authorized: false,
        pre_state: ActorState::from(msg(rec)),
        message: msg(&Message::Query(Query::AssertAnchor { instance_id: instance, evm_chain_id: chain, anchor: *anchor })),
    }
}

/// A stand-in for the anchor record the chain holds (only its instance, chain and anchor are read).
fn record(instance: [u8; 32], chain: u64, anchor: Anchor) -> AnchorRecord {
    AnchorRecord { instance_id: instance, evm_chain_id: chain, anchor, beacon_slot: 0, beacon_root: [0; 32], block_number: None }
}

pub fn run(args: &[String]) -> Result<bool> {
    match args.first().map(String::as_str) {
        Some("stmt-log") => {
            let [_, attest, probe, signer, instance, chain, dir, tx, log] = args else { return Err(anyhow!("stmt-log ATTEST PROBE SIGNER INSTANCE CHAIN DIR TX LOG")) };
            let (attest, probe, signer, instance, chain, dir) = (id(attest)?, id(probe)?, id(signer)?, h32(instance), chain.parse::<u64>()?, Path::new(dir));
            let (tx, log): (u64, u64) = (tx.parse()?, log.parse()?);
            println!("# receipt-log statement on real data (reference probe)");
            let (witness, proof) = receipt_material(dir, tx)?;
            let ctx = StatementContext { evm_chain_id: chain, instance_id: instance };
            let s = verify_receipt_log(&ctx, &witness, &proof, log).map_err(|e| anyhow!("{e}"))?;
            println!("  log {log} of tx {tx}: address 0x{}, {} topics, {} data bytes; statement id 0x{}", hex::encode(s.log.address), s.log.topics.len(), s.log.data.len(), hex::encode(s.id()));
            let m = msg(&Probe::Log { attest, ctx, witness: witness.clone(), proof, log_index: log, expect: s.log.clone() });
            let a = exec_one("probe: the statement in the signer's actor", ATTEST_PROBE_GUEST_ELF, eth_attest_probe::handle, &root_input(signer, probe, m.clone()))?;
            let b = exec_one("attestation: AssertAnchor", ATTEST_GUEST_PUBLIC_ELF, eth_attest::handle, &anchor_query(attest, instance, chain, &witness.anchor, &record(instance, chain, witness.anchor), Actor::new(signer, probe)))?;
            println!("  whole transaction {} user cycles; message {} bytes; anchor PDA {}", a + b, m.len(), anchor_address(&attest, &instance, &witness.anchor));
            let out = dir.join(format!("probe-log-{tx}-{log}.hex"));
            std::fs::write(&out, hex::encode(&m))?;
            println!("message written to {}", out.display());
            Ok(true)
        }
        Some("stmt-account") => {
            let [_, attest, probe, signer, instance, chain, dir, field] = args else { return Err(anyhow!("stmt-account ATTEST PROBE SIGNER INSTANCE CHAIN SLOTDIR FIELD")) };
            let (attest, probe, signer, instance, chain, dir) = (id(attest)?, id(probe)?, id(signer)?, h32(instance), chain.parse::<u64>()?, Path::new(dir));
            println!("# account-field statement on real data (reference probe)");
            let pf: Value = serde_json::from_slice(&std::fs::read(dir.join("eth_getProof.json"))?)?;
            let pf = if pf.get("result").is_some() { pf["result"].clone() } else { pf };
            let header = hx(std::fs::read_to_string(dir.join("header.rlp.hex"))?.trim());
            let anchor = Anchor::BlockHash(keccak(&header));
            let nodes = |v: &Value| v.as_array().unwrap().iter().map(|x| hx(x.as_str().unwrap())).collect::<Vec<_>>();
            let account = AccountProof {
                fields: AccountFields {
                    nonce: u64::from_str_radix(pf["nonce"].as_str().unwrap().trim_start_matches("0x"), 16)?,
                    balance: h32(pf["balance"].as_str().unwrap()),
                    storage_root: h32(pf["storageHash"].as_str().unwrap()),
                    code_hash: h32(pf["codeHash"].as_str().unwrap()),
                },
                nodes: nodes(&pf["accountProof"]),
            };
            let address: [u8; 20] = hx(pf["address"].as_str().unwrap()).try_into().map_err(|_| anyhow!("address"))?;
            let (f, value) = match field.as_str() {
                "nonce" => (AccountField::Nonce, word(u128::from(account.fields.nonce))),
                "balance" => (AccountField::Balance, account.fields.balance),
                "storage-root" => (AccountField::StorageRoot, account.fields.storage_root),
                "code-hash" => (AccountField::CodeHash, account.fields.code_hash),
                _ => return Err(anyhow!("field: nonce|balance|storage-root|code-hash")),
            };
            let witness = AnchorWitness { anchor, header_rlp: Some(header) };
            let ctx = StatementContext { evm_chain_id: chain, instance_id: instance };
            let s = eth_attest_core::inclusion::verify_account_field(&ctx, &witness, &address, f, Predicate::Eq(value), &account).map_err(|e| anyhow!("{e}"))?;
            println!("  account 0x{} {field} = 0x{}; anchor 0x{}; statement id 0x{}", hex::encode(address), hex::encode(value), hex::encode(anchor.value()), hex::encode(s.id()));
            let m = msg(&Probe::Account { attest, ctx, witness, address, field: f, predicate: Predicate::Eq(value), proof: account });
            let a = exec_one("probe: the statement in the signer's actor", ATTEST_PROBE_GUEST_ELF, eth_attest_probe::handle, &root_input(signer, probe, m.clone()))?;
            let b = exec_one("attestation: AssertAnchor", ATTEST_GUEST_PUBLIC_ELF, eth_attest::handle, &anchor_query(attest, instance, chain, &anchor, &record(instance, chain, anchor), Actor::new(signer, probe)))?;
            println!("  whole transaction {} user cycles; message {} bytes; anchor PDA {}", a + b, m.len(), anchor_address(&attest, &instance, &anchor));
            let out = dir.join(format!("probe-account-{field}.hex"));
            std::fs::write(&out, hex::encode(&m))?;
            println!("message written to {}", out.display());
            Ok(true)
        }
        Some("consumer") => {
            let (consumer, signer, attest, instance, chain) = (id(&args[1])?, id(&args[2])?, id(&args[3])?, h32(&args[4]), args[5].parse::<u64>()?);
            let a20 = |s: &str| -> Result<[u8; 20]> { hx(s).try_into().map_err(|_| anyhow!("address {s}")) };
            let (kind, rest) = match args[6].as_str() {
                "erc20" => (Kind::Erc20 { token: a20(&args[7])?, recipient: a20(&args[8])? }, &args[9..]),
                "eth" => (Kind::Eth { contract: a20(&args[7])? }, &args[8..]),
                _ => return Err(anyhow!("KIND erc20|eth")),
            };
            let [dir, tx, log] = rest else { return Err(anyhow!("... DIR TX LOG")) };
            let (dir, tx, log): (&Path, u64, u64) = (Path::new(dir), tx.parse()?, log.parse()?);
            let config = Config { attest, instance_id: instance, evm_chain_id: chain, kind };
            let tag = format!("{}-{chain}", &args[6]);
            println!("# reference consumer {tag}: config PDA {}", eth_consumer::config_address(&consumer, &config));
            let init = msg(&CMsg::Init { config });
            std::fs::write(dir.join(format!("consumer-{tag}-init.hex")), hex::encode(&init))?;
            let (witness, proof) = receipt_material(dir, tx)?;
            let ctx = StatementContext { evm_chain_id: chain, instance_id: instance };
            let s = verify_receipt_log(&ctx, &witness, &proof, log).map_err(|e| anyhow!("{e}"))?;
            match eth_consumer::accept(&kind, &s.log) {
                Ok((from, amount, to)) => println!("  accepted natively: sender 0x{}, amount 0x{}, recipient 0x{}", hex::encode(from), hex::encode(amount), hex::encode(to)),
                Err(e) => println!("  NOT accepted natively: {e}"),
            }
            let claim = msg(&CMsg::Claim { config, witness: witness.clone(), proof, log_index: log });
            let null = eth_consumer::nullifier_address(&consumer, &config, &s.id());
            println!("  statement id 0x{}; nullifier PDA {null}; anchor PDA {}", hex::encode(s.id()), anchor_address(&attest, &instance, &witness.anchor));
            if chain == u64::from_str_radix("aa36a7", 16)? {
                let c = exec_one("consumer: the claim in the signer's actor", ETH_CONSUMER_GUEST_ELF, eth_consumer::handle, &root_input(signer, consumer, claim.clone()))?;
                let q = exec_one("attestation: AssertAnchor", ATTEST_GUEST_PUBLIC_ELF, eth_attest::handle, &anchor_query(attest, instance, chain, &witness.anchor, &record(instance, chain, witness.anchor), Actor::new(signer, consumer)))?;
                let sp = exec_one("consumer: nullifier PDA", ETH_CONSUMER_GUEST_ELF, eth_consumer::handle, &ReceiveInput {
                    receiver: Actor::new(null, consumer),
                    from: Some(Actor::new(signer, consumer)),
                    is_authorized: false,
                    pre_state: ActorState::empty(),
                    message: msg(&CMsg::Effect(eth_consumer::Effect::Spend { config_id: config.id(), statement_id: s.id() })),
                })?;
                let ac = exec_one("consumer: config PDA check", ETH_CONSUMER_GUEST_ELF, eth_consumer::handle, &ReceiveInput {
                    receiver: Actor::new(eth_consumer::config_address(&consumer, &config), consumer),
                    from: Some(Actor::new(signer, consumer)),
                    is_authorized: false,
                    pre_state: ActorState::from(msg(&config)),
                    message: msg(&CMsg::Query(eth_consumer::Query::AssertConfig { config })),
                })?;
                println!("  whole claim transaction {} user cycles; message {} bytes", c + q + sp + ac, claim.len());
            }
            std::fs::write(dir.join(format!("consumer-{tag}-claim-{tx}-{log}.hex")), hex::encode(&claim))?;
            println!("messages written: consumer-{tag}-init.hex, consumer-{tag}-claim-{tx}-{log}.hex");
            Ok(true)
        }
        _ => Err(anyhow!("stmt-log | stmt-account | consumer")),
    }
}
