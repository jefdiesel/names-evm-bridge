//! The cache never passes off a partial scan as complete, and survives a JSON round trip.

use bridge_client::{cache::ClientCache, chain::LezTx};

#[test]
fn a_scan_asked_to_start_earlier_than_the_cache_drops_the_cached_range() {
    let mut c = ClientCache::new([1; 32]);
    // First run starts late (e.g. --lez-from-block 5000) and caches 5000..6000.
    assert_eq!(c.lez_start(5_000), 5_000);
    c.lez_txs.push(LezTx { block: 5_500, bytes: vec![1] });
    c.lez_next_block = 6_000;
    // A later run from the default block 1 must rescan from 1, not resume at 6000.
    assert_eq!(c.lez_start(1), 1);
    assert!(c.lez_txs.is_empty());
    assert_eq!(c.lez_from_block, 1);
    // A run starting at or after the cached start resumes.
    c.lez_next_block = 6_000;
    assert_eq!(c.lez_start(1), 6_000);
    assert_eq!(c.lez_start(3_000), 6_000);
    // Same rule on the Ethereum side.
    assert_eq!(c.eth_start(100), 100);
    c.eth_next_block = 900;
    assert_eq!(c.eth_start(50), 50);
    c.eth_next_block = 900;
    assert_eq!(c.eth_start(60), 900);
}

#[test]
fn the_cache_round_trips_and_ignores_another_deployment() {
    let dir = std::env::temp_dir().join(format!("bridge-client-cache-{}", std::process::id()));
    let path = dir.join("c.json");
    let mut c = ClientCache::new([2; 32]);
    c.handed_out("dep/x", 4);
    c.handed_out("dep/x", 1);
    c.save(&path).unwrap();
    let back = ClientCache::load(&path, [2; 32]).unwrap();
    assert_eq!(back, c);
    assert_eq!(back.next["dep/x"], 5);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert_eq!(ClientCache::load(&path, [3; 32]).unwrap(), ClientCache::new([3; 32]));
    std::fs::write(&path, b"{not json").unwrap();
    assert!(ClientCache::load(&path, [2; 32]).is_err(), "the CLI sets an unreadable cache aside and rebuilds");
    std::fs::remove_dir_all(&dir).ok();
}
