// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {CommonBase} from "forge-std/Base.sol";
import {StdCheats} from "forge-std/StdCheats.sol";
import {StdUtils} from "forge-std/StdUtils.sol";
import {BridgeVaultFailsafe as V} from "../../src/failsafe/BridgeVaultFailsafe.sol";
import {MockVerifier, TestToken} from "../mocks/Mocks.sol";

/// Drives the failsafe vault through every phase (active, wound down, final) with FORGED proofs:
/// the MockVerifier accepts any journal its seal names, so the handler can make any update
/// (claims above deposits, exits above claims) and any reclaim (any nullifier, any recipient)
/// that the vault's own checks let through. Admin and freezer actions are mixed in. Every vault
/// revert is caught and counted; the invariants compare the vault with ghost totals.
contract FailsafeHandler is CommonBase, StdCheats, StdUtils {
    bytes32 constant G2 = keccak256("g2");
    bytes32 constant G3 = keccak256("g3");
    bytes32 constant G4 = keccak256("g4");
    address constant ETH = address(0);

    V public immutable v;
    MockVerifier public immutable mv;
    TestToken public immutable usdc;
    address public immutable admin;
    address public immutable freezer;
    address public immutable user;
    address[2] public tokens;
    uint256 internal nonce;

    // ghosts: what left the vault as principal, per token
    mapping(address => uint256) public paidReleases;
    mapping(address => uint256) public paidReclaims;
    uint256 public reclaimsOk;
    uint256 public reclaimsRefused;
    uint256 public updatesOk;
    bool public sawWindDown;
    bytes32 public firstFinal;

    constructor(V v_, MockVerifier mv_, TestToken usdc_, address admin_, address freezer_) {
        v = v_;
        mv = mv_;
        usdc = usdc_;
        admin = admin_;
        freezer = freezer_;
        user = address(0xD00D);
        tokens = [address(usdc_), ETH];
        usdc_.mint(user, 1e18);
        vm.prank(user);
        usdc_.approve(address(v_), type(uint256).max);
        vm.deal(user, 1e9 ether);
    }

    function _tok(uint256 s) internal view returns (address) {
        return tokens[s % 2];
    }

    function deposit(uint256 seed) external {
        address t = _tok(seed);
        uint32 di = uint32((seed >> 8) % (t == ETH ? 1 : 2));
        bytes32 c = keccak256(abi.encode("c", ++nonce));
        vm.prank(user);
        if (t == ETH) {
            try v.depositETH{value: 1 ether + 0.01 ether}(di, c) {} catch {}
        } else {
            try v.deposit(t, di, c) {} catch {}
        }
    }

    function update(uint256 seed, uint64 nClaims, uint8 nExits) external {
        address t = _tok(seed);
        uint32 di = uint32((seed >> 8) % (t == ETH ? 1 : 2));
        // claims in a third of the updates, 1-2 at a time (sometimes above deposits: refused while
        // active, clamped after wind-down, ignored after the final)
        bool withClaims = nClaims % 3 == 0;
        V.ClaimDelta[] memory cl = new V.ClaimDelta[](withClaims ? 1 : 0);
        if (withClaims) cl[0] = V.ClaimDelta(t, di, uint64(bound(nClaims >> 2, 1, 2)));
        V.Exit[] memory ex = new V.Exit[](bound(nExits, 0, 3));
        for (uint256 i; i < ex.length; ++i) ex[i] = V.Exit(t, uint32((seed >> (16 + i)) % (t == ETH ? 1 : 2)), keccak256(abi.encode("e", ++nonce)));
        bytes memory j = abi.encode(V.UpdateJournal(v.cfgId(), v.g2State(), keccak256(abi.encode("s", ++nonce)), new bytes32[](0), cl, ex));
        try v.update(j, mv.sealFor(G2, sha256(j))) {
            updatesOk++;
        } catch {}
        if (firstFinal == bytes32(0)) firstFinal = v.finalState();
    }

    function release(uint256 seed) external {
        address t = _tok(seed);
        uint32 di = uint32((seed >> 8) % (t == ETH ? 1 : 2));
        bytes32 root = v.exitRoot();
        bytes32 nf = keccak256(abi.encode("rel", ++nonce));
        address to = address(uint160(uint256(keccak256(abi.encode("to", nonce)))));
        (,,,,, uint256[] memory denoms,, uint256[] memory wf,) = v.tokenConfig(t);
        bytes memory s = mv.sealFor(G3, sha256(abi.encode(v.cfgId(), root, nf, t, denoms[di], to, address(0), uint256(0))));
        try v.release(s, root, nf, t, di, to, address(0), 0) {
            paidReleases[t] += denoms[di] - wf[di];
        } catch {}
    }

    function reclaim(uint256 seed) external {
        address t = _tok(seed);
        uint32 di = uint32((seed >> 8) % (t == ETH ? 1 : 2));
        bytes32 root = v.depositRoot();
        // sometimes a replayed nullifier
        bytes32 nf = keccak256(abi.encode("rc", seed % 3 == 0 ? nonce : ++nonce));
        address to = address(uint160(uint256(keccak256(abi.encode("rto", seed)))) | 1);
        (,,,,, uint256[] memory denoms,,,) = v.tokenConfig(t);
        bytes memory s = mv.sealFor(G4, sha256(abi.encode(v.cfgId(), root, v.finalState(), nf, t, denoms[di], to, address(0), uint256(0))));
        try v.reclaim(s, root, nf, t, di, to, address(0), 0) {
            paidReclaims[t] += denoms[di];
            reclaimsOk++;
        } catch {
            reclaimsRefused++;
        }
    }

    function windDown(uint256 seed) external {
        if (seed % 8 != 0) return; // rarer than deposits, so deposits pile up first
        vm.prank(admin);
        try v.windDown() {
            sawWindDown = true;
        } catch {}
    }

    /// Admin and freezer actions that exist on every vault: none may move principal.
    function privileged(uint256 seed) external {
        uint256 a = seed % 9;
        address t = _tok(seed >> 8);
        if (a == 0) {
            vm.prank(freezer);
            try v.freezeAll(true, true) {} catch {}
        } else if (a == 1) {
            vm.prank(admin);
            try v.unfreezeAll(true, true) {} catch {}
        } else if (a == 2) {
            vm.prank(freezer);
            try v.freezeReclaims() {} catch {}
        } else if (a == 3) {
            vm.prank(admin);
            try v.unfreezeReclaims() {} catch {}
        } else if (a == 4) {
            vm.prank(admin);
            try v.setCaps(t, uint64(bound(seed >> 16, 0, 2 days)), type(uint256).max, bound(seed >> 40, 0, 5000e6)) {} catch {}
        } else if (a == 5) {
            vm.prank(admin);
            try v.withdrawFees(t) {} catch {}
        } else if (a == 6) {
            vm.prank(admin);
            try v.deregisterToken(t) {} catch {}
        } else if (a == 7) {
            vm.prank(admin);
            try v.setFeeRecipient(admin) {} catch {}
        } else {
            vm.warp(block.timestamp + bound(seed >> 16, 1, 3 days));
        }
    }
}

contract FailsafeInvariantTest is Test {
    V v;
    MockVerifier mv;
    TestToken usdc;
    FailsafeHandler h;
    address admin = makeAddr("admin");
    address freezer = makeAddr("freezer");

    function setUp() public {
        mv = new MockVerifier();
        v = new V(mv, keccak256("g2"), keccak256("g3"), keccak256("g4"), keccak256("lez"), keccak256("zone"), keccak256("prog"),
            keccak256("state0"), 0, admin, freezer, makeAddr("feeTo"));
        usdc = new TestToken("USDC", 6);
        uint256[] memory d = new uint256[](2);
        (d[0], d[1]) = (100e6, 1000e6);
        uint256[] memory df = new uint256[](2);
        (df[0], df[1]) = (1e6, 10e6);
        uint256[] memory wf = new uint256[](2);
        (wf[0], wf[1]) = (0.5e6, 5e6);
        uint256[] memory ed = new uint256[](1);
        ed[0] = 1 ether;
        uint256[] memory ef = new uint256[](1);
        ef[0] = 0.01 ether;
        vm.startPrank(admin);
        v.registerToken(address(usdc), 6, d, df, wf, 0);
        v.registerToken(address(0), 18, ed, ef, ef, 0);
        vm.stopPrank();
        h = new FailsafeHandler(v, mv, usdc, admin, freezer);
        targetContract(address(h));
    }

    function _bal(address t) internal view returns (uint256) {
        return t == address(0) ? address(v).balance : usdc.balanceOf(address(v));
    }

    /// claims + reclaims <= deposits and releases <= burns per (token, denomination); burned <=
    /// claimed value per token; the balance is exactly principal still owed plus accrued fees;
    /// the reclaims paid never exceed (deposits - claims) by value.
    function invariant_conservation() public view {
        address[2] memory ts = [address(usdc), address(0)];
        for (uint256 x; x < 2; ++x) {
            address t = ts[x];
            (,,,,, uint256[] memory denoms,,, uint256 fees) = v.tokenConfig(t);
            // By value per token: wrapped tokens are fungible across denominations on LEZ, so a
            // release may be in a denomination with fewer deposits than releases (the vault's own
            // bound is burned value <= claimed value per token, BridgeVault's header).
            uint256 depositedValue;
            uint256 releasedValue;
            uint256 unclaimedValue;
            uint256 reclaimedValue;
            for (uint256 d; d < denoms.length; ++d) {
                (uint64 dep, uint64 cl, uint64 bu, uint64 rel) = v.counters(t, d);
                uint64 rc = v.reclaims(t, d);
                assertLe(uint256(cl) + rc, dep, "claims + reclaims <= deposits");
                assertLe(rel, bu, "releases <= burns");
                depositedValue += uint256(dep) * denoms[d];
                releasedValue += uint256(rel) * denoms[d];
                unclaimedValue += (uint256(dep) - cl) * denoms[d];
                reclaimedValue += uint256(rc) * denoms[d];
            }
            assertLe(v.burnedValue(t), v.claimedValue(t), "burned <= claimed");
            assertLe(releasedValue + reclaimedValue, depositedValue, "released + reclaimed <= deposited, by value");
            assertEq(_bal(t), depositedValue - releasedValue - reclaimedValue + fees, "balance = principal owed + accrued fees");
            assertEq(h.paidReclaims(t), reclaimedValue, "reclaimed = sum of reclaims * denom");
            assertLe(reclaimedValue, unclaimedValue, "reclaimed value <= (deposits - claims) by value");
        }
    }

    /// The admin and the freezer never receive principal (the admin may be the fee recipient:
    /// at most the fees accrued).
    function invariant_noPrivilegedPayout() public view {
        assertEq(usdc.balanceOf(freezer), 0);
        assertEq(freezer.balance, 0);
        // every USDC the admin holds came from withdrawFees: deposit fees + withdraw fees only
        assertLe(usdc.balanceOf(admin), _feesEverAccrued(address(usdc)));
        assertLe(admin.balance, _feesEverAccrued(address(0)));
    }

    function _feesEverAccrued(address t) internal view returns (uint256 s) {
        (,,,,, uint256[] memory denoms, uint256[] memory df, uint256[] memory wf,) = v.tokenConfig(t);
        for (uint256 d; d < denoms.length; ++d) {
            (uint64 dep,,, uint64 rel) = v.counters(t, d);
            s += uint256(dep) * df[d] + uint256(rel) * wf[d];
        }
    }

    /// Once wound down, always wound down; the first final never changes; no deposit after it.
    function invariant_windDownIrreversible() public view {
        if (h.sawWindDown()) assertTrue(v.windDownAt() != 0, "wind-down undone");
        if (h.firstFinal() != bytes32(0)) assertEq(v.finalState(), h.firstFinal(), "final moved");
        if (v.finalState() != bytes32(0)) assertTrue(v.windDownAt() != 0);
        if (v.windDownAt() == 0) {
            for (uint256 d; d < 2; ++d) assertEq(v.reclaims(address(usdc), d), 0, "reclaim before wind-down");
        }
    }

    // Reachability (checked 2026-11-17 by making a successful handler reclaim revert on purpose:
    // every invariant then failed within its 64 runs): the campaign does reach wind-down, the
    // final and paid reclaims, so the invariants above are exercised in the reclaim phase.
}

/// Fuzz: any number of deposits and claims, then forged reclaims until the vault refuses. The sum
/// reclaimed is exactly the unclaimed remainder, never more.
contract FailsafeFuzzTest is Test {
    function testFuzz_SolvencyWithReclaims(uint8 nDep, uint8 nClaim, uint8 attempts) public {
        nDep = uint8(bound(nDep, 0, 20));
        nClaim = uint8(bound(nClaim, 0, 30)); // may exceed deposits: clamped after wind-down
        attempts = uint8(bound(attempts, 0, 40));
        MockVerifier mv = new MockVerifier();
        address admin = address(0xA);
        V v = new V(mv, keccak256("g2"), keccak256("g3"), keccak256("g4"), keccak256("lez"), keccak256("zone"), keccak256("prog"),
            keccak256("s0"), 0, admin, admin, address(0xFEE));
        TestToken usdc = new TestToken("USDC", 6);
        uint256[] memory d = new uint256[](1);
        d[0] = 100e6;
        uint256[] memory z = new uint256[](1);
        vm.prank(admin);
        v.registerToken(address(usdc), 6, d, z, z, 0);
        usdc.mint(address(this), 1e18);
        usdc.approve(address(v), type(uint256).max);
        for (uint256 i; i < nDep; ++i) v.deposit(address(usdc), 0, keccak256(abi.encode(i)));
        vm.prank(admin);
        v.windDown();
        V.ClaimDelta[] memory cl = new V.ClaimDelta[](1);
        cl[0] = V.ClaimDelta(address(usdc), 0, nClaim);
        bytes memory j = abi.encode(V.UpdateJournal(v.cfgId(), v.g2State(), keccak256("final"), new bytes32[](0), cl, new V.Exit[](0)));
        v.update(j, mv.sealFor(keccak256("g2"), sha256(j)));
        bytes32 root = v.depositRoot();
        uint256 paid;
        for (uint256 i; i < attempts; ++i) {
            address to = address(uint160(0x1000 + i));
            bytes32 nf = keccak256(abi.encode("nf", i));
            bytes memory s = mv.sealFor(keccak256("g4"), sha256(abi.encode(v.cfgId(), root, v.finalState(), nf, address(usdc), uint256(100e6), to, address(0), uint256(0))));
            try v.reclaim(s, root, nf, address(usdc), 0, to, address(0), 0) {
                paid += 100e6;
            } catch (bytes memory err) {
                assertEq(bytes4(err), V.ReclaimsExceedDeposits.selector, "the only refusal here is the bound");
            }
        }
        uint256 claimed = nClaim > nDep ? nDep : nClaim;
        uint256 room = (uint256(nDep) - claimed) * 100e6;
        assertEq(paid, attempts * 100e6 < room ? attempts * 100e6 : room, "reclaims stop exactly at deposits - claims");
        assertEq(usdc.balanceOf(address(v)), uint256(nDep) * 100e6 - paid);
    }
}
