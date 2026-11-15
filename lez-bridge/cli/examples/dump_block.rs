//! Print a LEZ block in full (Debug), from the raw borsh bytes of a `getBlock` result
//! (base64-decoded first). Inspection only: shows what a block makes public.
//!   cargo run --release --example dump_block -- block-42.bin ...
use common::block::Block;

fn main() {
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).expect("read the block file");
        let block: Block = borsh::from_slice(&bytes).expect("decode a common::block::Block");
        println!("=== {path}\n{block:#?}");
    }
}
