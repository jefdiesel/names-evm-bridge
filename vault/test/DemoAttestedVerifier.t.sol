// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Test, console} from "forge-std/Test.sol";
import {DemoAttestedVerifier as DAV} from "../src/demo/DemoAttestedVerifier.sol";
import {MockVerifier} from "./mocks/Mocks.sol";

/// Unit tests of the operator-attested DEMO verifier (`src/demo/DemoAttestedVerifier.sol`):
/// what it accepts, every refusal and its error, fuzzing, the two signing vectors the Rust
/// attestor must reproduce (also in `src/demo/README.md`), and the gas of `verify`.
contract DemoAttestedVerifierTest is Test {
    // Anvil / Hardhat account 0: a PUBLIC test key, used here only so the vectors are reproducible.
    uint256 constant ANVIL0_PK = 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80;
    address constant ANVIL0 = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;
    uint256 constant LIFETIME = 180 days;
    uint256 constant N = 0xfffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141;

    bytes32 constant IMG = keccak256("some-image");
    bytes32 constant DIG = keccak256("some-journal-digest");

    bytes4 constant PREFIX = 0x7fa98d9a;

    DAV d;
    uint256 attPk = ANVIL0_PK;
    address att = ANVIL0;

    function setUp() public {
        vm.warp(1_763_000_000); // a realistic 2025/2026 timestamp, so expiry arithmetic is real
        d = new DAV(att, LIFETIME);
    }

    // ------------------------------------------------------------------ helpers

    function _sign(DAV ver, uint256 pk, bytes32 img, bytes32 dig) internal view returns (bytes memory) {
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(pk, ver.attestationDigest(img, dig));
        return abi.encodePacked(ver.SEAL_PREFIX(), r, s, v);
    }

    /// No external call here: it is used inside expectRevert'd calls (the cheatcode applies to
    /// the next external call).
    function _seal(bytes32 r, bytes32 s, uint8 v) internal pure returns (bytes memory) {
        return abi.encodePacked(PREFIX, r, s, v);
    }

    function _parts(bytes memory seal) internal pure returns (bytes32 r, bytes32 s, uint8 v) {
        assembly {
            r := mload(add(seal, 36))
            s := mload(add(seal, 68))
            v := byte(0, mload(add(seal, 100)))
        }
    }

    // ------------------------------------------------------------------ accepted

    function test_ValidAttestationAccepted() public view {
        d.verify(_sign(d, attPk, IMG, DIG), IMG, DIG);
    }

    function test_Views() public view {
        assertEq(d.ATTESTOR(), att);
        assertEq(d.EXPIRES_AT(), block.timestamp + LIFETIME);
        assertEq(d.KIND(), "demo-attested: an operator checked the proof off chain; NOT trustless");
        assertEq(d.VERSION(), "1.0.0");
        assertEq(d.TAG(), keccak256("RFP022/demo-attested-verifier/v1"));
        assertEq(d.SEAL_PREFIX(), bytes4(0x7fa98d9a));
        assertTrue(d.SEAL_PREFIX() != d.RISC_ZERO_GROTH16_SELECTOR());
        bytes memory e = d.encodeSeal(bytes32(uint256(1)), bytes32(uint256(2)), 27);
        assertEq(e, abi.encodePacked(bytes4(0x7fa98d9a), bytes32(uint256(1)), bytes32(uint256(2)), uint8(27)));
    }

    /// The digest is the standard EIP-712 one, built independently here.
    function test_DigestIsEip712() public view {
        bytes32 ds = keccak256(
            abi.encode(
                keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"),
                keccak256("RFP022 DemoAttestedVerifier"),
                keccak256("1"),
                block.chainid,
                address(d)
            )
        );
        assertEq(d.domainSeparator(), ds);
        bytes32 sh = keccak256(
            abi.encode(
                keccak256("DemoAttestation(bytes32 tag,bytes32 imageId,bytes32 journalDigest)"),
                keccak256("RFP022/demo-attested-verifier/v1"),
                IMG,
                DIG
            )
        );
        assertEq(d.attestationDigest(IMG, DIG), keccak256(abi.encodePacked(bytes2(0x1901), ds, sh)));
    }

    function testFuzz_AnyPairSignedByAttestorAccepted(bytes32 img, bytes32 dig) public view {
        d.verify(_sign(d, attPk, img, dig), img, dig);
    }

    // ------------------------------------------------------------------ constructor

    function test_ConstructorRefusals() public {
        vm.expectRevert(DAV.ZeroAttestor.selector);
        new DAV(address(0), LIFETIME);
        vm.expectRevert(DAV.BadLifetime.selector);
        new DAV(att, 0);
        vm.expectRevert(DAV.BadLifetime.selector);
        new DAV(att, 365 days + 1);
        DAV ok = new DAV(att, 365 days);
        assertEq(ok.EXPIRES_AT(), block.timestamp + 365 days);
    }

    // ------------------------------------------------------------------ wrong statement

    function test_WrongAttestorRejected() public {
        bytes memory seal = _sign(d, uint256(keccak256("someone else")), IMG, DIG);
        vm.expectRevert(abi.encodeWithSelector(DAV.NotAttestor.selector, vm.addr(uint256(keccak256("someone else")))));
        d.verify(seal, IMG, DIG);
    }

    function test_WrongChainIdRejected() public {
        bytes memory seal = _sign(d, attPk, IMG, DIG);
        d.verify(seal, IMG, DIG);
        vm.chainId(block.chainid + 1);
        vm.expectPartialRevert(DAV.NotAttestor.selector);
        d.verify(seal, IMG, DIG);
    }

    function test_WrongVerifierAddressRejected() public {
        DAV other = new DAV(att, LIFETIME); // same attestor, second instance
        bytes memory seal = _sign(other, attPk, IMG, DIG);
        other.verify(seal, IMG, DIG);
        vm.expectPartialRevert(DAV.NotAttestor.selector);
        d.verify(seal, IMG, DIG);
    }

    function test_WrongImageIdRejected() public {
        bytes memory seal = _sign(d, attPk, IMG, DIG);
        vm.expectPartialRevert(DAV.NotAttestor.selector);
        d.verify(seal, keccak256("other-image"), DIG);
    }

    function test_WrongDigestRejected() public {
        bytes memory seal = _sign(d, attPk, IMG, DIG);
        vm.expectPartialRevert(DAV.NotAttestor.selector);
        d.verify(seal, IMG, bytes32(uint256(DIG) ^ 1));
    }

    /// Update and release images are distinct statements: a release attestation never passes as
    /// an update with the same digest.
    function test_ImageIdsAreNotInterchangeable() public {
        bytes32 g2 = keccak256("g2");
        bytes32 g3 = keccak256("g3");
        bytes memory seal = _sign(d, attPk, g3, DIG);
        vm.expectPartialRevert(DAV.NotAttestor.selector);
        d.verify(seal, g2, DIG);
    }

    // ------------------------------------------------------------------ expiry

    function test_Expired() public {
        bytes memory seal = _sign(d, attPk, IMG, DIG);
        uint256 exp = d.EXPIRES_AT();
        vm.warp(exp - 1);
        d.verify(seal, IMG, DIG);
        vm.warp(exp);
        vm.expectRevert(abi.encodeWithSelector(DAV.Expired.selector, exp));
        d.verify(seal, IMG, DIG);
        vm.warp(exp + 365 days);
        vm.expectRevert(abi.encodeWithSelector(DAV.Expired.selector, exp));
        d.verify(seal, IMG, DIG);
    }

    // ------------------------------------------------------------------ malformed signatures

    function test_HighSRejected() public {
        (bytes32 r, bytes32 s, uint8 v) = _parts(_sign(d, attPk, IMG, DIG));
        // the malleable twin (r, n - s, v ^ 1) recovers the same signer; it must be refused
        bytes32 s2 = bytes32(N - uint256(s));
        uint8 v2 = v == 27 ? 28 : 27;
        assertEq(ecrecover(d.attestationDigest(IMG, DIG), v2, r, s2), att, "twin recovers the attestor");
        vm.expectRevert(DAV.HighS.selector);
        d.verify(_seal(r, s2, v2), IMG, DIG);
    }

    function test_BadVRejected() public {
        (bytes32 r, bytes32 s, uint8 v) = _parts(_sign(d, attPk, IMG, DIG));
        uint8[5] memory bad = [0, 1, 26, 29, 255];
        for (uint256 i; i < bad.length; ++i) {
            vm.expectRevert(abi.encodeWithSelector(DAV.BadV.selector, bad[i]));
            d.verify(_seal(r, s, bad[i]), IMG, DIG);
        }
        // control: the right v passes
        d.verify(_seal(r, s, v), IMG, DIG);
    }

    function test_WrongLengthRejected() public {
        bytes memory good = _sign(d, attPk, IMG, DIG);
        (bytes32 r, bytes32 s, uint8 v) = _parts(good);
        // the bare 65-byte signature, without the prefix
        bytes memory bare = abi.encodePacked(r, s, v);
        vm.expectRevert(abi.encodeWithSelector(DAV.BadSealLength.selector, 65));
        d.verify(bare, IMG, DIG);
        uint256[6] memory lens = [uint256(0), 3, 4, 68, 70, 260];
        for (uint256 i; i < lens.length; ++i) {
            bytes memory x = new bytes(lens[i]);
            for (uint256 k; k < x.length && k < good.length; ++k) x[k] = good[k];
            vm.expectRevert(abi.encodeWithSelector(DAV.BadSealLength.selector, lens[i]));
            d.verify(x, IMG, DIG);
        }
        // the good seal with one extra byte
        vm.expectRevert(abi.encodeWithSelector(DAV.BadSealLength.selector, 70));
        d.verify(abi.encodePacked(good, bytes1(0)), IMG, DIG);
    }

    function test_UnknownPrefixRejected() public {
        (bytes32 r, bytes32 s, uint8 v) = _parts(_sign(d, attPk, IMG, DIG));
        vm.expectRevert(abi.encodeWithSelector(DAV.UnknownSealPrefix.selector, bytes4(0xdeadbeef)));
        d.verify(abi.encodePacked(bytes4(0xdeadbeef), r, s, v), IMG, DIG);
    }

    function test_ZeroRecoverRejected() public {
        // r = 0 is not a curve point's x: ecrecover returns address(0)
        vm.expectRevert(DAV.InvalidSignature.selector);
        d.verify(_seal(bytes32(0), bytes32(uint256(1)), 27), IMG, DIG);
        // r >= n is invalid too
        vm.expectRevert(DAV.InvalidSignature.selector);
        d.verify(_seal(bytes32(N), bytes32(uint256(1)), 28), IMG, DIG);
        // s = 0
        (bytes32 r,,) = _parts(_sign(d, attPk, IMG, DIG));
        vm.expectRevert(DAV.InvalidSignature.selector);
        d.verify(_seal(r, bytes32(0), 27), IMG, DIG);
    }

    /// A seal for the real RISC Zero Groth16 verifier (selector 0x73c457ba, 4 + 256 bytes) is
    /// refused with its own error, whatever its length, so nobody can read a success here as
    /// "the zk proof checked out".
    function test_RealRiscZeroSelectorRejected() public {
        bytes memory groth16 = abi.encodePacked(bytes4(0x73c457ba), new bytes(256));
        vm.expectRevert(DAV.ZkProofNotCheckedHere.selector);
        d.verify(groth16, IMG, DIG);
        (bytes32 r, bytes32 s, uint8 v) = _parts(_sign(d, attPk, IMG, DIG));
        vm.expectRevert(DAV.ZkProofNotCheckedHere.selector);
        d.verify(abi.encodePacked(bytes4(0x73c457ba), r, s, v), IMG, DIG);
        vm.expectRevert(DAV.ZkProofNotCheckedHere.selector);
        d.verify(hex"73c457ba", IMG, DIG);
    }

    // ------------------------------------------------------------------ fuzz: forgeries never pass

    /// Raw random bytes (mostly stopped by the length check).
    function testFuzz_RandomSealNeverPasses(bytes calldata seal, bytes32 img, bytes32 dig) public {
        vm.expectRevert();
        d.verify(seal, img, dig);
    }

    /// Well-formed seals (right prefix and length, v in {27, 28}, low s) with random r and s:
    /// they get past every format check and must still fail the signer check.
    function testFuzz_WellFormedRandomSignatureNeverPasses(bytes32 r, uint256 s, bool vHigh, bytes32 img, bytes32 dig)
        public
    {
        s = bound(s, 1, N / 2);
        uint8 v = vHigh ? 28 : 27;
        bytes32 h = d.attestationDigest(img, dig);
        address rec = ecrecover(h, v, r, bytes32(s));
        if (rec == address(0)) vm.expectRevert(DAV.InvalidSignature.selector);
        else vm.expectRevert(abi.encodeWithSelector(DAV.NotAttestor.selector, rec));
        d.verify(_seal(r, bytes32(s), v), img, dig);
    }

    /// Any key other than the attestor, signing exactly the right digest, is refused.
    function testFuzz_OtherSignerNeverPasses(uint256 pk, bytes32 img, bytes32 dig) public {
        pk = bound(pk, 1, N - 1);
        vm.assume(pk != attPk);
        bytes memory seal = _sign(d, pk, img, dig);
        vm.expectRevert(abi.encodeWithSelector(DAV.NotAttestor.selector, vm.addr(pk)));
        d.verify(seal, img, dig);
    }

    /// A valid seal with any single byte changed never passes.
    function testFuzz_BitFlipNeverPasses(uint256 pos, uint8 mask) public {
        vm.assume(mask != 0);
        bytes memory seal = _sign(d, attPk, IMG, DIG);
        pos = bound(pos, 0, seal.length - 1);
        seal[pos] = bytes1(uint8(seal[pos]) ^ mask);
        vm.expectRevert();
        d.verify(seal, IMG, DIG);
    }

    // ------------------------------------------------------------------ vectors for the Rust attestor

    // Fixed context: Sepolia, verifier at VEC_VERIFIER, attestor = Anvil account 0.
    uint256 constant VEC_CHAIN = 11155111;
    address constant VEC_VERIFIER = address(0x0000000000000000000000000000000000de3071);
    bytes32 constant VEC_DOMAIN_SEPARATOR = 0xe6b874687824a21c15cc5c4cb2de2e3ba9015adc0e7dc330b64abe48610ffb63;

    // Vector 1 (v = 28)
    bytes32 constant V1_IMAGE = 0x1111111111111111111111111111111111111111111111111111111111111111;
    bytes32 constant V1_DIGEST = 0x2222222222222222222222222222222222222222222222222222222222222222;
    bytes32 constant V1_STRUCT_HASH = 0x5821fc36b723560edbece58bc7c0a6524a9015cd4e5b1ed15a19b0102ff27ea2;
    bytes32 constant V1_SIGNED = 0xccd95db1a44605ac7755a99f998c2357a38c70b51ff5fcc129511baab566543a;
    bytes constant V1_SEAL =
        hex"7fa98d9ab548f39a529a2830d35ce44dd7321a713f950ff063c45b017a0cf08606096bcb17cae6edf919145681cfcd99b04496413f81e6531d9780983ab77bbb5dd792f61c";

    // Vector 2 (v = 27): the release image id of the Sepolia demo vaults
    bytes32 constant V2_IMAGE = 0x6bcbd714ca182de91cfb698216662531ae8243fa579bd31b8e48b514df0672a4;
    bytes32 constant V2_DIGEST = 0x3333333333333333333333333333333333333333333333333333333333333333;
    bytes32 constant V2_STRUCT_HASH = 0x04881f3c297418dccafe97327bdf29d7b2131f2aee35de97a5e7118224917352;
    bytes32 constant V2_SIGNED = 0xcfdb0f6252fd4bfcf2e36fb161b724f9262cdc670398d8332a78290cd9b8bf7d;
    bytes constant V2_SEAL =
        hex"7fa98d9a1e726f13485fbd807ae94d9308e3da98140007678e039767864a22eab3216e5f43fe4f685a4544bad2a8db39dcb353482fc5efb17842243edab45c215ffeb1001b";

    function _vecVerifier() internal returns (DAV ver) {
        vm.chainId(VEC_CHAIN);
        deployCodeTo("DemoAttestedVerifier.sol:DemoAttestedVerifier", abi.encode(ANVIL0, LIFETIME), VEC_VERIFIER);
        ver = DAV(VEC_VERIFIER);
    }

    function _structHash(bytes32 img, bytes32 dig) internal pure returns (bytes32) {
        return keccak256(
            abi.encode(
                keccak256("DemoAttestation(bytes32 tag,bytes32 imageId,bytes32 journalDigest)"),
                keccak256("RFP022/demo-attested-verifier/v1"),
                img,
                dig
            )
        );
    }

    function test_Vectors() public {
        DAV ver = _vecVerifier();
        assertEq(vm.addr(ANVIL0_PK), ANVIL0);
        assertEq(ver.domainSeparator(), VEC_DOMAIN_SEPARATOR, "domain separator");

        assertEq(_structHash(V1_IMAGE, V1_DIGEST), V1_STRUCT_HASH, "v1 struct hash");
        assertEq(ver.attestationDigest(V1_IMAGE, V1_DIGEST), V1_SIGNED, "v1 signed digest");
        assertEq(_sign(ver, ANVIL0_PK, V1_IMAGE, V1_DIGEST), V1_SEAL, "v1 seal (RFC 6979)");
        ver.verify(V1_SEAL, V1_IMAGE, V1_DIGEST);
        assertEq(uint8(V1_SEAL[68]), 28);

        assertEq(_structHash(V2_IMAGE, V2_DIGEST), V2_STRUCT_HASH, "v2 struct hash");
        assertEq(ver.attestationDigest(V2_IMAGE, V2_DIGEST), V2_SIGNED, "v2 signed digest");
        assertEq(_sign(ver, ANVIL0_PK, V2_IMAGE, V2_DIGEST), V2_SEAL, "v2 seal (RFC 6979)");
        ver.verify(V2_SEAL, V2_IMAGE, V2_DIGEST);
        assertEq(uint8(V2_SEAL[68]), 27);

        // the two vectors don't cross
        vm.expectPartialRevert(DAV.NotAttestor.selector);
        ver.verify(V1_SEAL, V2_IMAGE, V2_DIGEST);
    }

    // ------------------------------------------------------------------ gas

    function test_GasVerify() public {
        MockVerifier mv = new MockVerifier();
        bytes memory seal = _sign(d, attPk, IMG, DIG);
        bytes memory mseal = mv.sealFor(IMG, DIG);
        uint256 g0 = gasleft();
        d.verify(seal, IMG, DIG);
        uint256 gDemo = g0 - gasleft();
        g0 = gasleft();
        mv.verify(mseal, IMG, DIG);
        uint256 gMock = g0 - gasleft();
        console.log("verify gas: DemoAttestedVerifier %d, MockVerifier %d", gDemo, gMock);
        assertLt(gDemo, 15_000);
    }
}
