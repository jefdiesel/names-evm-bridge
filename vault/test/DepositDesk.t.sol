// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import {BridgeVault} from "../src/BridgeVault.sol";
import {DepositDesk, IBridgeVaultDeposits} from "../src/desk/DepositDesk.sol";
import {VaultFixture} from "./VaultFixture.sol";
import {TestToken, FeeOnTransferToken} from "./mocks/Mocks.sol";

/// A token whose transferFrom re-enters the desk once when armed.
contract ReenteringToken is ERC20 {
    DepositDesk public desk;
    bool public armed;

    constructor() ERC20("RE", "RE") {}

    function mint(address to, uint256 a) external {
        _mint(to, a);
    }

    function arm(DepositDesk d) external {
        desk = d;
        armed = true;
    }

    function _update(address from, address to, uint256 value) internal override {
        super._update(from, to, value);
        if (armed && to == address(desk)) {
            armed = false;
            desk.depositWithFee(address(this), 0, keccak256("re-entered"), bytes32(0), type(uint256).max);
        }
    }
}

/// An owner contract whose receive re-enters withdrawFees.
contract ReenteringOwner {
    DepositDesk public desk;

    function set(DepositDesk d) external {
        desk = d;
    }

    function pull() external {
        desk.withdrawFees(address(0), address(this));
    }

    receive() external payable {
        desk.withdrawFees(address(0), address(this));
    }
}

/// DepositDesk against the REAL BridgeVault of VaultFixture: USDC 100/1000 with a 1% vault
/// deposit fee, a USDT-style token (no return values) 100/1000 without fees, ETH 0.1/1 without
/// a deposit fee. Bridge fee 300 bps (3%) unless a test says otherwise.
contract DepositDeskTest is VaultFixture {
    DepositDesk desk;
    address deskOwner = address(0xD35C);
    address carol = address(0xCA201);

    // P2's public key and account id, from public-demo/claim-p2-public-20261113.txt (not published) (the id is
    // sha256("/LEE/v0.3/AccountId/Public/" ‖ 5 zero bytes ‖ pk); the desk does not derive it).
    bytes32 constant PK_P2 = 0xf31c4c105101bf5363ffebb605e318a92cc19e18689c1e57f823f7271ee5a1c3;

    event FeePaid(
        address indexed token,
        bytes32 indexed lezPublicKey,
        bytes32 indexed commitment,
        uint32 denomIndex,
        uint64 leafIndex,
        uint256 fee
    );
    event Deposit(address indexed token, uint32 denomIndex, bytes32 commitment, uint64 leafIndex, bytes32 root);

    function setUp() public override {
        super.setUp();
        desk = new DepositDesk(IBridgeVaultDeposits(address(v)), deskOwner, 300);
        vm.startPrank(alice);
        usdc.approve(address(desk), type(uint256).max);
        usdt.approve(address(desk), type(uint256).max);
        vm.stopPrank();
    }

    // ------------------------------------------------------------------ helpers

    function _ethTotal(uint32 d) internal view returns (uint256 total) {
        (,,, total) = desk.quote(address(0), d);
    }

    function _deskEth(uint32 d, bytes32 c, bytes32 pk) internal returns (uint64) {
        uint256 total = _ethTotal(d);
        vm.prank(alice);
        return desk.depositETHWithFee{value: total}(d, c, pk);
    }

    // ------------------------------------------------------------------ the ABI the page uses

    function test_SelectorsThePageUses() public view {
        assertEq(DepositDesk.depositETHWithFee.selector, bytes4(0x03321b04));
        // depositWithFee(address,uint32,bytes32,bytes32,uint256): the fee cap; the deployed
        // desks (v2 to v5) have the four-argument 0xa93bf283, which the page still sends them
        assertEq(DepositDesk.depositWithFee.selector, bytes4(0xc5bbd0a1));
        assertEq(DepositDesk.quote.selector, bytes4(0xea48dc6c));
        assertEq(desk.feeBps.selector, bytes4(0x24a9d853));
        assertEq(desk.vault.selector, bytes4(0xfbfa77cf));
        assertEq(DepositDesk.WrongAmount.selector, bytes4(0x49986e73));
        assertEq(DepositDesk.UnexpectedValue.selector, bytes4(0x123146a6));
        assertEq(
            keccak256("FeePaid(address,bytes32,bytes32,uint32,uint64,uint256)"),
            0x2fe535c5fd3b340521910740117c9de5d7485f5881b3b63c47bf695b1d8da057
        );
    }

    // ------------------------------------------------------------------ construction

    function test_ConstructorRefusesFeeAboveCapAndBadVault() public {
        vm.expectRevert(DepositDesk.FeeTooHigh.selector);
        new DepositDesk(IBridgeVaultDeposits(address(v)), deskOwner, 301);
        vm.expectRevert(DepositDesk.ZeroAddress.selector);
        new DepositDesk(IBridgeVaultDeposits(address(0)), deskOwner, 300);
        vm.expectRevert(DepositDesk.ZeroAddress.selector);
        new DepositDesk(IBridgeVaultDeposits(address(v)), address(0), 300);
        vm.expectRevert(DepositDesk.NoVaultCode.selector);
        new DepositDesk(IBridgeVaultDeposits(address(0xDEAD)), deskOwner, 300);
        DepositDesk d = new DepositDesk(IBridgeVaultDeposits(address(v)), deskOwner, 0);
        assertEq(d.feeBps(), 0);
        assertEq(d.MAX_FEE_BPS(), 300);
        assertEq(address(d.vault()), address(v));
        assertEq(d.owner(), deskOwner);
    }

    // ------------------------------------------------------------------ same leaf as a direct deposit

    function test_EthLeafIdenticalToDirectDeposit() public {
        bytes32 c = keccak256("eth leaf");
        _deskEth(0, keccak256("earlier"), bytes32(0)); // a leaf before, so the index is not 0
        uint256 snap = vm.snapshotState();
        vm.prank(alice);
        uint64 directIdx = v.depositETH{value: 0.1 ether}(0, c);
        bytes32 directRoot = v.depositRoot();
        uint64 directCount = v.depositCount();
        vm.revertToState(snap);

        assertFalse(v.commitmentUsed(c));
        vm.expectEmit(true, false, false, true, address(v));
        emit Deposit(address(0), 0, c, directIdx, directRoot);
        vm.expectEmit(true, true, true, true, address(desk));
        emit FeePaid(address(0), PK_P2, c, 0, directIdx, 0.003 ether);
        uint64 deskIdx = _deskEth(0, c, PK_P2);

        assertEq(deskIdx, directIdx);
        assertEq(v.depositRoot(), directRoot);
        assertEq(v.depositCount(), directCount);
        assertTrue(v.commitmentUsed(c));
        assertTrue(v.isDepositRoot(directRoot));
    }

    function test_Erc20LeafIdenticalToDirectDepositWithVaultFee() public {
        bytes32 c = keccak256("usdc leaf");
        uint256 snap = vm.snapshotState();
        vm.prank(alice);
        uint64 directIdx = v.deposit(address(usdc), 0, c);
        bytes32 directRoot = v.depositRoot();
        uint256 directVaultBal = usdc.balanceOf(address(v));
        vm.revertToState(snap);

        (uint256 denom, uint256 vaultFee, uint256 fee, uint256 total) = desk.quote(address(usdc), 0);
        assertEq(denom, 100e6);
        assertEq(vaultFee, 1e6);
        assertEq(fee, 3e6);
        assertEq(total, 104e6);

        uint256 alice0 = usdc.balanceOf(alice);
        vm.expectEmit(true, false, false, true, address(v));
        emit Deposit(address(usdc), 0, c, directIdx, directRoot);
        vm.expectEmit(true, true, true, true, address(desk));
        emit FeePaid(address(usdc), bytes32(0), c, 0, directIdx, 3e6);
        vm.prank(alice);
        uint64 deskIdx = desk.depositWithFee(address(usdc), 0, c, bytes32(0), total);

        assertEq(deskIdx, directIdx);
        assertEq(v.depositRoot(), directRoot);
        assertEq(usdc.balanceOf(address(v)), directVaultBal); // the vault got exactly 101
        assertEq(alice0 - usdc.balanceOf(alice), 104e6); // one pull of denom + vault fee + 3%
        assertEq(usdc.balanceOf(address(desk)), 3e6);
        assertEq(desk.feesHeld(address(usdc)), 3e6);
        assertEq(usdc.allowance(address(desk), address(v)), 0);
        (,,,,,,,, uint256 accrued) = v.tokenConfig(address(usdc));
        assertEq(accrued, 1e6); // the vault's own fee is the vault's
    }

    function test_UsdtStyleTokenThroughDesk() public {
        uint256 alice0 = usdt.balanceOf(alice);
        vm.prank(alice);
        desk.depositWithFee(address(usdt), 1, keccak256("usdt"), PK_P2, 1030e6);
        assertEq(alice0 - usdt.balanceOf(alice), 1030e6);
        assertEq(usdt.balanceOf(address(v)), 1000e6);
        assertEq(usdt.balanceOf(address(desk)), 30e6);
        assertEq(usdt.allowance(address(desk), address(v)), 0);
    }

    // ------------------------------------------------------------------ the fee is always charged

    function test_FeeChargedForZeroKeyToo() public {
        uint256 alice0 = alice.balance;
        vm.expectEmit(true, true, true, true, address(desk));
        emit FeePaid(address(0), bytes32(0), keccak256("private"), 1, 0, 0.03 ether);
        _deskEth(1, keccak256("private"), bytes32(0));
        assertEq(alice0 - alice.balance, 1.03 ether);
        assertEq(address(v).balance, 1 ether);
        assertEq(address(desk).balance, 0.03 ether);
        assertEq(desk.feesHeld(address(0)), 0.03 ether);
    }

    function test_FeeZeroAllowed() public {
        vm.prank(deskOwner);
        desk.setFeeBps(0);
        vm.prank(alice);
        desk.depositETHWithFee{value: 0.1 ether}(0, keccak256("free"), PK_P2);
        assertEq(address(desk).balance, 0);
        assertEq(address(v).balance, 0.1 ether);
    }

    // ------------------------------------------------------------------ rounding

    /// fee = floor(denom * bps / 10_000), for any denomination and any allowed rate.
    function testFuzz_FeeRoundsDown(uint128 denom, uint16 bpsSeed) public {
        uint256 bps = uint256(bpsSeed) % 301;
        vm.prank(deskOwner);
        desk.setFeeBps(bps);
        uint256 fee = desk.feeFor(denom);
        assertLe(fee * 10_000, uint256(denom) * bps);
        assertGt((fee + 1) * 10_000, uint256(denom) * bps);
    }

    /// Odd denominations through a real deposit: 1 unit (fee 0), 33 (fee 0.99 -> 0), 1e6 + 7
    /// (fee 30,000.21 -> 30,000). The depositor pays exactly denom + floor(3%).
    function test_OddDenominationsExactAmounts() public {
        TestToken odd = new TestToken("ODD", 6);
        uint256[] memory d = new uint256[](3);
        d[0] = 1;
        d[1] = 33;
        d[2] = 1e6 + 7;
        vm.prank(admin);
        v.registerToken(address(odd), 6, d, new uint256[](3), new uint256[](3), DELAY);
        odd.mint(alice, 10e6);
        vm.prank(alice);
        odd.approve(address(desk), type(uint256).max);
        uint256[3] memory wantFee = [uint256(0), 0, 30_000];
        for (uint32 i; i < 3; ++i) {
            uint256 a0 = odd.balanceOf(alice);
            uint256 v0 = odd.balanceOf(address(v));
            vm.prank(alice);
            desk.depositWithFee(address(odd), i, keccak256(abi.encode("odd", i)), bytes32(0), d[i] + wantFee[i]);
            assertEq(a0 - odd.balanceOf(alice), d[i] + wantFee[i]);
            assertEq(odd.balanceOf(address(v)) - v0, d[i]);
        }
        assertEq(odd.balanceOf(address(desk)), 30_000);
        assertEq(desk.feesHeld(address(odd)), 30_000);
    }

    function test_CurrentDemoDenominationsDivideExactly() public view {
        uint256[6] memory dn = [uint256(100e6), 1_000e6, 10_000e6, 0.01 ether, 0.1 ether, 1 ether];
        for (uint256 i; i < 6; ++i) assertEq(desk.feeFor(dn[i]) * 10_000, dn[i] * 300);
    }

    // ------------------------------------------------------------------ wrong amounts (fuzz)

    function testFuzz_EthWrongValueReverts(uint256 value, uint8 d) public {
        uint32 di = uint32(d % 2);
        uint256 total = _ethTotal(di);
        value = bound(value, 0, 5 ether);
        vm.assume(value != total);
        vm.prank(alice);
        vm.expectRevert(DepositDesk.WrongAmount.selector);
        desk.depositETHWithFee{value: value}(di, keccak256("x"), PK_P2);
        assertEq(address(desk).balance, 0);
        assertEq(address(v).balance, 0);
    }

    function testFuzz_Erc20WithEthReverts(uint256 value) public {
        value = bound(value, 1, 5 ether);
        vm.prank(alice);
        vm.expectRevert(DepositDesk.UnexpectedValue.selector);
        desk.depositWithFee{value: value}(address(usdc), 0, keccak256("x"), PK_P2, type(uint256).max);
    }

    /// An allowance below the total (for example the denomination alone, without the fee)
    /// pulls nothing.
    function testFuzz_Erc20ShortAllowanceReverts(uint256 allowance_, uint8 d) public {
        uint32 di = uint32(d % 2);
        (,,, uint256 total) = desk.quote(address(usdc), di);
        allowance_ = bound(allowance_, 0, total - 1);
        vm.startPrank(alice);
        usdc.approve(address(desk), allowance_);
        uint256 a0 = usdc.balanceOf(alice);
        vm.expectRevert();
        desk.depositWithFee(address(usdc), di, keccak256("x"), bytes32(0), total);
        vm.stopPrank();
        assertEq(usdc.balanceOf(alice), a0);
        assertEq(usdc.balanceOf(address(desk)), 0);
    }

    // ------------------------------------------------------------------ maxTotal cap

    /// Total strictly below maxTotal passes and pulls only the quoted total, never maxTotal.
    function test_DepositWithFeeUnderCapPasses() public {
        (uint256 denom, uint256 vaultFee, uint256 fee, uint256 total) = desk.quote(address(usdc), 0);
        uint256 cap = total + 50e6;
        uint256 a0 = usdc.balanceOf(alice);
        bytes32 c = keccak256("under cap");
        vm.prank(alice);
        desk.depositWithFee(address(usdc), 0, c, bytes32(0), cap);
        assertEq(a0 - usdc.balanceOf(alice), total);
        assertEq(usdc.balanceOf(address(v)), denom + vaultFee);
        assertEq(usdc.balanceOf(address(desk)), fee);
        assertEq(desk.feesHeld(address(usdc)), fee);
        assertTrue(v.commitmentUsed(c));
    }

    /// Total equal to maxTotal passes.
    function test_DepositWithFeeExactCapPasses() public {
        (,,, uint256 total) = desk.quote(address(usdc), 0);
        uint256 a0 = usdc.balanceOf(alice);
        bytes32 c = keccak256("exact cap");
        vm.prank(alice);
        desk.depositWithFee(address(usdc), 0, c, bytes32(0), total);
        assertEq(a0 - usdc.balanceOf(alice), total);
        assertTrue(v.commitmentUsed(c));
    }

    /// Total above maxTotal reverts even when the depositor has an unlimited standing allowance,
    /// pulling nothing and leaving both desk and vault unchanged.
    function test_DepositWithFeeOverCapReverts() public {
        (,,, uint256 total) = desk.quote(address(usdc), 0);
        uint256 a0 = usdc.balanceOf(alice);
        bytes32 c = keccak256("over cap");
        // alice has type(uint256).max approval from setUp()
        assertEq(usdc.allowance(alice, address(desk)), type(uint256).max);
        vm.startPrank(alice);
        vm.expectRevert(DepositDesk.WrongAmount.selector);
        desk.depositWithFee(address(usdc), 0, c, bytes32(0), total - 1);
        vm.expectRevert(DepositDesk.WrongAmount.selector);
        desk.depositWithFee(address(usdc), 0, c, bytes32(0), 0);
        vm.stopPrank();
        assertEq(usdc.balanceOf(alice), a0);
        assertEq(usdc.balanceOf(address(desk)), 0);
        assertEq(usdc.balanceOf(address(v)), 0);
        assertEq(desk.feesHeld(address(usdc)), 0);
        assertFalse(v.commitmentUsed(c));
    }

    function testFuzz_DepositWithFeeMaxTotal(uint256 maxTotal, uint8 d) public {
        uint32 di = uint32(d % 2);
        (,,, uint256 total) = desk.quote(address(usdc), di);
        uint256 a0 = usdc.balanceOf(alice);
        bytes32 c = keccak256(abi.encode("cap", maxTotal, di));
        vm.prank(alice);
        if (maxTotal < total) {
            vm.expectRevert(DepositDesk.WrongAmount.selector);
            desk.depositWithFee(address(usdc), di, c, bytes32(0), maxTotal);
            assertEq(usdc.balanceOf(alice), a0);
            assertFalse(v.commitmentUsed(c));
        } else {
            desk.depositWithFee(address(usdc), di, c, bytes32(0), maxTotal);
            assertEq(a0 - usdc.balanceOf(alice), total);
            assertTrue(v.commitmentUsed(c));
        }
    }

    /// A fee change between the page's quote and the transaction makes the deposit revert:
    /// ETH via exact msg.value, ERC-20 via maxTotal even with an unlimited standing approval.
    function test_FeeChangeAfterQuoteReverts() public {
        uint256 total = _ethTotal(0); // 0.103
        vm.prank(deskOwner);
        desk.setFeeBps(100);
        vm.prank(alice);
        vm.expectRevert(DepositDesk.WrongAmount.selector);
        desk.depositETHWithFee{value: total}(0, keccak256("x"), bytes32(0));

        // Unlimited approval is already set in setUp(); raising feeBps above maxTotal still reverts.
        (,,, uint256 uTotal) = desk.quote(address(usdc), 0); // 102 at 1%
        uint256 a0 = usdc.balanceOf(alice);
        vm.prank(deskOwner);
        desk.setFeeBps(300); // now 104
        vm.prank(alice);
        vm.expectRevert(DepositDesk.WrongAmount.selector);
        desk.depositWithFee(address(usdc), 0, keccak256("y"), bytes32(0), uTotal);
        assertEq(usdc.balanceOf(alice), a0);
    }

    function test_BadTokenOrIndexReverts() public {
        vm.startPrank(alice);
        vm.expectRevert(DepositDesk.BadDenomination.selector);
        desk.depositWithFee(address(0), 0, keccak256("x"), bytes32(0), type(uint256).max);
        vm.expectRevert(DepositDesk.BadDenomination.selector);
        desk.depositETHWithFee{value: 1 ether}(7, keccak256("x"), bytes32(0));
        vm.expectRevert(DepositDesk.BadDenomination.selector);
        desk.depositWithFee(address(0xBEEF), 0, keccak256("x"), bytes32(0), type(uint256).max); // unregistered: no denoms
        vm.stopPrank();
    }

    // ------------------------------------------------------------------ the vault refuses -> nothing kept

    function test_VaultRefusalRevertsEverything() public {
        bytes32 c = keccak256("used");
        _deskEth(0, c, bytes32(0));
        uint256 deskBal = address(desk).balance;
        uint256 a0 = alice.balance;
        uint256 total = _ethTotal(0);

        vm.prank(alice);
        vm.expectRevert(BridgeVault.CommitmentUsed.selector);
        desk.depositETHWithFee{value: total}(0, c, bytes32(0));

        vm.prank(freezer);
        v.freeze(address(usdc), true, false);
        vm.prank(alice);
        vm.expectRevert(BridgeVault.IsFrozen.selector);
        desk.depositWithFee(address(usdc), 0, keccak256("frozen"), bytes32(0), type(uint256).max);

        assertEq(alice.balance, a0);
        assertEq(address(desk).balance, deskBal);
        assertEq(usdc.balanceOf(address(desk)), 0);
        assertEq(desk.feesHeld(address(usdc)), 0);
    }

    function test_FeeOnTransferTokenRefused() public {
        FeeOnTransferToken fot = new FeeOnTransferToken();
        vm.prank(admin);
        v.registerToken(address(fot), 18, _arr(100e18, 1000e18), _arr(0, 0), _arr(0, 0), DELAY);
        fot.mint(alice, 10_000e18);
        vm.startPrank(alice);
        fot.approve(address(desk), type(uint256).max);
        vm.expectRevert(DepositDesk.WrongAmount.selector);
        desk.depositWithFee(address(fot), 0, keccak256("fot"), bytes32(0), type(uint256).max);
        vm.stopPrank();
    }

    /// Tokens or a balance sent to the desk by anyone else neither block deposits nor become
    /// withdrawable fees.
    function test_StrayTokensDoNotBlockOrCount() public {
        usdc.mint(carol, 5e6);
        vm.prank(carol);
        usdc.transfer(address(desk), 5e6);
        vm.prank(alice);
        desk.depositWithFee(address(usdc), 0, keccak256("after stray"), bytes32(0), type(uint256).max);
        assertEq(desk.feesHeld(address(usdc)), 3e6);
        assertEq(usdc.balanceOf(address(desk)), 8e6);
        vm.prank(deskOwner);
        desk.withdrawFees(address(usdc), deskOwner);
        assertEq(usdc.balanceOf(deskOwner), 3e6);
        assertEq(usdc.balanceOf(address(desk)), 5e6);
    }

    function test_NoReceive() public {
        vm.prank(alice);
        (bool ok,) = address(desk).call{value: 1 ether}("");
        assertFalse(ok);
    }

    // ------------------------------------------------------------------ the desk never holds a deposit

    /// Any sequence of deposits: the vault gets every denomination + vault fee in the same call;
    /// the desk holds exactly the bridge fees it records, nothing more.
    function testFuzz_DeskHoldsOnlyFees(uint256 seed) public {
        uint256 n = seed % 8 + 1;
        uint256 ethToVault;
        uint256 usdcToVault;
        uint256 ethFees;
        uint256 usdcFees;
        for (uint256 i; i < n; ++i) {
            uint256 r = uint256(keccak256(abi.encode(seed, i)));
            uint32 di = uint32(r % 2);
            bytes32 c = keccak256(abi.encode("c", seed, i));
            bytes32 pk = r % 3 == 0 ? bytes32(0) : bytes32(r);
            if ((r >> 8) % 2 == 0) {
                (uint256 dn, uint256 vf, uint256 f,) = desk.quote(address(0), di);
                _deskEth(di, c, pk);
                ethToVault += dn + vf;
                ethFees += f;
            } else {
                (uint256 dn, uint256 vf, uint256 f, uint256 tot) = desk.quote(address(usdc), di);
                vm.prank(alice);
                desk.depositWithFee(address(usdc), di, c, pk, tot);
                usdcToVault += dn + vf;
                usdcFees += f;
            }
            assertEq(address(desk).balance, desk.feesHeld(address(0)));
            assertEq(usdc.balanceOf(address(desk)), desk.feesHeld(address(usdc)));
            assertEq(usdc.allowance(address(desk), address(v)), 0);
        }
        assertEq(address(v).balance, ethToVault);
        assertEq(usdc.balanceOf(address(v)), usdcToVault);
        assertEq(desk.feesHeld(address(0)), ethFees);
        assertEq(desk.feesHeld(address(usdc)), usdcFees);
        assertEq(v.depositCount(), n);
    }

    // ------------------------------------------------------------------ reentrancy

    function test_ReentrantTokenRefused() public {
        ReenteringToken rt = new ReenteringToken();
        vm.prank(admin);
        v.registerToken(address(rt), 18, _arr(100e18, 1000e18), _arr(0, 0), _arr(0, 0), DELAY);
        rt.mint(alice, 10_000e18);
        vm.prank(alice);
        rt.approve(address(desk), type(uint256).max);
        rt.arm(desk);
        vm.prank(alice);
        vm.expectRevert(ReentrancyGuard.ReentrancyGuardReentrantCall.selector);
        desk.depositWithFee(address(rt), 0, keccak256("outer"), bytes32(0), type(uint256).max);
    }

    function test_ReentrantWithdrawRefused() public {
        ReenteringOwner o = new ReenteringOwner();
        DepositDesk d = new DepositDesk(IBridgeVaultDeposits(address(v)), address(o), 300);
        o.set(d);
        vm.prank(alice);
        d.depositETHWithFee{value: 0.103 ether}(0, keccak256("w"), bytes32(0));
        vm.expectRevert(DepositDesk.EthTransferFailed.selector);
        o.pull();
        assertEq(address(d).balance, 0.003 ether);
        assertEq(d.feesHeld(address(0)), 0.003 ether);
    }

    // ------------------------------------------------------------------ owner powers

    function testFuzz_SetFeeBpsWithinCap(uint256 bps) public {
        vm.prank(deskOwner);
        if (bps > 300) {
            vm.expectRevert(DepositDesk.FeeTooHigh.selector);
            desk.setFeeBps(bps);
        } else {
            desk.setFeeBps(bps);
            assertEq(desk.feeBps(), bps);
        }
    }

    function testFuzz_OnlyOwner(address who) public {
        vm.assume(who != deskOwner);
        vm.startPrank(who);
        vm.expectRevert(DepositDesk.NotOwner.selector);
        desk.setFeeBps(100);
        vm.expectRevert(DepositDesk.NotOwner.selector);
        desk.withdrawFees(address(0), who);
        vm.stopPrank();
    }

    function test_WithdrawFeesEthAndToken() public {
        _deskEth(1, keccak256("a"), PK_P2);
        vm.prank(alice);
        desk.depositWithFee(address(usdc), 1, keccak256("b"), bytes32(0), type(uint256).max);
        vm.startPrank(deskOwner);
        vm.expectRevert(DepositDesk.ZeroAddress.selector);
        desk.withdrawFees(address(0), address(0));
        desk.withdrawFees(address(0), carol);
        desk.withdrawFees(address(usdc), carol);
        desk.withdrawFees(address(usdc), carol); // nothing left: sends 0
        vm.stopPrank();
        assertEq(carol.balance, 0.03 ether);
        assertEq(usdc.balanceOf(carol), 30e6);
        assertEq(address(desk).balance, 0);
        assertEq(desk.feesHeld(address(0)), 0);
        assertEq(desk.feesHeld(address(usdc)), 0);
        // the vault's deposits are untouched by any desk call
        assertEq(address(v).balance, 1 ether);
        assertEq(usdc.balanceOf(address(v)), 1010e6);
    }
}
