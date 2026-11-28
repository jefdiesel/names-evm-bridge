// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {BridgeVault} from "../src/BridgeVault.sol";
import {VaultFixture} from "./VaultFixture.sol";

/// A3: the USDT-style token (no return values, F#17) through the whole path with fees.
/// A11: deposit amounts: depositETH takes exactly denomination + fee, an ERC-20 deposit pulls
/// exactly that.
contract DepositsTest is VaultFixture {
    uint256 constant T0 = 1_000_000;

    // ------------------------------------------------------------------ A3: USDT-style full path

    function test_UsdtFullPathExactBalances() public {
        bytes memory feeParams = abi.encode(address(usdt), _arr(1e6, 10e6), _arr(1.5e6, 20e6)); // deposit 1 / withdraw 1.5 on 100
        vm.warp(T0 - ADMIN_DELAY);
        vm.prank(admin);
        v.schedule(BridgeVault.Action.SetFees, feeParams);
        vm.warp(T0);
        vm.startPrank(admin);
        v.execute(BridgeVault.Action.SetFees, feeParams);
        v.setRelayFeeMenu(address(usdt), _menu2(_one(2e6), new uint256[](0)));
        vm.stopPrank();

        uint256 alice0 = usdt.balanceOf(alice);
        vm.startPrank(alice);
        v.deposit(address(usdt), 0, keccak256("u0")); // 100 + 1
        v.deposit(address(usdt), 1, keccak256("u1")); // 1000 + 10
        vm.stopPrank();
        assertEq(alice0 - usdt.balanceOf(alice), 1111e6);
        assertEq(usdt.balanceOf(address(v)), 1111e6);
        assertEq(_fees(address(usdt)), 11e6);

        // claim both, exit the 100 one
        bytes32[] memory roots = new bytes32[](1);
        roots[0] = v.depositRoot();
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](2);
        cl[0] = BridgeVault.ClaimDelta(address(usdt), 0, 1);
        cl[1] = BridgeVault.ClaimDelta(address(usdt), 1, 1);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](1);
        ex[0] = BridgeVault.Exit(address(usdt), 0, keccak256("exit"));
        _update(_journal(S0, keccak256("s1"), roots, cl, ex));
        assertEq(v.claimedValue(address(usdt)), 1100e6);
        assertEq(v.burnedValue(address(usdt)), 100e6);
        bytes32 root = v.exitRoot();

        vm.warp(T0 + DELAY);
        bytes32 nf = keccak256("nf");
        bytes memory seal = _releaseSeal(root, nf, address(usdt), 100e6, bob, relayer, 2e6);
        v.release(seal, root, nf, address(usdt), 0, bob, relayer, 2e6);
        assertEq(usdt.balanceOf(bob), 100e6 - 1.5e6 - 2e6); // 96.5
        assertEq(usdt.balanceOf(relayer), 2e6);
        assertEq(usdt.balanceOf(address(v)), 1111e6 - 96.5e6 - 2e6);
        assertEq(_fees(address(usdt)), 12.5e6); // 1 + 10 + 1.5

        v.withdrawFees(address(usdt)); // permissionless, pays the fee recipient
        assertEq(usdt.balanceOf(feeTo), 12.5e6);
        assertEq(_fees(address(usdt)), 0);
        // only the unreleased 1000 of principal is left
        assertEq(usdt.balanceOf(address(v)), 1000e6);
        // a second withdrawal moves nothing
        v.withdrawFees(address(usdt));
        assertEq(usdt.balanceOf(feeTo), 12.5e6);
        assertEq(usdt.balanceOf(address(v)), 1000e6);
        (uint64 d0, uint64 c0, uint64 b0, uint64 r0) = v.counters(address(usdt), 0);
        assertEq(d0, 1);
        assertEq(c0, 1);
        assertEq(b0, 1);
        assertEq(r0, 1);
    }

    // ------------------------------------------------------------------ A11: amounts

    /// Any msg.value other than denomination + fee is refused and changes nothing; the exact
    /// value then goes through (with the same commitment, which the failed call didn't burn).
    function testFuzz_DepositEthWrongAmount(uint256 value, bool big, uint256 feeSeed) public {
        uint32 d = big ? 1 : 0;
        uint256 denom = big ? 1 ether : 0.1 ether;
        uint256 fee = bound(feeSeed, 0, denom * 200 / 10_000);
        uint256[] memory dfees = big ? _arr(0, fee) : _arr(fee, 0);
        _execAsAdmin(BridgeVault.Action.SetFees, abi.encode(address(0), dfees, _arr(0.001 ether, 0.01 ether)));
        value = bound(value, 0, 100 ether);
        vm.assume(value != denom + fee);

        bytes32 c = keccak256(abi.encode("eth", value));
        uint256 aliceBefore = alice.balance;
        uint256 vaultBefore = address(v).balance;
        bytes32 rootBefore = v.depositRoot();
        vm.prank(alice);
        vm.expectRevert(BridgeVault.WrongAmount.selector);
        v.depositETH{value: value}(d, c);
        assertEq(alice.balance, aliceBefore);
        assertEq(address(v).balance, vaultBefore);
        assertEq(v.depositRoot(), rootBefore);
        assertEq(v.depositCount(), 0);
        (uint64 dep,,,) = v.counters(address(0), d);
        assertEq(dep, 0);
        assertEq(_fees(address(0)), 0);
        assertFalse(v.commitmentUsed(c));

        vm.prank(alice);
        v.depositETH{value: denom + fee}(d, c);
        assertEq(aliceBefore - alice.balance, denom + fee);
        assertEq(address(v).balance, vaultBefore + denom + fee);
        assertEq(_fees(address(0)), fee);
        (dep,,,) = v.counters(address(0), d);
        assertEq(dep, 1);
    }

    /// The values next to the right one: denom + fee +- 1, the bare denomination when a fee is
    /// due, the denomination plus twice the fee, and 0. All refused, nothing recorded.
    function test_DepositEthNearMissAmounts() public {
        _execAsAdmin(
            BridgeVault.Action.SetFees, abi.encode(address(0), _arr(0.002 ether, 0.02 ether), _arr(0.001 ether, 0.01 ether))
        );
        uint256[2] memory denoms = [uint256(0.1 ether), 1 ether];
        uint256[2] memory fees = [uint256(0.002 ether), 0.02 ether];
        for (uint32 d; d < 2; ++d) {
            uint256 right = denoms[d] + fees[d];
            uint256[5] memory wrong = [right - 1, right + 1, denoms[d], denoms[d] + 2 * fees[d], 0];
            for (uint256 i; i < wrong.length; ++i) {
                vm.prank(alice);
                vm.expectRevert(BridgeVault.WrongAmount.selector);
                v.depositETH{value: wrong[i]}(d, keccak256(abi.encode(d, i)));
            }
        }
        assertEq(address(v).balance, 0);
        assertEq(v.depositCount(), 0);
        assertEq(_fees(address(0)), 0);
        vm.prank(alice);
        v.depositETH{value: 1.02 ether}(1, keccak256("ok"));
        assertEq(address(v).balance, 1.02 ether);
    }

    /// ETH only enters through depositETH: `deposit` refuses the ETH address, is not payable,
    /// and the vault has no receive function (so stray ETH can't upset the balance accounting).
    function test_EthOnlyThroughDepositEth() public {
        // `deposit` refuses the ETH address
        vm.prank(alice);
        vm.expectRevert(BridgeVault.BadDenomination.selector);
        v.deposit(address(0), 0, keccak256("c"));
        // `deposit` is not payable: sending ETH along fails at the ABI level
        (bool ok,) = address(v).call{value: 1}(abi.encodeCall(BridgeVault.deposit, (address(usdc), 0, keccak256("c"))));
        assertFalse(ok);
        // and a plain transfer has no receive function
        (ok,) = address(v).call{value: 1}("");
        assertFalse(ok);
    }

    /// An ERC-20 deposit pulls exactly denomination + fee, whatever the fee setting.
    function testFuzz_Erc20DepositPullsExactly(bool big, uint256 depFee, uint256 wFee, bool noReturn) public {
        address t = noReturn ? address(usdt) : address(usdc);
        uint32 d = big ? 1 : 0;
        uint256 denom = big ? 1000e6 : 100e6;
        uint256 fee = bound(depFee, 0, denom * 200 / 10_000);
        uint256 wf = bound(wFee, 0, 2e6);
        _execAsAdmin(BridgeVault.Action.SetFees, abi.encode(t, big ? _arr(1e6, fee) : _arr(fee, 10e6), _arr(wf, 0)));

        uint256 aliceBefore = _bal(t, alice);
        uint256 vaultBefore = _bal(t, address(v));
        uint256 feesBefore = _fees(t);
        vm.prank(alice);
        v.deposit(t, d, keccak256("c"));
        assertEq(aliceBefore - _bal(t, alice), denom + fee);
        assertEq(_bal(t, address(v)) - vaultBefore, denom + fee);
        assertEq(_fees(t) - feesBefore, fee);
        (uint64 dep,,,) = v.counters(t, d);
        assertEq(dep, 1);
    }

    /// Without enough allowance or balance the deposit reverts inside the token and nothing
    /// is recorded (both token styles).
    function test_Erc20DepositInsufficientFundsRecordsNothing() public {
        address carol = address(0xCA201);
        usdc.mint(carol, 100e6); // fee is 1, so 101 is needed
        vm.startPrank(carol);
        usdc.approve(address(v), type(uint256).max);
        vm.expectRevert();
        v.deposit(address(usdc), 0, keccak256("c"));
        usdt.approve(address(v), 50e6);
        vm.expectRevert();
        v.deposit(address(usdt), 0, keccak256("c")); // allowance too small, no bool returned
        vm.stopPrank();
        assertEq(v.depositCount(), 0);
        assertFalse(v.commitmentUsed(keccak256("c")));
    }
}
