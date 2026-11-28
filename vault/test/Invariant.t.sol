// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {Test, console} from "forge-std/Test.sol";
import {CommonBase} from "forge-std/Base.sol";
import {StdCheats} from "forge-std/StdCheats.sol";
import {StdUtils} from "forge-std/StdUtils.sol";
import {BridgeVault} from "../src/BridgeVault.sol";
import {IRiscZeroVerifier} from "../src/IRiscZeroVerifier.sol";
import {MockVerifier, TestToken, NoReturnToken} from "./mocks/Mocks.sol";

// actions (indices into ok/rej)
uint256 constant DEP_USDC = 0;
uint256 constant DEP_USDT = 1;
uint256 constant DEP_ETH = 2;
uint256 constant UPD_CLAIMS = 3;
uint256 constant UPD_EXITS = 4;
uint256 constant RELEASE = 5;
uint256 constant REPLAY = 6;
uint256 constant WARP = 7;
uint256 constant SET_CAPS = 8;
uint256 constant SET_GLOBAL_CAPS = 9;
uint256 constant FREEZE = 10;
uint256 constant UNFREEZE = 11;
uint256 constant SET_DELAY = 12;
uint256 constant SET_MENU = 13;
uint256 constant WITHDRAW_FEES = 14;
uint256 constant N_ACTIONS = 15;

function actionName(uint256 a) pure returns (string memory) {
    string[N_ACTIONS] memory n = [
        "deposit USDC (6 dec)",
        "deposit USDT-style",
        "deposit ETH",
        "update: claims (+exit)",
        "update: exits (+claim)",
        "release",
        "release replay (spent nf)",
        "warp",
        "setCaps",
        "setGlobalCaps",
        "freeze / freezeAll",
        "unfreeze / unfreezeAll",
        "setReleaseDelay",
        "setRelayFeeMenu",
        "withdrawFees"
    ];
    return n[a];
}

/// A1: stateful invariant suite. The handler drives the vault with deposits (6-decimal ERC-20,
/// USDT-style token with no return values, native ETH; every denomination), G2 updates carrying
/// claims and/or exits in any denomination (sized so some are valid and some exceed the
/// bounds), releases (fee 0 or a menu value, sometimes off the menu; random recipient and
/// relayer), replays of spent nullifiers, non-advancing update journals, time warps, caps, freezes, delay and menu changes, and
/// fee withdrawals (the admin actions live in AdminHandler below, for contract size). It
/// catches every vault revert and counts it, so `fail_on_revert = true` only trips on a
/// handler bug, and keeps ghost totals that the invariants compare against the vault's own
/// counters and balances.
contract VaultHandler is CommonBase, StdCheats, StdUtils {
    address constant ETH = address(0);

    BridgeVault public immutable v;
    MockVerifier public immutable mv;
    bytes32 public immutable g2;
    bytes32 public immutable g3;
    address public immutable feeTo;
    address[3] public tokens;

    /// Handler-side clock: every action re-warps to it first (no reliance on block.timestamp
    /// surviving between calls, and no via-IR re-read of TIMESTAMP).
    uint256 public clock;
    uint256 internal nonce;

    struct Ghost {
        uint64 deposits;
        uint64 claims;
        uint64 burns;
        uint64 releases;
    }

    mapping(address => mapping(uint256 => Ghost)) internal _g;
    mapping(address => uint256) public gDeposited; // principal value deposited
    mapping(address => uint256) public gReleased; // principal value released (denominations)
    mapping(address => uint256) public gClaimed;
    mapping(address => uint256) public gBurned;
    mapping(address => uint256) public gFeesIn; // deposit + withdraw fees taken
    mapping(address => uint256) public gFeesOut; // withdrawn by withdrawFees

    bytes32[] public depositRoots;
    bytes32[] public exitRoots;
    mapping(bytes32 => uint64) public exitSeenAt;
    bytes32[] public spent;
    mapping(bytes32 => uint256) public payCount;
    mapping(bytes32 => bytes32) internal spentRoot; // the (releasable) root it was spent under
    bytes32[] internal usedCommitments;
    /// A successful release or fee withdrawal that moved a different amount than expected.
    uint256 public payoutMismatch;

    uint256[N_ACTIONS] public ok;
    uint256[N_ACTIONS] public rej;
    uint256 public replaysSkipped; // no spent nullifier yet
    /// Updates whose journal had newState == prevState that the vault applied (must stay 0).
    uint256 public noAdvanceAccepted;
    uint256[3] public releasesOkByToken;
    /// Refusals per action and revert selector (what the refusals were).
    mapping(uint256 => mapping(bytes4 => uint256)) public reasonCount;
    uint256[3] public depositsOkByDenom; // by denomination index, all tokens

    constructor(BridgeVault v_, MockVerifier mv_, bytes32 g2_, bytes32 g3_, address feeTo_, address usdc_, address usdt_, uint256 clock_) {
        v = v_;
        mv = mv_;
        g2 = g2_;
        g3 = g3_;
        feeTo = feeTo_;
        tokens = [usdc_, usdt_, ETH];
        clock = clock_;
        TestToken(usdc_).approve(address(v_), type(uint256).max);
        NoReturnToken(usdt_).approve(address(v_), type(uint256).max);
    }

    receive() external payable {}

    // ------------------------------------------------------------------ views for the invariants

    function ghost(address t, uint256 d) external view returns (Ghost memory) {
        return _g[t][d];
    }

    function depositRootCount() external view returns (uint256) {
        return depositRoots.length;
    }

    function exitRootCount() external view returns (uint256) {
        return exitRoots.length;
    }

    function spentCount() external view returns (uint256) {
        return spent.length;
    }


    // ------------------------------------------------------------------ helpers

    function _terms(address t)
        internal
        view
        returns (uint256[] memory denoms, uint256[] memory dfees, uint256[] memory wfees)
    {
        (,,,,, denoms, dfees, wfees,) = v.tokenConfig(t);
    }

    function _bal(address t, address who) internal view returns (uint256) {
        if (t == ETH) return who.balance;
        return TestToken(t).balanceOf(who);
    }

    function _rejected(uint256 a, bytes memory err) internal {
        rej[a]++;
        bytes4 sel = err.length >= 4 ? bytes4(err) : bytes4(0);
        reasonCount[a][sel]++;
    }

    function _h(uint256 a, uint256 b) internal pure returns (uint256) {
        return uint256(keccak256(abi.encode(a, b)));
    }

    /// 0x1001..0x1004, plus the vault and address(0), which `release` must refuse (BadRecipient).
    function _recipient(uint256 s) internal view returns (address) {
        uint256 k = s % 6;
        return k == 4 ? address(v) : k == 5 ? address(0) : address(uint160(0x1001 + k));
    }

    /// Overlaps the recipients; address(0) and the vault (refused with a fee: ZeroRelayer,
    /// BadRelayer; with fee 0 accepted and paid nothing).
    function _relayer(uint256 s) internal view returns (address) {
        uint256 k = s % 6;
        return k == 4 ? address(0) : k == 5 ? address(v) : address(uint160(0x1003 + k));
    }

    /// Called once from setUp (not a fuzz target): 3 deposits of every token and denomination,
    /// one update claiming them all and exiting 2 of each, then past the release delay. Every
    /// fuzz run starts from this state, so releases, replays and fee withdrawals are reachable
    /// from the first call instead of only after a lucky deposit/claim/exit/warp sequence.
    function bootstrap() external {
        uint256 nClaims;
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](7);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](14);
        for (uint256 ti; ti < 3; ++ti) {
            (uint256[] memory denoms,,) = _terms(tokens[ti]);
            for (uint256 d; d < denoms.length; ++d) {
                for (uint256 k; k < 3; ++k) _deposit(ti, ti, d, 1);
                cl[nClaims] = BridgeVault.ClaimDelta(tokens[ti], uint32(d), 3);
                ex[2 * nClaims] = BridgeVault.Exit(tokens[ti], uint32(d), keccak256(abi.encode("boot", ti, d, 0)));
                ex[2 * nClaims + 1] = BridgeVault.Exit(tokens[ti], uint32(d), keccak256(abi.encode("boot", ti, d, 1)));
                nClaims++;
            }
        }
        require(nClaims == 7, "bootstrap layout");
        // seed bits: a known deposit root, the current g2 state
        _submit(UPD_CLAIMS, cl, ex, type(uint256).max);
        require(ok[UPD_CLAIMS] == 1, "bootstrap update failed");
        clock += 1 days;
        vm.warp(clock);
        // the bootstrap doesn't count toward the coverage table
        for (uint256 a; a < N_ACTIONS; ++a) (ok[a], rej[a]) = (0, 0);
        for (uint256 d; d < 3; ++d) depositsOkByDenom[d] = 0;
    }

    // ------------------------------------------------------------------ deposits

    function depositUsdc(uint256 dSeed, uint256 cSeed) external {
        _deposit(DEP_USDC, 0, dSeed, cSeed);
    }

    function depositUsdt(uint256 dSeed, uint256 cSeed) external {
        _deposit(DEP_USDT, 1, dSeed, cSeed);
    }

    function depositEth(uint256 dSeed, uint256 cSeed) external {
        _deposit(DEP_ETH, 2, dSeed, cSeed);
    }

    function _deposit(uint256 a, uint256 ti, uint256 dSeed, uint256 cSeed) internal {
        vm.warp(clock);
        address t = tokens[ti];
        (uint256[] memory denoms, uint256[] memory dfees,) = _terms(t);
        uint256 d = dSeed % denoms.length;
        bytes32 c;
        // one in 16 reuses a commitment (always refused)
        if (cSeed % 16 == 0 && usedCommitments.length != 0) c = usedCommitments[(cSeed >> 8) % usedCommitments.length];
        else c = keccak256(abi.encode("commit", ++nonce));
        bool success;
        if (t == ETH) {
            try v.depositETH{value: denoms[d] + dfees[d]}(uint32(d), c) {
                success = true;
            } catch (bytes memory err) {
                _rejected(a, err);
            }
        } else {
            try v.deposit(t, uint32(d), c) {
                success = true;
            } catch (bytes memory err) {
                _rejected(a, err);
            }
        }
        if (!success) return;
        ok[a]++;
        depositsOkByDenom[d]++;
        _g[t][d].deposits++;
        gDeposited[t] += denoms[d];
        gFeesIn[t] += dfees[d];
        usedCommitments.push(c);
        depositRoots.push(v.depositRoot());
    }

    // ------------------------------------------------------------------ updates (G2)

    /// One claim delta (count up to 2 more than is unclaimed, so some exceed deposits), against
    /// a known deposit root (one in 16: an unknown root), and one time in four an exit too.
    function updateClaims(uint256 tSeed, uint256 dSeed, uint256 nSeed, uint256 seed) external {
        vm.warp(clock);
        address t = tokens[tSeed % 3];
        (uint256[] memory denoms,,) = _terms(t);
        uint256 d = dSeed % denoms.length;
        Ghost memory k = _g[t][d];
        uint64 n = uint64(_bound(nSeed, 1, uint256(k.deposits - k.claims) + 2));
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](1);
        cl[0] = BridgeVault.ClaimDelta(t, uint32(d), n);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](seed % 4 == 0 ? 1 : 0);
        if (ex.length != 0) {
            uint256 ed = (seed >> 8) % denoms.length; // any denomination of the same token
            ex[0] = BridgeVault.Exit(t, uint32(ed), keccak256(abi.encode("exit", ++nonce)));
        }
        _submit(UPD_CLAIMS, cl, ex, seed);
    }

    /// 1 to 4 exits across tokens and denominations. Two times in three an exit picks a
    /// denomination that still fits the token's unburned claimed value, otherwise any.
    /// One time in four a claim delta rides along.
    function updateExits(uint256 nSeed, uint256 seed) external {
        vm.warp(clock);
        uint256 n = _bound(nSeed, 1, 4);
        BridgeVault.Exit[] memory ex = new BridgeVault.Exit[](n);
        uint256[3] memory budget;
        for (uint256 ti; ti < 3; ++ti) budget[ti] = gClaimed[tokens[ti]] - gBurned[tokens[ti]];
        for (uint256 i; i < n; ++i) {
            uint256 s = _h(seed, i);
            uint256 ti = s % 3;
            address t = tokens[ti];
            (uint256[] memory denoms,,) = _terms(t);
            uint256 d = (s >> 16) % denoms.length;
            if ((s >> 8) % 3 != 0) {
                uint256 fit;
                for (uint256 j; j < denoms.length; ++j) if (denoms[j] <= budget[ti]) fit++;
                if (fit != 0) {
                    uint256 pick = (s >> 24) % fit;
                    for (uint256 j; j < denoms.length; ++j) {
                        if (denoms[j] <= budget[ti]) {
                            if (pick == 0) {
                                d = j;
                                break;
                            }
                            pick--;
                        }
                    }
                }
            }
            budget[ti] = budget[ti] >= denoms[d] ? budget[ti] - denoms[d] : 0;
            ex[i] = BridgeVault.Exit(t, uint32(d), keccak256(abi.encode("exit", ++nonce)));
        }
        BridgeVault.ClaimDelta[] memory cl = new BridgeVault.ClaimDelta[](seed % 4 == 0 ? 1 : 0);
        if (cl.length != 0) {
            address t = tokens[(seed >> 32) % 3];
            (uint256[] memory denoms,,) = _terms(t);
            uint256 d = (seed >> 40) % denoms.length;
            Ghost memory k = _g[t][d];
            if (k.deposits > k.claims) cl[0] = BridgeVault.ClaimDelta(t, uint32(d), 1);
            else cl[0] = BridgeVault.ClaimDelta(t, uint32(d), 0);
        }
        _submit(UPD_EXITS, cl, ex, seed);
    }

    function _submit(uint256 a, BridgeVault.ClaimDelta[] memory cl, BridgeVault.Exit[] memory ex, uint256 seed) internal {
        bytes32[] memory roots = new bytes32[](1);
        uint256 r = (seed >> 48) % 16;
        if (r == 0) roots[0] = keccak256(abi.encode("unknown root", seed));
        else if (depositRoots.length == 0) roots[0] = v.depositRoot(); // the empty root
        else roots[0] = depositRoots[(seed >> 56) % depositRoots.length];
        bytes32 prev = (seed >> 64) % 16 == 0 ? keccak256("stale") : v.g2State();
        // one in 16: a journal that does not advance the state (refused: NoStateAdvance, or
        // StaleState when prev is stale too)
        bool noAdvance = (seed >> 72) % 16 == 0;
        bytes32 next = noAdvance ? prev : keccak256(abi.encode("g2", ++nonce));
        bytes memory j = abi.encode(BridgeVault.UpdateJournal(v.cfgId(), prev, next, roots, cl, ex));
        bytes memory seal = mv.sealFor(g2, sha256(j));
        try v.update(j, seal) {
            ok[a]++;
            if (noAdvance) noAdvanceAccepted++;
        } catch (bytes memory err) {
            _rejected(a, err);
            return;
        }
        for (uint256 i; i < cl.length; ++i) {
            (uint256[] memory denoms,,) = _terms(cl[i].token);
            _g[cl[i].token][cl[i].denomIndex].claims += cl[i].count;
            gClaimed[cl[i].token] += uint256(cl[i].count) * denoms[cl[i].denomIndex];
        }
        for (uint256 i; i < ex.length; ++i) {
            (uint256[] memory denoms,,) = _terms(ex[i].token);
            _g[ex[i].token][ex[i].denomIndex].burns++;
            gBurned[ex[i].token] += denoms[ex[i].denomIndex];
        }
        if (ex.length != 0) {
            bytes32 root = v.exitRoot();
            exitRoots.push(root);
            exitSeenAt[root] = uint64(clock);
        }
    }

    // ------------------------------------------------------------------ releases (G3)

    struct Rel {
        bytes32 root;
        address t;
        uint256 d;
        uint256 denom;
        uint256 wfee;
        uint256 fee;
        address to;
        address rel;
        bytes32 nf;
    }

    function release(uint256 rootSeed, uint256 tSeed, uint256 dSeed, uint256 feeSeed, uint256 who) external {
        vm.warp(clock);
        Rel memory r;
        r.root = exitRoots.length == 0 ? keccak256("no exit root yet") : exitRoots[rootSeed % exitRoots.length];
        r.t = tokens[tSeed % 3];
        (uint256[] memory denoms,, uint256[] memory wfees) = _terms(r.t);
        r.d = dSeed % denoms.length;
        if (dSeed % 4 != 0) {
            // prefer a denomination with an unreleased burn
            for (uint256 j; j < denoms.length; ++j) {
                uint256 dd = (r.d + j) % denoms.length;
                Ghost memory k = _g[r.t][dd];
                if (k.burns > k.releases) {
                    r.d = dd;
                    break;
                }
            }
        }
        r.denom = denoms[r.d];
        r.wfee = wfees[r.d];
        uint256[] memory menu = v.relayFeeMenu(r.t, uint32(r.d));
        uint256 mode = feeSeed % 4;
        if (mode == 1 || mode == 2) r.fee = menu.length == 0 ? 0 : menu[(feeSeed >> 8) % menu.length];
        else if (mode == 3) r.fee = (menu.length == 0 ? 0 : menu[0]) + 1; // usually off the menu
        r.to = _recipient(who);
        r.rel = _relayer(who >> 8);
        r.nf = keccak256(abi.encode("nf", ++nonce));
        bytes memory seal = mv.sealFor(g3, sha256(abi.encode(v.cfgId(), r.root, r.nf, r.t, r.denom, r.to, r.rel, r.fee)));
        uint256 toBefore = _bal(r.t, r.to);
        uint256 relBefore = _bal(r.t, r.rel);
        try v.release(seal, r.root, r.nf, r.t, uint32(r.d), r.to, r.rel, r.fee) {
            ok[RELEASE]++;
        } catch (bytes memory err) {
            _rejected(RELEASE, err);
            return;
        }
        releasesOkByToken[tSeed % 3]++;
        if (r.to == r.rel) {
            if (_bal(r.t, r.to) - toBefore != r.denom - r.wfee) payoutMismatch++;
        } else {
            if (_bal(r.t, r.to) - toBefore != r.denom - r.wfee - r.fee) payoutMismatch++;
            // a release names the vault as relayer only with fee 0 (BadRelayer otherwise)
            if (r.rel == address(v)) {
                if (r.fee != 0) payoutMismatch++;
            } else if (r.rel != address(0) && _bal(r.t, r.rel) - relBefore != r.fee) {
                payoutMismatch++;
            }
        }
        _g[r.t][r.d].releases++;
        gReleased[r.t] += r.denom;
        gFeesIn[r.t] += r.wfee;
        spent.push(r.nf);
        spentRoot[r.nf] = r.root;
        payCount[r.nf]++;
    }

    /// A spent nullifier again, with a valid seal for other arguments (the root it was spent
    /// under, which stays releasable, or the latest root; any token, denomination and
    /// recipient; fee 0). Must always be refused.
    function releaseReplay(uint256 idx, uint256 tSeed, uint256 dSeed, uint256 who) external {
        vm.warp(clock);
        if (spent.length == 0) {
            replaysSkipped++;
            return;
        }
        bytes32 nf = spent[idx % spent.length];
        bytes32 root = (idx >> 128) % 4 == 0 ? exitRoots[exitRoots.length - 1] : spentRoot[nf];
        address t = tokens[tSeed % 3];
        (uint256[] memory denoms,,) = _terms(t);
        uint256 d = dSeed % denoms.length;
        address to = _recipient(who % 4); // valid recipients only, so the nullifier check is what refuses
        bytes memory seal = mv.sealFor(g3, sha256(abi.encode(v.cfgId(), root, nf, t, denoms[d], to, address(0), uint256(0))));
        try v.release(seal, root, nf, t, uint32(d), to, address(0), 0) {
            ok[REPLAY]++;
            payCount[nf]++; // breaks the invariant
        } catch (bytes memory err) {
            _rejected(REPLAY, err);
        }
    }

    // ------------------------------------------------------------------ time and fees

    function warp(uint256 dt) external {
        clock += _bound(dt, 0, 3 days);
        vm.warp(clock);
        ok[WARP]++;
    }

    /// Used only by AdminHandler when advancing past the admin timelock (not a fuzz target).
    function advanceClock(uint256 dt) external {
        clock += dt;
        vm.warp(clock);
    }

    /// Permissionless; pays exactly the accrued fees to the fee recipient.
    function withdrawFees(uint256 tSeed) external {
        vm.warp(clock);
        address t = tokens[tSeed % 3];
        (,,,,,,,, uint256 accrued) = v.tokenConfig(t);
        uint256 before = _bal(t, feeTo);
        try v.withdrawFees(t) {
            ok[WITHDRAW_FEES]++;
        } catch {
            rej[WITHDRAW_FEES]++;
            return;
        }
        if (_bal(t, feeTo) - before != accrued) payoutMismatch++;
        gFeesOut[t] += accrued;
    }
}

/// The admin-side actions of the A1 campaign (caps, freezes, delay, menu), split from
/// VaultHandler so neither handler exceeds the EIP-170 size limit (`forge build --sizes` in CI
/// checks every non-test contract). Shares VaultHandler's clock.
contract AdminHandler is CommonBase, StdCheats, StdUtils {
    BridgeVault public immutable v;
    VaultHandler public immutable main;
    address public immutable admin;
    address public immutable freezer;
    address[3] public tokens;

    uint256[N_ACTIONS] public ok;
    uint256[N_ACTIONS] public rej;

    constructor(BridgeVault v_, VaultHandler main_, address admin_, address freezer_) {
        v = v_;
        main = main_;
        admin = admin_;
        freezer = freezer_;
        tokens = [main_.tokens(0), main_.tokens(1), main_.tokens(2)];
    }

    function _terms(address t) internal view returns (uint256[] memory denoms) {
        (,,,,, denoms,,,) = v.tokenConfig(t);
    }

    function _h(uint256 a, uint256 b) internal pure returns (uint256) {
        return uint256(keccak256(abi.encode(a, b)));
    }

    function _window(uint256 s) internal pure returns (uint64) {
        uint64[4] memory w = [uint64(0), 1 hours, 1 days, 7 days];
        return w[s % 4];
    }

    function _exec(BridgeVault.Action action, bytes memory params) internal {
        vm.prank(admin);
        bytes32 id = v.schedule(action, params);
        uint64 eta = v.scheduledEta(id);
        uint256 nowTs = main.clock();
        if (eta > nowTs) main.advanceClock(eta - nowTs);
        vm.prank(admin);
        v.execute(action, params);
    }

    function setCaps(uint256 tSeed, uint256 wSeed, uint256 depK, uint256 relK) external {
        vm.warp(main.clock());
        address t = tokens[tSeed % 3];
        uint256[] memory denoms = _terms(t);
        uint256 unit = denoms[0];
        uint256 dc = depK % 8 == 0 ? type(uint256).max : unit * _bound(depK, 0, 200);
        uint256 rc = relK % 8 == 0 ? type(uint256).max : unit * _bound(relK, 0, 200);
        uint64 w = _window(wSeed);
        vm.prank(admin);
        try v.setCaps(t, w, dc, rc) {
            ok[SET_CAPS]++;
        } catch (bytes memory err) {
            if (bytes4(err) == BridgeVault.TimelockRequired.selector) {
                _exec(BridgeVault.Action.SetCaps, abi.encode(t, w, dc, rc));
                ok[SET_CAPS]++;
            } else {
                rej[SET_CAPS]++;
            }
        }
    }

    /// Global units: USD with 6 decimals (weights set in setUp: USDC and USDT 1e18, ETH 3e9).
    function setGlobalCaps(uint256 wSeed, uint256 depK, uint256 relK) external {
        vm.warp(main.clock());
        uint256 dc = depK % 8 == 0 ? type(uint256).max : 100e6 * _bound(depK, 0, 400);
        uint256 rc = relK % 8 == 0 ? type(uint256).max : 100e6 * _bound(relK, 0, 400);
        uint64 w = _window(wSeed);
        vm.prank(admin);
        try v.setGlobalCaps(w, dc, rc) {
            ok[SET_GLOBAL_CAPS]++;
        } catch (bytes memory err) {
            if (bytes4(err) == BridgeVault.TimelockRequired.selector) {
                _exec(BridgeVault.Action.SetGlobalCaps, abi.encode(w, dc, rc));
                ok[SET_GLOBAL_CAPS]++;
            } else {
                rej[SET_GLOBAL_CAPS]++;
            }
        }
    }

    /// target 0..2: one token; 3: global. Caller: the freezer or the admin.
    function freeze(uint256 target, uint256 flags, bool byAdmin) external {
        vm.warp(main.clock());
        bool dep = flags % 2 == 1;
        bool rel = (flags >> 1) % 2 == 1;
        vm.prank(byAdmin ? admin : freezer);
        if (target % 4 == 3) {
            try v.freezeAll(dep, rel) {
                ok[FREEZE]++;
            } catch {
                rej[FREEZE]++;
            }
        } else {
            try v.freeze(tokens[target % 4], dep, rel) {
                ok[FREEZE]++;
            } catch {
                rej[FREEZE]++;
            }
        }
    }

    function unfreeze(uint256 target, uint256 flags) external {
        vm.warp(main.clock());
        if (flags % 2 == 0) {
            // clear every flag (global and each token) through one timelock window, so freezes don't starve the other actions
            bytes memory pAll = abi.encode(true, true);
            vm.startPrank(admin);
            bytes32 idAll = v.schedule(BridgeVault.Action.UnfreezeAll, pAll);
            for (uint256 i; i < 3; ++i) {
                v.schedule(BridgeVault.Action.Unfreeze, abi.encode(tokens[i], true, true));
            }
            vm.stopPrank();
            uint64 eta = v.scheduledEta(idAll);
            uint256 nowTs = main.clock();
            if (eta > nowTs) main.advanceClock(eta - nowTs);
            vm.startPrank(admin);
            v.execute(BridgeVault.Action.UnfreezeAll, pAll);
            for (uint256 i; i < 3; ++i) {
                v.execute(BridgeVault.Action.Unfreeze, abi.encode(tokens[i], true, true));
            }
            vm.stopPrank();
            ok[UNFREEZE]++;
            return;
        }
        flags >>= 1;
        // mostly clear both flags, so freezes don't starve the other actions
        bool dep = flags % 4 != 1;
        bool rel = flags % 4 != 2;
        if (target % 4 == 3) {
            _exec(BridgeVault.Action.UnfreezeAll, abi.encode(dep, rel));
            ok[UNFREEZE]++;
        } else {
            _exec(BridgeVault.Action.Unfreeze, abi.encode(tokens[target % 4], dep, rel));
            ok[UNFREEZE]++;
        }
    }

    function setReleaseDelay(uint256 tSeed, uint256 dSeed) external {
        vm.warp(main.clock());
        address t = tokens[tSeed % 3];
        uint64 d = uint64(_bound(dSeed, 30 minutes, 3 days)); // below 1 hour is refused
        vm.prank(admin);
        try v.setReleaseDelay(t, d) {
            ok[SET_DELAY]++;
        } catch (bytes memory err) {
            bytes4 sel = bytes4(err);
            if (sel == BridgeVault.TimelockRequired.selector) {
                _exec(BridgeVault.Action.SetReleaseDelay, abi.encode(t, d));
                ok[SET_DELAY]++;
            } else {
                rej[SET_DELAY]++;
                require(sel == BridgeVault.DelayTooShort.selector, "unexpected setReleaseDelay refusal");
            }
        }
    }

    /// A random menu per denomination: 0 to 4 distinct fees in [1, 2% of the denomination].
    function setRelayFeeMenu(uint256 tSeed, uint256 seed) external {
        vm.warp(main.clock());
        address t = tokens[tSeed % 3];
        uint256[] memory denoms = _terms(t);
        uint256[][] memory menus = new uint256[][](denoms.length);
        for (uint256 d; d < denoms.length; ++d) {
            uint256 s = _h(seed, d);
            uint256 n = s % 5;
            uint256[] memory m = new uint256[](n);
            uint256 maxFee = denoms[d] * 200 / 10_000;
            uint256 got;
            for (uint256 i; i < n; ++i) {
                uint256 f = _bound(_h(s, i), 1, maxFee);
                bool dup;
                for (uint256 k; k < got; ++k) dup = dup || m[k] == f;
                if (!dup) m[got++] = f;
            }
            assembly {
                mstore(m, got)
            }
            menus[d] = m;
        }
        vm.prank(admin);
        try v.setRelayFeeMenu(t, menus) {
            ok[SET_MENU]++;
        } catch {
            rej[SET_MENU]++;
        }
    }
}

contract InvariantTest is Test {
    BridgeVault v;
    MockVerifier mv;
    TestToken usdc;
    NoReturnToken usdt;
    VaultHandler h;
    AdminHandler ah;

    bytes32 constant G2 = keccak256("g2-image");
    bytes32 constant G3 = keccak256("g3-image");
    bytes32 constant LEZ_CHAIN = keccak256("lez-chain");
    bytes32 constant ZONE = keccak256("zone");
    bytes32 constant PROGRAM = keccak256("program");
    address constant ADMIN = address(0xA11CE);
    address constant FREEZER = address(0xF2EE);
    address constant FEE_TO = address(0xFEE);
    uint256 constant T0 = 1_000_000;

    bytes32 emptyRoot;
    bytes32 cfg0;

    function setUp() public {
        vm.warp(T0);
        mv = new MockVerifier();
        v = new BridgeVault(
            IRiscZeroVerifier(address(mv)), G2, G3, LEZ_CHAIN, ZONE, PROGRAM, keccak256("s0"), 1 hours, ADMIN, FREEZER, FEE_TO, 1 hours
        );
        emptyRoot = v.depositRoot();
        cfg0 = v.cfgId();
        usdc = new TestToken("USDC", 6);
        usdt = new NoReturnToken();

        vm.startPrank(ADMIN);
        v.registerToken(address(usdc), 6, _a3(100e6, 1000e6, 10_000e6), _a3(1e6, 10e6, 100e6), _a3(1e6, 5e6, 0), 6 hours);
        v.registerToken(address(usdt), 6, _a2(100e6, 1000e6), _a2(0.5e6, 0), _a2(0, 10e6), 6 hours);
        v.registerToken(address(0), 18, _a2(0.1 ether, 1 ether), _a2(0.001 ether, 0), _a2(0.002 ether, 0.01 ether), 6 hours);
        uint256[][] memory m3 = new uint256[][](3);
        m3[0] = _a2(1e6, 2e6);
        m3[1] = _a1(5e6);
        v.setRelayFeeMenu(address(usdc), m3);
        uint256[][] memory m2 = new uint256[][](2);
        m2[0] = _a1(2e6);
        v.setRelayFeeMenu(address(usdt), m2);
        m2 = new uint256[][](2);
        m2[1] = _a2(0.002 ether, 0.02 ether);
        v.setRelayFeeMenu(address(0), m2);
        v.setCapWeight(address(usdc), 1e18);
        v.setCapWeight(address(usdt), 1e18);
        v.setCapWeight(address(0), 3e9);
        vm.stopPrank();

        h = new VaultHandler(v, mv, G2, G3, FEE_TO, address(usdc), address(usdt), T0);
        usdc.mint(address(h), type(uint128).max);
        usdt.mint(address(h), type(uint128).max);
        vm.deal(address(h), 1e12 ether);
        h.bootstrap();
        _checkAll();

        ah = new AdminHandler(v, h, ADMIN, FREEZER);

        bytes4[] memory sel = new bytes4[](9);
        sel[0] = VaultHandler.depositUsdc.selector;
        sel[1] = VaultHandler.depositUsdt.selector;
        sel[2] = VaultHandler.depositEth.selector;
        sel[3] = VaultHandler.updateClaims.selector;
        sel[4] = VaultHandler.updateExits.selector;
        sel[5] = VaultHandler.release.selector;
        sel[6] = VaultHandler.releaseReplay.selector;
        sel[7] = VaultHandler.warp.selector;
        sel[8] = VaultHandler.withdrawFees.selector;
        targetContract(address(h));
        targetSelector(FuzzSelector({addr: address(h), selectors: sel}));
        bytes4[] memory asel = new bytes4[](6);
        asel[0] = AdminHandler.setCaps.selector;
        asel[1] = AdminHandler.setGlobalCaps.selector;
        asel[2] = AdminHandler.freeze.selector;
        asel[3] = AdminHandler.unfreeze.selector;
        asel[4] = AdminHandler.setReleaseDelay.selector;
        asel[5] = AdminHandler.setRelayFeeMenu.selector;
        targetContract(address(ah));
        targetSelector(FuzzSelector({addr: address(ah), selectors: asel}));
    }

    function _a1(uint256 a) internal pure returns (uint256[] memory r) {
        r = new uint256[](1);
        r[0] = a;
    }

    function _a2(uint256 a, uint256 b) internal pure returns (uint256[] memory r) {
        r = new uint256[](2);
        (r[0], r[1]) = (a, b);
    }

    function _a3(uint256 a, uint256 b, uint256 c) internal pure returns (uint256[] memory r) {
        r = new uint256[](3);
        (r[0], r[1], r[2]) = (a, b, c);
    }

    // ------------------------------------------------------------------ the invariants

    function invariant_all() public view {
        _checkAll();
    }

    function _checkAll() internal view {
        for (uint256 ti; ti < 3; ++ti) _checkToken(h.tokens(ti));
        _checkNullifiers();
        _checkRoots();
        _checkConfig();
        assertEq(h.payoutMismatch(), 0, "a release or fee withdrawal paid a wrong amount");
        assertEq(h.noAdvanceAccepted(), 0, "an update that did not advance the state was applied");
    }

    /// released value <= burnedValue <= claimedValue <= deposited value; per denomination
    /// claims <= deposits and releases <= burns; counters equal the handler's ghosts; the vault
    /// holds exactly deposited - released + accruedFees, and accruedFees equals the fees taken
    /// minus the fees withdrawn (so principal never left as fees).
    function _checkToken(address t) internal view {
        (,,,,, uint256[] memory denoms,,, uint256 accrued) = v.tokenConfig(t);
        uint256 deposited;
        uint256 released;
        for (uint256 d; d < denoms.length; ++d) {
            (uint64 dep, uint64 cl, uint64 bu, uint64 re) = v.counters(t, d);
            VaultHandler.Ghost memory g = h.ghost(t, d);
            assertEq(dep, g.deposits, "deposits counter");
            assertEq(cl, g.claims, "claims counter");
            assertEq(bu, g.burns, "burns counter");
            assertEq(re, g.releases, "releases counter");
            assertLe(cl, dep, "claims > deposits in a denomination");
            assertLe(re, bu, "releases > burns in a denomination");
            deposited += uint256(dep) * denoms[d];
            released += uint256(re) * denoms[d];
        }
        uint256 burned = v.burnedValue(t);
        uint256 claimed = v.claimedValue(t);
        assertEq(deposited, h.gDeposited(t), "deposited value");
        assertEq(released, h.gReleased(t), "released value");
        assertEq(claimed, h.gClaimed(t), "claimedValue");
        assertEq(burned, h.gBurned(t), "burnedValue");
        assertLe(released, burned, "released > burned");
        assertLe(burned, claimed, "burned > claimed");
        assertLe(claimed, deposited, "claimed > deposited");
        uint256 bal = t == address(0) ? address(v).balance : usdc.balanceOf(address(v));
        if (t == address(usdt)) bal = usdt.balanceOf(address(v));
        assertEq(bal, deposited - released + accrued, "vault balance != deposited - released + fees");
        assertEq(accrued, h.gFeesIn(t) - h.gFeesOut(t), "accruedFees != fees taken - fees withdrawn");
    }

    function _checkNullifiers() internal view {
        uint256 n = h.spentCount();
        for (uint256 i; i < n; ++i) {
            bytes32 nf = h.spent(i);
            assertEq(h.payCount(nf), 1, "a release nullifier paid twice");
            assertTrue(v.releaseNullifierSpent(nf), "spent nullifier not marked");
        }
        assertEq(h.ok(6), 0, "a replayed nullifier was accepted");
    }

    function _checkRoots() internal view {
        assertTrue(v.isDepositRoot(emptyRoot), "empty root forgotten");
        uint256 n = h.depositRootCount();
        for (uint256 i; i < n; ++i) assertTrue(v.isDepositRoot(h.depositRoots(i)), "deposit root forgotten");
        n = h.exitRootCount();
        for (uint256 i; i < n; ++i) {
            bytes32 r = h.exitRoots(i);
            assertEq(v.exitRootTime(r), h.exitSeenAt(r), "exit root time changed or disappeared");
            assertGt(v.exitRootTime(r), 0, "exit root disappeared");
        }
    }

    function _checkConfig() internal view {
        assertEq(v.cfgId(), cfg0, "cfgId");
        assertEq(v.cfgId(), v.computeCfgId(block.chainid, address(v), LEZ_CHAIN, ZONE, PROGRAM), "cfgId derivation");
        assertEq(address(v.verifier()), address(mv), "verifier");
        assertEq(v.updateImageId(), G2, "updateImageId");
        assertEq(v.releaseImageId(), G3, "releaseImageId");
        assertEq(v.lezChainId(), LEZ_CHAIN, "lezChainId");
        assertEq(v.zoneId(), ZONE, "zoneId");
        assertEq(v.programId(), PROGRAM, "programId");
        assertEq(v.minReleaseDelay(), 1 hours, "minReleaseDelay");
        assertEq(v.adminDelay(), 1 hours, "adminDelay");
        assertEq(v.admin(), ADMIN, "admin");
    }

    // ------------------------------------------------------------------ seeded driver

    /// The invariant campaign starts every run from a fresh handler, so its counters are per
    /// run. This deterministic driver runs one long sequence (1,500 handler calls, weighted
    /// toward deposits, updates and releases), checks every invariant every 10 calls and at the
    /// end, and asserts that every action both succeeded and (where it can) was refused, so the
    /// invariants are not vacuous. The table it logs (-vv) is the call-coverage summary.
    function test_SeededDriverCoverage() public {
        uint256 seed = uint256(keccak256("rfp022-invariant-driver"));
        for (uint256 i; i < 1500; ++i) {
            seed = uint256(keccak256(abi.encode(seed, i)));
            _step(seed);
            if (i % 10 == 9) _checkAll();
        }
        _checkAll();

        console.log("action                       ok   rejected");
        for (uint256 a; a < 15; ++a) {
            console.log(string.concat(_pad(actionName(a), 28), _pad(vm.toString(_ok(a)), 5), vm.toString(_rej(a))));
        }
        console.log(
            string.concat(
                "releases ok by token (usdc/usdt/eth): ",
                vm.toString(h.releasesOkByToken(0)), " / ", vm.toString(h.releasesOkByToken(1)), " / ",
                vm.toString(h.releasesOkByToken(2))
            )
        );
        console.log(
            string.concat(
                "deposits ok by denomination index (0/1/2): ",
                vm.toString(h.depositsOkByDenom(0)), " / ", vm.toString(h.depositsOkByDenom(1)), " / ",
                vm.toString(h.depositsOkByDenom(2))
            )
        );
        bytes4[] memory known = _knownErrors();
        for (uint256 a; a < 15; ++a) {
            if (h.rej(a) == 0) continue;
            string memory line = string.concat("  refusals of ", actionName(a), ":");
            uint256 counted;
            for (uint256 i; i < known.length; ++i) {
                uint256 c = h.reasonCount(a, known[i]);
                if (c == 0) continue;
                counted += c;
                line = string.concat(line, " ", _errName(known[i]), " x", vm.toString(c));
            }
            assertEq(counted, h.rej(a), "a refusal with an unlisted error");
            console.log(line);
        }
        console.log(string.concat("spent nullifiers: ", vm.toString(h.spentCount()), ", exit roots: ", vm.toString(h.exitRootCount())));

        for (uint256 a; a < 15; ++a) {
            if (a == 6) continue;
            assertGt(_ok(a), 0, string.concat("never succeeded: ", actionName(a)));
        }
        // the actions that can be refused were refused at least once
        uint256[7] memory canFail = [uint256(0), 1, 2, 3, 4, 5, 6];
        for (uint256 k; k < canFail.length; ++k) {
            assertGt(_rej(canFail[k]), 0, string.concat("never refused: ", actionName(canFail[k])));
        }
        assertGt(_rej(12), 0, "setReleaseDelay below the minimum never tried");
        // the vault and address(0) were tried as recipients, and the vault as a paid relayer
        assertGt(h.reasonCount(RELEASE, BridgeVault.BadRecipient.selector), 0, "BadRecipient never hit");
        assertGt(h.reasonCount(RELEASE, BridgeVault.BadRelayer.selector), 0, "BadRelayer never hit");
        assertGt(
            h.reasonCount(UPD_CLAIMS, BridgeVault.NoStateAdvance.selector)
                + h.reasonCount(UPD_EXITS, BridgeVault.NoStateAdvance.selector),
            0,
            "NoStateAdvance never hit"
        );
        for (uint256 ti; ti < 3; ++ti) assertGt(h.releasesOkByToken(ti), 0, "a token never released");
        for (uint256 d; d < 3; ++d) assertGt(h.depositsOkByDenom(d), 0, "a denomination never deposited");
        // every token ends solvent with value actually moved
        for (uint256 ti; ti < 3; ++ti) assertGt(h.gReleased(h.tokens(ti)), 0);
    }

    function _knownErrors() internal pure returns (bytes4[] memory k) {
        k = new bytes4[](20);
        k[0] = BridgeVault.IsFrozen.selector;
        k[1] = BridgeVault.CapExceeded.selector;
        k[2] = BridgeVault.CommitmentUsed.selector;
        k[3] = BridgeVault.StaleState.selector;
        k[4] = BridgeVault.UnknownDepositRoot.selector;
        k[5] = BridgeVault.ClaimsExceedDeposits.selector;
        k[6] = BridgeVault.BurnsExceedClaims.selector;
        k[7] = BridgeVault.UnknownExitRoot.selector;
        k[8] = BridgeVault.ReleaseTooEarly.selector;
        k[9] = BridgeVault.NullifierSpent.selector;
        k[10] = BridgeVault.ReleasesExceedBurns.selector;
        k[11] = BridgeVault.RelayFeeNotOnMenu.selector;
        k[12] = BridgeVault.ZeroRelayer.selector;
        k[13] = BridgeVault.DelayTooShort.selector;
        k[14] = BridgeVault.BadDenomination.selector;
        k[15] = BridgeVault.WrongAmount.selector;
        k[16] = MockVerifier.BadSeal.selector;
        k[17] = BridgeVault.BadRecipient.selector;
        k[18] = BridgeVault.BadRelayer.selector;
        k[19] = BridgeVault.NoStateAdvance.selector;
    }

    function _errName(bytes4 e) internal view returns (string memory) {
        if (e == BridgeVault.IsFrozen.selector) return "IsFrozen";
        if (e == BridgeVault.CapExceeded.selector) return "CapExceeded";
        if (e == BridgeVault.CommitmentUsed.selector) return "CommitmentUsed";
        if (e == BridgeVault.StaleState.selector) return "StaleState";
        if (e == BridgeVault.UnknownDepositRoot.selector) return "UnknownDepositRoot";
        if (e == BridgeVault.ClaimsExceedDeposits.selector) return "ClaimsExceedDeposits";
        if (e == BridgeVault.BurnsExceedClaims.selector) return "BurnsExceedClaims";
        if (e == BridgeVault.UnknownExitRoot.selector) return "UnknownExitRoot";
        if (e == BridgeVault.ReleaseTooEarly.selector) return "ReleaseTooEarly";
        if (e == BridgeVault.NullifierSpent.selector) return "NullifierSpent";
        if (e == BridgeVault.ReleasesExceedBurns.selector) return "ReleasesExceedBurns";
        if (e == BridgeVault.RelayFeeNotOnMenu.selector) return "RelayFeeNotOnMenu";
        if (e == BridgeVault.ZeroRelayer.selector) return "ZeroRelayer";
        if (e == BridgeVault.DelayTooShort.selector) return "DelayTooShort";
        if (e == BridgeVault.BadDenomination.selector) return "BadDenomination";
        if (e == BridgeVault.WrongAmount.selector) return "WrongAmount";
        if (e == MockVerifier.BadSeal.selector) return "BadSeal";
        if (e == BridgeVault.BadRecipient.selector) return "BadRecipient";
        if (e == BridgeVault.BadRelayer.selector) return "BadRelayer";
        if (e == BridgeVault.NoStateAdvance.selector) return "NoStateAdvance";
        return vm.toString(abi.encodePacked(e));
    }

    function _ok(uint256 a) internal view returns (uint256) {
        return h.ok(a) + ah.ok(a);
    }

    function _rej(uint256 a) internal view returns (uint256) {
        return h.rej(a) + ah.rej(a);
    }

    function _pad(string memory s, uint256 n) internal pure returns (string memory) {
        bytes memory b = bytes(s);
        if (b.length >= n) return string.concat(s, " ");
        bytes memory p = new bytes(n - b.length);
        for (uint256 i; i < p.length; ++i) p[i] = " ";
        return string.concat(s, string(p));
    }

    function _step(uint256 s) internal {
        uint256 k = s % 32;
        uint256 a = s >> 5;
        uint256 b = uint256(keccak256(abi.encode(s, 1)));
        uint256 c = uint256(keccak256(abi.encode(s, 2)));
        uint256 d = uint256(keccak256(abi.encode(s, 3)));
        uint256 e = uint256(keccak256(abi.encode(s, 4)));
        if (k < 4) h.depositUsdc(a, b);
        else if (k < 6) h.depositUsdt(a, b);
        else if (k < 8) h.depositEth(a, b);
        else if (k < 11) h.updateClaims(a, b, c % 8, d);
        else if (k < 14) h.updateExits(a, b);
        else if (k < 19) h.release(a, b, c, d, e);
        else if (k < 20) h.releaseReplay(a, b, c, d);
        else if (k < 23) h.warp(a % 2 == 0 ? a % 2 hours : a % 2 days);
        else if (k < 24) ah.setCaps(a, b, c, d);
        else if (k < 25) ah.setGlobalCaps(a, b, c);
        else if (k < 27) ah.freeze(a, b, c % 2 == 0);
        else if (k < 29) ah.unfreeze(a, b);
        else if (k < 30) ah.setReleaseDelay(a, b);
        else if (k < 31) ah.setRelayFeeMenu(a, b);
        else h.withdrawFees(a);
    }
}
