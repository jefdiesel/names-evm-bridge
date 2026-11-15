// SPDX-License-Identifier: MIT OR Apache-2.0
//! Providers that prune old history (PublicNode's Sepolia RPC, 2026-11-14: "pruned history
//! unavailable" for the vault's first deposits) or answer with fewer logs than exist: the fetch
//! stops at once instead of retrying, the next RPC is used, and a short set of deposit logs is
//! refused against the vault's own `depositCount()` and `depositRoot()`. Synthetic data only.

use std::cell::Cell;

use bridge_client::{
    block_on,
    chain::{DepositEvent, EthLogSource, FetchPolicy, RawLog, RpcError, check_deposits_complete, decode_vault_logs, encode_deposit_log, fetch_logs, vault_topics},
};

const VAULT: [u8; 20] = [0x22; 20];
const CREATED: u64 = 1_000;

/// What a provider does with blocks below `kept_from`.
#[derive(Clone, Copy)]
enum Old {
    /// Serves everything.
    Full,
    /// Refuses any range reaching below `kept_from` (PublicNode's error).
    Refuses,
    /// Answers without the logs below `kept_from`, and no error.
    Drops,
}

struct Provider {
    logs: Vec<RawLog>,
    kept_from: u64,
    old: Old,
    calls: Cell<u32>,
    pauses: Cell<u32>,
}

impl Provider {
    fn new(deposits: &[DepositEvent], kept_from: u64, old: Old) -> Self {
        let logs = deposits.iter().enumerate().map(|(i, d)| encode_deposit_log(d, i as u64)).collect();
        Self { logs, kept_from, old, calls: Cell::new(0), pauses: Cell::new(0) }
    }
}

impl EthLogSource for Provider {
    async fn latest(&self) -> Result<u64, RpcError> {
        Ok(20_000)
    }
    async fn finalized(&self) -> Result<u64, RpcError> {
        Ok(19_930)
    }
    async fn logs(&self, _a: &[u8; 20], _t: &[[u8; 32]], from: u64, to: u64) -> Result<Vec<RawLog>, RpcError> {
        self.calls.set(self.calls.get() + 1);
        let floor = match self.old {
            Old::Refuses if from < self.kept_from => {
                return Err(RpcError::classify(r#"eth_getLogs: {"code":4444,"message":"pruned history unavailable"}"#));
            }
            Old::Drops => self.kept_from,
            _ => 0,
        };
        Ok(self.logs.iter().filter(|l| (from..=to).contains(&l.block) && l.block >= floor).cloned().collect())
    }
    async fn pause(&self, _attempt: u32) {
        self.pauses.set(self.pauses.get() + 1);
    }
}

/// Eight deposits: five just after the vault's creation, three much later (the shape of a demo
/// vault whose first deposits fell out of a pruning provider's window). Synthetic roots.
fn deposits() -> Vec<DepositEvent> {
    [1_017u64, 1_018, 1_019, 1_020, 1_021, 13_996, 16_826, 17_082]
        .iter()
        .enumerate()
        .map(|(i, b)| DepositEvent {
            token: [0; 20],
            denom_index: 0,
            commitment: [i as u8 + 1; 32],
            leaf_index: i as u64,
            root: [0xA0 + i as u8; 32],
            block: *b,
            tx_hash: [i as u8 + 0x10; 32],
        })
        .collect()
}

const COUNT: u64 = 8;
const ROOT: [u8; 32] = [0xA7; 32];

fn read(p: &Provider) -> Result<Vec<DepositEvent>, String> {
    let mut logs = Vec::new();
    block_on(fetch_logs(p, &VAULT, &vault_topics(), CREATED, 20_000, FetchPolicy { max_range: 2_000, retries: 5 }, |_, l| logs.extend(l)))
        .map_err(|e| e.to_string())?;
    Ok(decode_vault_logs(&logs).deposits)
}

#[test]
fn rate_limits_are_transient_and_pruned_history_is_fatal() {
    // thirdweb's rate limit shares -32005 with its range refusal; OnFinality says "Too Many Requests".
    for m in [
        r#"eth_getLogs: {"code":-32005,"message":"Rate limit exceeded. Upgrade your plan"}"#,
        r#"eth_getLogs: {"code":-32029,"message":"Too Many Requests, Please apply an API key"}"#,
    ] {
        assert!(matches!(RpcError::classify(m), RpcError::Transient(_)), "{m}");
    }
    let r = r#"eth_getLogs: {"code":-32005,"message":"Log response size exceeded. Maximum allowed number of requested blocks is 1000"}"#;
    assert!(matches!(RpcError::classify(r), RpcError::RangeTooLarge(_)));
    match RpcError::classify(r#"eth_getLogs: {"code":4444,"message":"pruned history unavailable"}"#) {
        RpcError::Fatal(m) => assert!(m.contains("pruned history unavailable") && m.contains("creation block"), "{m}"),
        other => panic!("pruned history must be fatal, got {other:?}"),
    }
}

#[test]
fn a_pruning_provider_stops_the_fetch_at_once_and_the_next_one_completes_it() {
    let pruned = Provider::new(&deposits(), 10_000, Old::Refuses);
    let err = read(&pruned).unwrap_err();
    assert!(err.contains("pruned history unavailable"), "{err}");
    assert_eq!(pruned.calls.get(), 1, "no retries against pruned history");
    assert_eq!(pruned.pauses.get(), 0);
    // The next RPC in the list serves the whole history: complete, as the vault counts.
    let full = Provider::new(&deposits(), 10_000, Old::Full);
    let got = read(&full).unwrap();
    assert_eq!(got, deposits());
    check_deposits_complete(&got, 20_000, COUNT, &ROOT).unwrap();
}

#[test]
fn silently_missing_old_logs_are_refused() {
    // The provider answers every range, but without the five early deposits.
    let p = Provider::new(&deposits(), 10_000, Old::Drops);
    let got = read(&p).unwrap();
    assert_eq!(got.len(), 3);
    let err = check_deposits_complete(&got, 20_000, COUNT, &ROOT).unwrap_err();
    assert!(err.contains("skip from leaf 0 to leaf 5"), "{err}");
}

#[test]
fn a_short_tail_or_another_root_is_refused() {
    let all = deposits();
    // The last two deposits missing: a valid-looking prefix tree, but not the vault's.
    let err = check_deposits_complete(&all[..6], 20_000, COUNT, &ROOT).unwrap_err();
    assert!(err.contains("hold 6 deposit(s)") && err.contains("depositCount() at block 20000 is 8"), "{err}");
    // Right count, different last root (a log altered or from another vault).
    let err = check_deposits_complete(&all, 20_000, COUNT, &[0xEE; 32]).unwrap_err();
    assert!(err.contains("is not the vault's depositRoot()"), "{err}");
    // More logs than the vault counts.
    assert!(check_deposits_complete(&all, 20_000, 7, &ROOT).is_err());
    // Deposits above the checked block are ignored: at block 16,900 the vault had counted seven.
    check_deposits_complete(&all, 16_900, 7, &all[6].root).unwrap();
    // An empty vault with no logs is complete.
    check_deposits_complete(&[], 20_000, 0, &[0; 32]).unwrap();
}
