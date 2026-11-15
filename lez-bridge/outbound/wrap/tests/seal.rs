// SPDX-License-Identifier: MIT OR Apache-2.0
//! The seal encoding and native Groth16 verification, against RISC Zero's own 3.1 test receipt:
//! risc0-ethereum v3.1.0 `contracts/test/TestReceiptV3_1.sol` (Copyright 2025 RISC Zero, Inc.,
//! Apache-2.0; generated there by `cargo xtask bootstrap-groth16`). The same seal passes
//! `verify` on the Sepolia RiscZeroGroth16Verifier 3.1.0 (`public-demo/V4-PLAN.md`; the v4 fork test).
//! Nothing here proves or wraps.

use outbound_wrap::*;
use risc0_zkvm::{
    FakeReceipt, Groth16Receipt, Groth16ReceiptVerifierParameters, InnerReceipt, MaybePruned, Receipt, ReceiptClaim,
    sha::{Digest, Digestible as _},
};

const SEAL: &str = "e71e662021a0c81a5cc1c5cddabc3eefe6a3841315ad1a1cd74da3f34db1db91d01240a70b220d069e774da52f06c0d0ca166b219f0e1b0548a08270fa88f5e71a55bb2911fba8d9c52968295894f9b6e1804afcda16b66956ad973a290cb8d6736813502b336b4ec0382ab1a7508781f3e1a8d7ea73d4491382cae39ec9f14aff07f53623bc5d3d6a718a686588bd6027b1713110031c85fa34117a4fed94d98a41fe8e0bcf572d3389eedcc34ec00bd1aa571b0bcaa80af13b30bf3cb26741e56deabb23d1abf7efdfba3f55523687ce10ef220c8d6d879f2e9161684f044e6e3597a2087d050b687d2242476dde08f789d726635f51ccd6fc216091297f0b7b416a6a";
const JOURNAL: &str = "6a75737420612073696d706c652072656365697074"; // "just a simple receipt"
const IMAGE_ID: &str = "c92414d80cdeb43ba0f4ff4882447c21f18b547d41ffa506a84da164e8e31f7d";

fn image() -> [u8; 32] {
    hex::decode(IMAGE_ID).unwrap().try_into().unwrap()
}

/// The test receipt as risc0-zkvm 3.1.0 holds it: the 256-byte seal after the selector, the claim
/// ok(image, sha256(journal)), this version's verifier parameters.
fn test_receipt(journal: &[u8]) -> Receipt {
    let seal = hex::decode(SEAL).unwrap();
    let claim = ReceiptClaim::ok(Digest::from_bytes(image()), MaybePruned::Pruned(Digest::from(sha256(journal))));
    let g = Groth16Receipt::new(seal[4..].to_vec(), MaybePruned::Value(claim), Groth16ReceiptVerifierParameters::default().digest());
    Receipt::new(InnerReceipt::Groth16(g), journal.to_vec())
}

#[test]
fn the_selector_is_3_1_0s() {
    assert_eq!(selector(), SELECTOR_V3_1);
    assert_eq!(hex::decode(SEAL).unwrap()[..4], SELECTOR_V3_1);
}

#[test]
fn risc0s_3_1_test_receipt_verifies_natively_and_encodes_to_the_published_seal() {
    let j = hex::decode(JOURNAL).unwrap();
    let r = test_receipt(&j);
    assert_eq!(check(&r, image()).unwrap(), "groth16");
    let seal = encode_seal(&r).unwrap();
    assert_eq!(seal.len(), 260);
    assert_eq!(hex::encode(&seal), SEAL);
    // borsh round trip (the groth16.bin file)
    let f = std::env::temp_dir().join(format!("g16-{}.bin", std::process::id()));
    save(&r, &f).unwrap();
    assert_eq!(encode_seal(&load(&f).unwrap()).unwrap(), seal);
    std::fs::remove_file(&f).unwrap();
    // the read-only on-chain call carries exactly these values
    let call = verify_call(&seal, &image(), &sha256(&j));
    assert!(call.starts_with("cast call 0x8c915b5071206C6E328BC18529586c513408aB3b \"verify(bytes,bytes32,bytes32)\" 0xe71e6620"));
    assert!(call.contains(&format!("0x{IMAGE_ID}")));
}

#[test]
fn a_changed_journal_image_or_seal_does_not_verify() {
    let j = hex::decode(JOURNAL).unwrap();
    // another journal under the same seal
    assert!(check(&test_receipt(b"just a simple receipT"), image()).is_err());
    // another image id
    let mut other = image();
    other[0] ^= 1;
    assert!(check(&test_receipt(&j), other).is_err());
    // one seal byte changed
    let mut r = test_receipt(&j);
    if let InnerReceipt::Groth16(g) = &mut r.inner {
        g.seal[40] ^= 1;
    }
    assert!(check(&r, image()).is_err());
}

#[test]
fn only_groth16_receipts_encode_and_fakes_are_refused() {
    let j = hex::decode(JOURNAL).unwrap();
    let claim = ReceiptClaim::ok(Digest::from_bytes(image()), MaybePruned::Pruned(Digest::from(sha256(&j))));
    let fake = Receipt::new(InnerReceipt::Fake(FakeReceipt::new(claim)), j);
    assert!(check(&fake, image()).unwrap_err().to_string().contains("fake"));
    assert!(encode_seal(&fake).is_err());
}

#[test]
fn the_gate_refuses_dev_mode_and_remote_provers() {
    assert!(gate(|_| None).is_ok());
    for k in ["RISC0_DEV_MODE", "RISC0_SERVER_PATH", "BONSAI_API_URL", "BONSAI_API_KEY"] {
        assert!(gate(|v| (v == k).then(|| "1".to_string())).is_err(), "{k}");
    }
}
