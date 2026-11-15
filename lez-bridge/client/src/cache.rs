//! The client's local cache: a convenience, never a source of truth (crate docs, "Client
//! state"). Deleting it loses nothing: `recover` rebuilds every field from the wallet and the
//! chains. One JSON file per deployment, mode 600, written atomically.

use std::{collections::BTreeMap, path::Path};

use serde::{Deserialize, Serialize};

use crate::{
    chain::{LezTx, RawLog},
    hexser,
    scan::Recovery,
};

pub const CACHE_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientCache {
    pub version: u32,
    #[serde(with = "hexser::arr")]
    pub cfg_id: [u8; 32],
    /// Next index per sequence key ([`crate::scan::dep_key`], [`crate::scan::exit_key`]),
    /// including indexes handed out but not (yet) seen on chain.
    pub next: BTreeMap<String, u64>,
    /// Vault logs of blocks `eth_from_block..eth_next_block` (finalized blocks only).
    #[serde(default)]
    pub eth_from_block: u64,
    pub eth_next_block: u64,
    pub eth_logs: Vec<RawLog>,
    /// Non-reverted LEZ transactions naming the bridge config, of blocks
    /// `lez_from_block..lez_next_block`.
    #[serde(default)]
    pub lez_from_block: u64,
    pub lez_next_block: u64,
    pub lez_txs: Vec<LezTx>,
    /// The last scan's result (indexes and public values only).
    pub last: Option<Recovery>,
}

impl ClientCache {
    #[must_use]
    pub fn new(cfg_id: [u8; 32]) -> Self {
        Self { version: CACHE_VERSION, cfg_id, ..Self::default() }
    }

    /// The cache at `path`, or a new one if there is none (or it is for another deployment).
    ///
    /// # Errors
    /// An unreadable or unparsable file.
    pub fn load(path: &Path, cfg_id: [u8; 32]) -> Result<Self, String> {
        match std::fs::read(path) {
            Ok(bytes) => {
                let c: Self = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
                Ok(if c.version == CACHE_VERSION && c.cfg_id == cfg_id { c } else { Self::new(cfg_id) })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::new(cfg_id)),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Writes the cache to `path` (a new file renamed over it; mode 600).
    ///
    /// # Errors
    /// I/O.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        use std::io::Write as _;
        #[cfg(unix)]
        use std::os::unix::fs::OpenOptionsExt as _;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut f = options.open(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        f.write_all(&serde_json::to_vec(self).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Where the next Ethereum fetch starts for a scan asked to begin at `from`. A cached range
    /// that began later than `from` is dropped (it would hide the blocks before it).
    pub fn eth_start(&mut self, from: u64) -> u64 {
        if self.eth_next_block == 0 || from < self.eth_from_block {
            self.eth_logs.clear();
            self.eth_from_block = from;
            self.eth_next_block = from;
        }
        self.eth_next_block
    }

    /// As [`ClientCache::eth_start`], for LEZ blocks.
    pub fn lez_start(&mut self, from: u64) -> u64 {
        if self.lez_next_block == 0 || from < self.lez_from_block {
            self.lez_txs.clear();
            self.lez_from_block = from;
            self.lez_next_block = from;
        }
        self.lez_next_block
    }

    /// Records that `index` of `key` was handed out.
    pub fn handed_out(&mut self, key: &str, index: u64) {
        let n = self.next.entry(key.to_owned()).or_default();
        *n = (*n).max(index + 1);
    }
}
