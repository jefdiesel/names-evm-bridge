//! Range splitting, retry and resume against fakes that refuse large ranges and fail
//! intermittently (RFP-022 Reliability #6).

use std::cell::{Cell, RefCell};

use bridge_client::{
    block_on,
    chain::{EthLogSource, FetchPolicy, Interrupted, LezBlockSource, LezTx, RawLog, RpcError, fetch_logs, scan_lez},
};
use lee_core::account::AccountId;

const VAULT: [u8; 20] = [0x11; 20];

/// Logs at the given blocks; refuses ranges above `max_range`, fails every `fail_every`-th call,
/// and after `dead_after` calls fails for good.
struct FakeEth {
    logs: Vec<RawLog>,
    max_range: u64,
    fail_every: u64,
    dead_after: u64,
    calls: Cell<u64>,
    pauses: Cell<u32>,
}

impl FakeEth {
    fn new(blocks: &[u64], max_range: u64, fail_every: u64, dead_after: u64) -> Self {
        let logs = blocks
            .iter()
            .enumerate()
            .map(|(i, b)| RawLog { block: *b, tx_hash: [i as u8; 32], log_index: i as u64, topics: vec![[1; 32]], data: vec![] })
            .collect();
        Self { logs, max_range, fail_every, dead_after, calls: Cell::new(0), pauses: Cell::new(0) }
    }
}

impl EthLogSource for FakeEth {
    async fn latest(&self) -> Result<u64, RpcError> {
        Ok(10_000)
    }
    async fn finalized(&self) -> Result<u64, RpcError> {
        Ok(9_990)
    }
    async fn logs(&self, _a: &[u8; 20], _t: &[[u8; 32]], from: u64, to: u64) -> Result<Vec<RawLog>, RpcError> {
        let n = self.calls.get() + 1;
        self.calls.set(n);
        if n > self.dead_after {
            return Err(RpcError::Transient("connection refused".into()));
        }
        if self.fail_every > 0 && n % self.fail_every == 0 {
            return Err(RpcError::Transient("timeout".into()));
        }
        if to - from + 1 > self.max_range {
            return Err(RpcError::classify("query exceeds max block range 500"));
        }
        // A sloppy provider may return a log outside the range: the fetcher drops it.
        let mut out: Vec<RawLog> = self.logs.iter().filter(|l| (from..=to).contains(&l.block)).cloned().collect();
        if let Some(extra) = self.logs.iter().find(|l| l.block == to + 1) {
            out.push(extra.clone());
        }
        Ok(out)
    }
    async fn pause(&self, _attempt: u32) {
        self.pauses.set(self.pauses.get() + 1);
    }
}

/// Sorted, with gaps and some blocks holding two logs.
fn blocks() -> Vec<u64> {
    let mut b: Vec<u64> = (0..3_000).map(|i| i * 3 + (i % 7)).collect();
    b.sort_unstable();
    b
}

#[test]
fn classify_sorts_provider_errors() {
    for m in ["query returned more than 10000 results", "block range is too large", "Log response size exceeded", "-32005 limit exceeded"] {
        assert!(matches!(RpcError::classify(m), RpcError::RangeTooLarge(_)), "{m}");
    }
    assert!(matches!(RpcError::classify("connection reset by peer"), RpcError::Transient(_)));
}

#[test]
fn ranges_split_and_transient_failures_retry_without_duplicates() {
    let src = FakeEth::new(&blocks(), 500, 4, u64::MAX);
    let got = RefCell::new(Vec::new());
    let mut next_seen = 0;
    let requests = block_on(fetch_logs(&src, &VAULT, &[[1; 32]], 0, 9_100, FetchPolicy { max_range: 4_000, retries: 3 }, |next, logs| {
        assert!(next > next_seen, "progress goes forward");
        next_seen = next;
        got.borrow_mut().extend(logs);
    }))
    .unwrap();
    let got = got.into_inner();
    let want: Vec<u64> = blocks().into_iter().filter(|b| *b <= 9_100).collect();
    assert_eq!(got.iter().map(|l| l.block).collect::<Vec<_>>(), want, "every log once, in order");
    assert_eq!(next_seen, 9_101);
    assert!(src.pauses.get() > 0, "transient failures were retried");
    assert!(requests > 9_101 / 500, "split to the provider's limit");
}

#[test]
fn an_interrupted_fetch_resumes_where_it_stopped() {
    let all: Vec<u64> = blocks().into_iter().filter(|b| *b <= 9_000).collect();
    let mut kept: Vec<RawLog> = Vec::new();
    let mut checkpoint = 0u64;
    // The provider dies after 7 calls: the fetch stops, everything before the checkpoint is kept.
    let dying = FakeEth::new(&blocks(), 1_000, 0, 7);
    let err: Interrupted = block_on(fetch_logs(&dying, &VAULT, &[[1; 32]], 0, 9_000, FetchPolicy { max_range: 1_000, retries: 2 }, |next, logs| {
        checkpoint = next;
        kept.extend(logs);
    }))
    .unwrap_err();
    assert_eq!(err.next_block, checkpoint);
    assert!(checkpoint > 0 && checkpoint < 9_000);
    assert!(kept.iter().all(|l| l.block < checkpoint));
    // Rerun from the checkpoint with a healthy provider: the union is exact, nothing twice.
    let healthy = FakeEth::new(&blocks(), 1_000, 0, u64::MAX);
    block_on(fetch_logs(&healthy, &VAULT, &[[1; 32]], checkpoint, 9_000, FetchPolicy::default(), |_, logs| kept.extend(logs))).unwrap();
    assert_eq!(kept.iter().map(|l| l.block).collect::<Vec<_>>(), all);
}

#[test]
fn a_range_refused_even_at_one_block_stops_cleanly() {
    let src = FakeEth::new(&blocks(), 0, 0, u64::MAX);
    let err = block_on(fetch_logs(&src, &VAULT, &[], 5, 10, FetchPolicy::default(), |_, _| panic!("no progress"))).unwrap_err();
    assert_eq!(err.next_block, 5);
}

/// LEZ blocks: each holds (bytes, reverted); block 13 fails twice before answering.
struct FakeLez {
    blocks: Vec<Vec<(Vec<u8>, bool)>>,
    flaky: Cell<u32>,
}

impl LezBlockSource for FakeLez {
    async fn tip(&self) -> Result<u64, RpcError> {
        Ok(self.blocks.len() as u64 - 1)
    }
    async fn block_txs(&self, id: u64) -> Result<Option<Vec<(Vec<u8>, bool)>>, RpcError> {
        if id == 13 && self.flaky.get() < 2 {
            self.flaky.set(self.flaky.get() + 1);
            return Err(RpcError::Transient("503".into()));
        }
        Ok(self.blocks.get(id as usize).cloned())
    }
    async fn account_exists(&self, _a: &AccountId, _p: &AccountId) -> Result<bool, RpcError> {
        Ok(false)
    }
}

#[test]
fn the_lez_scan_keeps_applied_bridge_transactions_only_and_resumes() {
    let config = [0xCF; 32];
    let mut blocks = vec![vec![]; 30];
    blocks[5] = vec![([&[1u8][..], &config, &[2]].concat(), false), (vec![9; 40], false)];
    blocks[13] = vec![([&config[..], &[3]].concat(), true), ([&config[..], &[4]].concat(), false)];
    blocks[29] = vec![(config.to_vec(), false)];
    let src = FakeLez { blocks, flaky: Cell::new(0) };
    let mut kept: Vec<LezTx> = Vec::new();
    let mut next = 0;
    // First pass stops at 20, as an interrupted run would; the second resumes from 21.
    block_on(scan_lez(&src, &config, 0, 20, 3, |n, txs| {
        next = n;
        kept.extend(txs);
    }))
    .unwrap();
    assert_eq!(next, 21);
    block_on(scan_lez(&src, &config, next, 29, 3, |_, txs| kept.extend(txs))).unwrap();
    assert_eq!(kept.iter().map(|t| (t.block, t.bytes.last().copied())).collect::<Vec<_>>(), vec![(5, Some(2)), (13, Some(4)), (29, Some(0xCF))]);
    assert_eq!(src.flaky.get(), 2);
    // A block the sequencer does not have stops the scan with a checkpoint.
    let err = block_on(scan_lez(&src, &config, 28, 40, 3, |_, _| {})).unwrap_err();
    assert_eq!(err.next_block, 30);
}
