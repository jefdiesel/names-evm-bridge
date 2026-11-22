// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Script, console} from "forge-std/Script.sol";
import {BridgeVault} from "../src/BridgeVault.sol";
import {IRiscZeroVerifier} from "../src/IRiscZeroVerifier.sol";
import {MockVerifier, TestToken, NoReturnToken} from "../test/mocks/Mocks.sol";

/// Gas measurement as real transactions on a local anvil, so receipts (intrinsic gas,
/// calldata and EIP-8037 state gas included) can be compared across hardforks, e.g.
///   anvil --hardfork osaka      vs   anvil --hardfork amsterdam   (Foundry >= 1.9)
/// Same flow as test/Gas.t.sol (mock verifier: add the Groth16 verify cost to update and
/// release). Stages, each its own `forge script --broadcast --sig`:
///   setUpAll(bool caps)  deployer = anvil key 0; alice = anvil key 1
///   deposits(uint256 from, uint256 to)   ERC-20 deposits with commitments keccak(i)
///   extraDeposits()      2 USDT-style, 2 ETH
///   updates()            update with 1, 2, 4, 8, 16, 32 exits
///   (then: cast rpc evm_increaseTime 3600 && cast rpc evm_mine)
///   releases()           three ERC-20 releases, relayer paid (first, same recipient, fresh recipient)
/// Read gas from the receipts (cast receipt / broadcast run-latest.json).
contract GasMeasure is Script {
    uint256 immutable K0 = vm.envUint("ANVIL_DEV_KEY_0");
    uint256 immutable K1 = vm.envUint("ANVIL_DEV_KEY_1");
    bytes32 constant G2 = keccak256("g2");
    bytes32 constant G3 = keccak256("g3");

    function _a() internal view returns (address d, address alice, MockVerifier mv, TestToken usdc, NoReturnToken usdt, BridgeVault v) {
        d = vm.addr(K0);
        alice = vm.addr(K1);
        mv = MockVerifier(vm.computeCreateAddress(d, 0));
        usdc = TestToken(vm.computeCreateAddress(d, 1));
        usdt = NoReturnToken(vm.computeCreateAddress(d, 2));
        v = BridgeVault(payable(vm.computeCreateAddress(d, 3)));
    }

    function setUpAll(bool caps) external {
        (address d, address alice,,,,) = _a();
        vm.startBroadcast(K0);
        MockVerifier mv = new MockVerifier();
        TestToken usdc = new TestToken("USDC", 6);
        NoReturnToken usdt = new NoReturnToken();
        BridgeVault v = new BridgeVault(IRiscZeroVerifier(address(mv)), G2, G3, bytes32(0), bytes32(0), keccak256("program"), bytes32(0), 1 hours, d, d, d);
        uint256[] memory dn = new uint256[](1);
        dn[0] = 100e6;
        uint256[] memory f = new uint256[](1);
        f[0] = 1e6;
        v.registerToken(address(usdc), 6, dn, f, f, 1 hours);
        v.registerToken(address(usdt), 6, dn, f, f, 1 hours);
        uint256[] memory de = new uint256[](1);
        de[0] = 1 ether;
        uint256[] memory fe = new uint256[](1);
        v.registerToken(address(0), 18, de, fe, fe, 1 hours);
        uint256[][] memory menu = new uint256[][](1);
        menu[0] = f;
        v.setRelayFeeMenu(address(usdc), menu);
        usdc.mint(alice, type(uint128).max);
        usdt.mint(alice, type(uint128).max);
        if (caps) {
            v.setCaps(address(usdc), 1 days, 1e30, 1e30);
            v.setCaps(address(usdt), 1 days, 1e30, 1e30);
            v.setCaps(address(0), 1 days, 1e30, 1e30);
            v.setGlobalCaps(1 days, 1e30, 1e30);
            v.setCapWeight(address(usdc), 1e18);
            v.setCapWeight(address(usdt), 1e18);
            v.setCapWeight(address(0), 3e9);
        }
        vm.stopBroadcast();
        vm.startBroadcast(K1);
        usdc.approve(address(v), type(uint256).max);
        usdt.approve(address(v), type(uint256).max);
        vm.stopBroadcast();
    }

    function deposits(uint256 from, uint256 to) external {
        (,,, TestToken usdc,, BridgeVault v) = _a();
        vm.startBroadcast(K1);
        for (uint256 i = from; i < to; ++i) v.deposit(address(usdc), 0, keccak256(abi.encode(i)));
        vm.stopBroadcast();
    }

    function extraDeposits() external {
        (,,,, NoReturnToken usdt, BridgeVault v) = _a();
        vm.startBroadcast(K1);
        v.deposit(address(usdt), 0, keccak256(abi.encode(uint256(1001))));
        v.deposit(address(usdt), 0, keccak256(abi.encode(uint256(1002))));
        v.depositETH{value: 1 ether}(0, keccak256(abi.encode(uint256(1003))));
        v.depositETH{value: 1 ether}(0, keccak256(abi.encode(uint256(1004))));
        vm.stopBroadcast();
    }

    function updates() external {
        (,, MockVerifier mv, TestToken usdc,, BridgeVault v) = _a();
        bytes32 cfg = v.cfgId();
        bytes32 state = v.g2State();
        vm.startBroadcast(K1);
        for (uint256 n = 1; n <= 32; n *= 2) {
            BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
            cl[0] = BridgeVault.ClaimDelta(address(usdc), 0, uint64(n));
            bytes32[] memory roots = new bytes32[](1);
            roots[0] = v.depositRoot();
            BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](n);
            for (uint256 i; i < n; ++i) ex[i] = BridgeVault.Exit(address(usdc), 0, keccak256(abi.encode(n, i)));
            bytes32 ns = keccak256(abi.encode(n));
            bytes memory j = abi.encode(BridgeVault.UpdateJournal(cfg, state, ns, roots, cl, ex));
            v.update(j, mv.sealFor(G2, sha256(j)));
            state = ns;
        }
        vm.stopBroadcast();
    }

    function releases() external {
        (,, MockVerifier mv, TestToken usdc,, BridgeVault v) = _a();
        bytes32 cfg = v.cfgId();
        bytes32 root = v.exitRoot();
        vm.startBroadcast(K1);
        // #0: first release (recipient, relayer, nullifier and counters all new)
        // #1: same recipient and relayer again (only the nullifier slot is new)
        // #2: fresh recipient, known relayer (nullifier + recipient balance new): the usual case
        address[3] memory to = [address(0xB0B), address(0xB0B), address(0xB0C)];
        for (uint256 i; i < 3; ++i) {
            bytes32 nf = keccak256(abi.encode("nf", i));
            bytes memory s = mv.sealFor(G3, sha256(abi.encode(cfg, root, nf, address(usdc), uint256(100e6), to[i], address(0x7E1A), uint256(1e6))));
            v.release(s, root, nf, address(usdc), 0, to[i], address(0x7E1A), 1e6);
        }
        vm.stopBroadcast();
    }
}
