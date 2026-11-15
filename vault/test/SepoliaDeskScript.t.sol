// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {BridgeVault} from "../src/BridgeVault.sol";
import {DepositDesk} from "../src/desk/DepositDesk.sol";
import {SepoliaDesk} from "../script/SepoliaDesk.s.sol";
import {VaultFixture} from "./VaultFixture.sol";

/// script/SepoliaDesk.s.sol with the deployer = Anvil account 0 (a public test key) and the
/// fixture's vault instead of the live v2 vault.
contract SepoliaDeskHarness is SepoliaDesk {
    address public vaultAddr;
    bytes32 public cfg;

    function setVault(address a, bytes32 c) external {
        vaultAddr = a;
        cfg = c;
    }

    function _deployer() internal pure override returns (address) {
        return 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;
    }

    function _vault() internal view override returns (address) {
        return vaultAddr;
    }

    function _cfgId() internal view override returns (bytes32) {
        return cfg;
    }
}

contract SepoliaDeskScriptTest is VaultFixture {
    uint256 constant ANVIL0_PK = 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80;
    address constant ANVIL0 = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;
    SepoliaDeskHarness h;

    function setUp() public override {
        super.setUp();
        h = new SepoliaDeskHarness();
        h.setVault(address(v), v.cfgId());
    }

    function test_DeploysWithOwnerVaultAndFee() public {
        DepositDesk d = h.deploy(ANVIL0_PK, 300);
        assertEq(address(d.vault()), address(v));
        assertEq(d.owner(), ANVIL0);
        assertEq(d.feeBps(), 300);
        // and it works: 0.1 ETH + 3%
        vm.prank(alice);
        d.depositETHWithFee{value: 0.103 ether}(0, keccak256("script"), bytes32(0));
        assertEq(address(v).balance, 0.1 ether);
    }

    function test_RefusesWrongKeyFeeOrVault() public {
        vm.expectRevert(bytes("SEPOLIA_PK is not the expected deployer"));
        h.deploy(uint256(keccak256("someone else")), 300);
        vm.expectRevert(bytes("DESK_FEE_BPS above 300"));
        h.deploy(ANVIL0_PK, 301);
        h.setVault(address(v), keccak256("another cfg"));
        vm.expectRevert(bytes("vault cfgId is not the expected one"));
        h.deploy(ANVIL0_PK, 300);
        h.setVault(address(0xDEAD), bytes32(0));
        vm.expectRevert(bytes("no vault code"));
        h.deploy(ANVIL0_PK, 300);
    }
}
