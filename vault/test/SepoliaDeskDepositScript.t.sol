// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {DepositDesk, IBridgeVaultDeposits} from "../src/desk/DepositDesk.sol";
import {SepoliaDeskDeposit} from "../script/SepoliaDeskDeposit.s.sol";
import {VaultFixture} from "./VaultFixture.sol";

/// script/SepoliaDeskDeposit.s.sol against the fixture's vault and a fresh desk, with Anvil
/// account 0 (a public test key) as the depositor.
contract SepoliaDeskDepositHarness is SepoliaDeskDeposit {
    address public vaultAddr;
    bytes32 public cfg;

    function setVault(address a, bytes32 c) external {
        vaultAddr = a;
        cfg = c;
    }

    function _vault() internal view override returns (address) {
        return vaultAddr;
    }

    function _cfgId() internal view override returns (bytes32) {
        return cfg;
    }
}

contract SepoliaDeskDepositScriptTest is VaultFixture {
    uint256 constant ANVIL0_PK = 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80;
    address constant ANVIL0 = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266;
    SepoliaDeskDepositHarness h;
    DepositDesk d;

    event FeePaid(
        address indexed token,
        bytes32 indexed lezPublicKey,
        bytes32 indexed commitment,
        uint32 denomIndex,
        uint64 leafIndex,
        uint256 fee
    );

    function setUp() public override {
        super.setUp();
        d = new DepositDesk(IBridgeVaultDeposits(address(v)), admin, 300);
        h = new SepoliaDeskDepositHarness();
        h.setVault(address(v), v.cfgId());
        vm.deal(ANVIL0, 10 ether);
    }

    function test_DepositsTheQuotedTotalWithAZeroAccount() public {
        bytes32 c = keccak256("relay test");
        (uint256 denom,, uint256 fee, uint256 total) = d.quote(address(0), 0);
        uint256 before = ANVIL0.balance;
        vm.expectEmit(true, true, true, false, address(d));
        emit FeePaid(address(0), bytes32(0), c, 0, 0, fee);
        uint64 leaf = h.deposit(ANVIL0_PK, d, 0, c);
        assertTrue(v.commitmentUsed(c));
        assertEq(address(v).balance, denom);
        assertEq(address(d).balance, fee);
        assertEq(before - ANVIL0.balance, total);
        assertEq(leaf, v.depositCount() - 1);
    }

    function test_RefusesAUsedCommitmentAnotherVaultOrZero() public {
        bytes32 c = keccak256("once");
        h.deposit(ANVIL0_PK, d, 0, c);
        vm.expectRevert(bytes("commitment already used in the vault"));
        h.deposit(ANVIL0_PK, d, 0, c);
        vm.expectRevert(bytes("COMMITMENT is zero"));
        h.deposit(ANVIL0_PK, d, 0, bytes32(0));
        h.setVault(address(0xBEEF), v.cfgId());
        vm.expectRevert(bytes("the desk does not forward to the expected demo vault"));
        h.deposit(ANVIL0_PK, d, 0, keccak256("other"));
        h.setVault(address(v), keccak256("another cfg"));
        vm.expectRevert(bytes("vault cfgId is not the expected demo vault's"));
        h.deposit(ANVIL0_PK, d, 0, keccak256("other"));
    }
}
