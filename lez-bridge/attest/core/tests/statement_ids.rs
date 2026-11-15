//! Statement and fact identifiers (RFP-021 Functionality #5, #6): deterministic, equal for equal
//! inputs, different when any single field differs, and pinned against the Sepolia fixture
//! vector (`attest/fixtures/sepolia`: the OP messenger mapping entry the instance tests attest).
//!
//! The pinned literals were computed outside Rust (Python `hashlib.sha256` over the
//! concatenations documented on `StorageStatement::id`, `fact_id`, `AccountStatement::id`,
//! `LogStatement::id` and `instance_id`, with the instance config borsh-encoded by hand), so a
//! change to the encoding of any id fails here. These are integration tests on purpose: a
//! `#[cfg(test)]` module inside `src/` is never compiled into a dependent guest, but proving the
//! deployed bridge guest's image is unchanged would need a guest build, which this suite avoids.

use eth_attest_core::{
    Anchor, Fork, InstanceConfig, MIN_SUPERMAJORITY,
    inclusion::{AccountField, AccountStatement, Log, LogStatement, Predicate, StatementContext, StorageStatement, word},
    instance_id, sha256,
};
use lee_core::account::AccountId;

const ATTEST: AccountId = AccountId::new([0xE7; 32]);

fn h32(s: &str) -> [u8; 32] {
    let s = s.trim_start_matches("0x");
    let mut o = [0u8; 32];
    for (i, b) in o.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap();
    }
    o
}

fn h20(s: &str) -> [u8; 20] {
    h32(&format!("{:0>64}", s.trim_start_matches("0x")))[12..].try_into().unwrap()
}

/// `attest/fixtures/sepolia/meta.json` + `fork_schedule.json`, checkpoint `bootstrap_same_period`
/// (the same config `instance.rs` builds from the JSON).
fn sepolia_config() -> InstanceConfig {
    let v = |x: u32| x.to_be_bytes();
    let forks = [
        (0, 0x9000_0069),
        (50, 0x9000_0070),
        (100, 0x9000_0071),
        (56_832, 0x9000_0072),
        (132_608, 0x9000_0073),
        (222_464, 0x9000_0074),
        (272_640, 0x9000_0075),
        (353_024, 0x9000_0076),
    ]
    .map(|(epoch, version)| Fork { epoch, version: v(version) });
    InstanceConfig {
        evm_chain_id: 11_155_111,
        genesis_validators_root: h32("d8ea171f3c94aea21ebc42a1ed61052acf3f9209c00e4efbaaddac09ed9b8078"),
        genesis_time: 1_655_733_600,
        seconds_per_slot: 12,
        forks,
        checkpoint: h32("f54bd3575c05c7d83ee53dcef9bbfd98720b39e0be93aa9707a22dce8f2225d3"),
        min_participants: MIN_SUPERMAJORITY,
        salt: [0; 32],
    }
}

fn ctx() -> StatementContext {
    StatementContext { evm_chain_id: 11_155_111, instance_id: instance_id(&ATTEST, &sepolia_config()) }
}

const CONTRACT: &str = "58cc85b8d04ea49cc6dbd3cbffd00b4b8d6cb3ef";
const SLOT_KEY: &str = "4cd52750e4fc393f9bb4ad1efdb4cbe3e9de0379e51dbc148da8c429956e09b5";
const MAPPING_KEY: &str = "ce672ed0dc29e25537be0defe5d9f5fda6021998c3aef3a29aa43a52d7313df3";
const BLOCK_HASH: &str = "c637bfeee44c1685a0c8aec236d73876b7ab7cefdbea48b94d190296d11111f1";
const STORAGE_HASH: &str = "3cbbe7321b9753f7f740523ca467aa7294cd6a5a218cda4062bc5e9f645a94c5";

fn storage() -> StorageStatement {
    StorageStatement {
        context: ctx(),
        address: h20(CONTRACT),
        slot: h32(SLOT_KEY),
        predicate: Predicate::Eq(word(1)),
        anchor: Anchor::BlockHash(h32(BLOCK_HASH)),
    }
}

fn account() -> AccountStatement {
    AccountStatement {
        context: ctx(),
        address: h20(CONTRACT),
        field: AccountField::StorageRoot,
        predicate: Predicate::Eq(h32(STORAGE_HASH)),
        anchor: Anchor::BlockHash(h32(BLOCK_HASH)),
    }
}

fn log() -> LogStatement {
    LogStatement {
        context: ctx(),
        anchor: Anchor::BlockHash(h32(BLOCK_HASH)),
        tx_index: 5,
        log_index: 2,
        log: Log { address: h20(CONTRACT), topics: vec![h32(SLOT_KEY), h32(MAPPING_KEY)], data: b"RFP021 log data".to_vec() },
    }
}

/// Every element differs from every other one.
fn all_distinct(ids: &[[u8; 32]]) {
    for (i, a) in ids.iter().enumerate() {
        for (j, b) in ids.iter().enumerate().skip(i + 1) {
            assert_ne!(a, b, "variants {i} and {j} share an id");
        }
    }
}

#[test]
fn the_instance_id_of_the_fixture_config_is_pinned() {
    assert_eq!(instance_id(&ATTEST, &sepolia_config()), h32("778b1890283b84f27e52842b0efad701f52aba50950824bfa39beae07c8a6347"));
}

#[test]
fn ids_are_pinned_against_the_fixture_vector() {
    assert_eq!(storage().id(), h32("fe276c38ec744a62b452647483e326c4696de7179d949b687c16edd08f04c5b2"));
    assert_eq!(storage().fact_id(), h32("e934bd49c6575e46c1ad19082c959030b5fe28aa01c5cc366c6993ee026cb5f3"));
    assert_eq!(account().id(), h32("44e0eae122167607fefabb0dfb5e6c4f509e1a99d21e7e3ebac6932c626cca64"));
    assert_eq!(log().id(), h32("57dc8b33127d9697ea6665a2270a7b4a389127340945aa48e8d86d862ac85fa8"));
}

#[test]
fn equal_inputs_give_equal_ids() {
    // Built twice from scratch, not copied.
    assert_eq!(storage().id(), storage().id());
    assert_eq!(storage().fact_id(), storage().fact_id());
    assert_eq!(account().id(), account().id());
    assert_eq!(log().id(), log().id());
    // The three kinds never collide with each other on the same fact.
    all_distinct(&[storage().id(), storage().fact_id(), account().id(), log().id()]);
}

#[test]
fn a_storage_id_changes_with_every_field() {
    let base = storage();
    let mut v = vec![base.id()];
    let mut push = |f: &dyn Fn(&mut StorageStatement)| {
        let mut s = storage();
        f(&mut s);
        v.push(s.id());
    };
    push(&|s| s.context.evm_chain_id = 1);
    push(&|s| s.context.instance_id[31] ^= 1);
    push(&|s| s.address[19] ^= 1);
    push(&|s| s.slot[0] ^= 1);
    push(&|s| s.predicate = Predicate::Ne(word(1)));
    push(&|s| s.predicate = Predicate::Eq(word(2)));
    push(&|s| s.predicate = Predicate::Ne(word(0)));
    push(&|s| s.anchor = Anchor::StateRoot(h32(BLOCK_HASH)));
    push(&|s| s.anchor = Anchor::BlockHash(h32(STORAGE_HASH)));
    assert_eq!(v.len(), 10);
    all_distinct(&v);
}

#[test]
fn an_account_id_changes_with_every_field() {
    let mut v = vec![account().id()];
    let mut push = |f: &dyn Fn(&mut AccountStatement)| {
        let mut s = account();
        f(&mut s);
        v.push(s.id());
    };
    push(&|s| s.context.evm_chain_id = 1);
    push(&|s| s.context.instance_id[0] ^= 1);
    push(&|s| s.address[0] ^= 1);
    for field in [AccountField::Nonce, AccountField::Balance, AccountField::CodeHash] {
        push(&move |s| s.field = field);
    }
    push(&|s| s.predicate = Predicate::Ne(h32(STORAGE_HASH)));
    push(&|s| s.predicate = Predicate::Eq(word(0)));
    push(&|s| s.anchor = Anchor::StateRoot(h32(BLOCK_HASH)));
    push(&|s| s.anchor = Anchor::BlockHash(h32(SLOT_KEY)));
    assert_eq!(v.len(), 11);
    all_distinct(&v);
}

#[test]
fn a_log_id_changes_with_every_field() {
    let mut v = vec![log().id()];
    let mut push = |f: &dyn Fn(&mut LogStatement)| {
        let mut s = log();
        f(&mut s);
        v.push(s.id());
    };
    push(&|s| s.context.evm_chain_id = 1);
    push(&|s| s.context.instance_id[5] ^= 1);
    push(&|s| s.anchor = Anchor::StateRoot(h32(BLOCK_HASH)));
    push(&|s| s.anchor = Anchor::BlockHash(h32(SLOT_KEY)));
    push(&|s| s.tx_index = 6);
    push(&|s| s.log_index = 3);
    // Position fields swapped: still another id.
    push(&|s| (s.tx_index, s.log_index) = (2, 5));
    push(&|s| s.log.address[7] ^= 1);
    push(&|s| s.log.topics[1][0] ^= 1);
    push(&|s| s.log.topics.reverse());
    push(&|s| {
        s.log.topics.pop();
    });
    push(&|s| s.log.topics.clear());
    push(&|s| s.log.data.push(0));
    push(&|s| s.log.data.clear());
    // Moving a topic's bytes into the data is another log (the topic count is in the id).
    push(&|s| {
        let t = s.log.topics.pop().unwrap();
        let mut d = t.to_vec();
        d.extend_from_slice(&s.log.data);
        s.log.data = d;
    });
    assert_eq!(v.len(), 16);
    all_distinct(&v);
}

#[test]
fn a_fact_id_ignores_anchor_and_instance_but_not_the_chain_or_the_fact() {
    let base = storage();
    let same = |f: &dyn Fn(&mut StorageStatement)| {
        let mut s = storage();
        f(&mut s);
        assert_eq!(s.fact_id(), base.fact_id());
        assert_ne!(s.id(), base.id(), "the statement id still binds anchor and instance");
    };
    same(&|s| s.anchor = Anchor::StateRoot(h32(STORAGE_HASH)));
    same(&|s| s.anchor = Anchor::BlockHash(h32(MAPPING_KEY)));
    same(&|s| s.context.instance_id = [0x42; 32]);
    same(&|s| {
        s.context.instance_id = [0x43; 32];
        s.anchor = Anchor::StateRoot([9; 32]);
    });
    let mut v = vec![base.fact_id()];
    let mut differ = |f: &dyn Fn(&mut StorageStatement)| {
        let mut s = storage();
        f(&mut s);
        v.push(s.fact_id());
    };
    differ(&|s| s.context.evm_chain_id = 1);
    differ(&|s| s.context.evm_chain_id = 17_000);
    differ(&|s| s.address[0] ^= 1);
    differ(&|s| s.slot[31] ^= 1);
    differ(&|s| s.predicate = Predicate::Ne(word(1)));
    differ(&|s| s.predicate = Predicate::Eq(word(0)));
    all_distinct(&v);
}

#[test]
fn ids_follow_the_documented_concatenation() {
    // The formulas on the id methods, recomputed here from their doc comments.
    let s = storage();
    let chain = 11_155_111u64.to_be_bytes();
    assert_eq!(
        s.id(),
        sha256(&[b"RFP021/statement/storage/v1", &chain, &s.context.instance_id, &s.address, &s.slot, &[1], &word(1), &[1], &h32(BLOCK_HASH)])
    );
    assert_eq!(s.fact_id(), sha256(&[b"RFP021/fact/storage/v1", &chain, &s.address, &s.slot, &[1], &word(1)]));
    let mut ne = s;
    ne.predicate = Predicate::Ne(word(1));
    ne.anchor = Anchor::StateRoot(h32(BLOCK_HASH));
    assert_eq!(
        ne.id(),
        sha256(&[b"RFP021/statement/storage/v1", &chain, &s.context.instance_id, &s.address, &s.slot, &[2], &word(1), &[2], &h32(BLOCK_HASH)])
    );
}
