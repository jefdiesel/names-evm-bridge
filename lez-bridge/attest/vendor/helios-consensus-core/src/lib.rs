pub mod consensus_spec;
pub mod errors;
pub mod types;

mod consensus_core;
mod proof;
mod utils;

pub use crate::consensus_core::*;
/// RFP-021 patch 2 (see proof.rs).
pub use crate::proof::sync_committee_root;
