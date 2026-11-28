// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {IERC20Metadata} from "@openzeppelin/contracts/token/ERC20/extensions/IERC20Metadata.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";

import {IRiscZeroVerifier} from "./IRiscZeroVerifier.sol";
import {Sha256IncrementalTree} from "./Sha256IncrementalTree.sol";

/// @title BridgeVault: Ethereum side of the RFP-022 private ERC-20 / ETH bridge to LEZ.
///
/// Holds locked ERC-20s and ETH in fixed denominations. Every wrapped token on LEZ is
/// backed one-for-one by a deposit here. The vault itself enforces solvency:
///
///   claims[t][d]    <= deposits[t][d]   (a LEZ claim counts only against a real deposit root)
///   burnedValue[t]  <= claimedValue[t]  (an update can't create exits worth more than was minted)
///   releases[t][d]  <= burns[t][d]      (a release needs a proven exit and a fresh nullifier)
///
/// where claimedValue[t] = sum over d of claims[t][d] * denom[d] and burnedValue[t] = sum of
/// the denominations of all exits of t. The burn bound is by value per token, not per
/// denomination, because wrapped tokens are fungible on LEZ: 10 x 100 bought on a DEX can
/// exit against a single 1 x 1000 claim, and the other way round. Per token this gives
///
///   released value <= burned value <= claimed value <= deposited value
///
/// so the vault can never pay out more principal than was deposited, whatever the LEZ
/// sequencer committee does. Verification is fixed at deployment (verifier address and
/// image IDs are immutable); a new proof system means a new vault version.
///
/// Deposits commit to `C = SHA256("RFP022/dep" | k_i | dest | token | denom | cfgId)` and
/// carry no destination. The vault stores the leaf `SHA256(tag | cfgId | token | denom | C)`
/// so a claim can't mint a larger denomination than was paid. A commitment can be used only
/// once: the LEZ claim nullifier depends on the deposit secret alone, so a second deposit
/// under the same commitment could never be claimed.
///
/// `cfgId = SHA256(CFG_TAG | chainid | address(this) | lezChainId | zoneId | programId)`
/// (32 + 32 + 20 + 32 + 32 + 32 = 180 bytes) is derived at deployment, never supplied.
contract BridgeVault is ReentrancyGuard {
    using SafeERC20 for IERC20;
    using Sha256IncrementalTree for Sha256IncrementalTree.Tree;

    // ------------------------------------------------------------------ constants

    address public constant ETH = address(0);
    bytes32 public constant DEPOSIT_LEAF_TAG = keccak256("RFP022/deposit-leaf/v1");
    bytes32 public constant EXIT_LEAF_TAG = keccak256("RFP022/exit-leaf/v1");
    /// Fees can never exceed 2% of a denomination, whatever the admin sets.
    uint256 public constant MAX_FEE_BPS = 200;
    uint256 public constant MAX_DENOMS = 8;
    /// Non-zero relay fees allowed per denomination (fee 0 is always allowed on top).
    uint256 public constant MAX_RELAY_FEES = 4;
    bytes32 public constant CFG_TAG = keccak256("RFP022/cfg/v1");
    /// Token address used in freeze events for the global flags.
    address public constant GLOBAL = address(type(uint160).max);
    uint256 internal constant MAX112 = type(uint112).max;
    /// Bounds on the immutable admin timelock delay (B1.5).
    uint64 public constant MIN_ADMIN_DELAY = 1 hours;
    uint64 public constant MAX_ADMIN_DELAY = 30 days;
    /// Window after `eta` during which a scheduled admin action may be executed before it expires.
    uint64 public constant ADMIN_GRACE_PERIOD = 7 days;

    // ------------------------------------------------------------------ immutables

    IRiscZeroVerifier public immutable verifier;
    /// G2: outbound state proof (Logos Blockchain + LEZ chain -> new exits, claim counts).
    bytes32 public immutable updateImageId;
    /// G3: release proof (exit membership + nullifier, made on the user's device).
    bytes32 public immutable releaseImageId;
    /// LEZ chain, zone and bridge program this vault serves.
    bytes32 public immutable lezChainId;
    bytes32 public immutable zoneId;
    bytes32 public immutable programId;
    /// Binds this vault, chain, LEZ zone and program (F#15). Derived in the constructor
    /// from block.chainid, address(this) and the three LEZ ids; see `computeCfgId`.
    bytes32 public immutable cfgId;
    /// Lower bound on any token's release delay; the admin can't go below it.
    uint64 public immutable minReleaseDelay;
    /// Minimum delay between scheduling and executing any safety-weakening admin action (B1.5).
    /// Enforced in [MIN_ADMIN_DELAY, MAX_ADMIN_DELAY] at deployment.
    uint64 public immutable adminDelay;

    // ------------------------------------------------------------------ roles

    address public admin;
    address public pendingAdmin;
    address public freezer;
    address public feeRecipient;
    /// Global freezes (F#14): deposits and releases are paused independently.
    bool public globalDepositsFrozen;
    bool public globalReleasesFrozen;
    /// Global caps across all tokens, in normalised units (see `capWeight`). 0 disables.
    uint64 public globalCapWindow;
    uint256 public globalDepositCap;
    uint256 public globalReleaseCap;

    // ------------------------------------------------------------------ caps

    /// Usage over fixed buckets of `window` seconds aligned to multiples of `window`; `index`
    /// is block.timestamp / window. Used now = prev * (window - elapsed in current bucket) /
    /// window + cur (the two-bucket sliding estimate of a rolling window). One slot. Amounts
    /// saturate at 2^112 - 1, which only matters for a cap at least that large (no limit).
    struct Bucket {
        uint32 index;
        uint112 prev;
        uint112 cur;
    }

    Bucket internal _globalDep;
    Bucket internal _globalRel;
    /// Global cap units per smallest token unit, scaled by 1e18. 0: the token doesn't count
    /// toward the global caps.
    mapping(address => uint256) public capWeight;

    // ------------------------------------------------------------------ tokens

    struct TokenConfig {
        bool registered; // false: no new deposits (releases continue)
        bool depositsFrozen;
        bool releasesFrozen;
        uint8 decimals;
        uint64 releaseDelay;
        uint64 capWindow; // seconds; 0 disables caps
        uint64 delayChangedAt; // time of the last effective setReleaseDelay (0: never changed)
        // Unused since the per-token delay history (`delaySteps`) replaced them. Kept so the
        // members after them (depositCap...) keep their offsets inside the struct.
        uint64 unusedPrevReleaseDelay;
        uint64 unusedReleasableUpTo;
        uint256 depositCap; // per window, in token units
        uint256 releaseCap; // per window, in token units
        uint256 accruedFees;
        Bucket dep;
        Bucket rel;
        uint256[] denoms;
        uint256[] depositFees;
        uint256[] withdrawFees;
    }

    struct Counters {
        uint64 deposits;
        uint64 claims;
        uint64 burns;
        uint64 releases;
    }

    mapping(address => TokenConfig) internal _tokens;
    mapping(address => mapping(uint256 => Counters)) public counters;
    /// Per token, in token units: sum of claims * denom, and sum of burned exit denoms.
    mapping(address => uint256) public claimedValue;
    mapping(address => uint256) public burnedValue;
    address[] public tokenList;

    // ------------------------------------------------------------------ trees and state

    Sha256IncrementalTree.Tree internal _depositTree;
    Sha256IncrementalTree.Tree internal _exitTree;
    /// Every deposit root ever reached, kept forever so claims never expire (R#4).
    mapping(bytes32 => bool) public isDepositRoot;
    /// Exit root -> timestamp it became known. Kept forever.
    mapping(bytes32 => uint64) public exitRootTime;
    mapping(bytes32 => bool) public releaseNullifierSpent;
    /// Deposit commitments already used, across all tokens and denominations.
    mapping(bytes32 => bool) public commitmentUsed;
    /// Commitment to G2's carried state (L1 tip, LEZ tip, claim-nullifier accumulator).
    bytes32 public g2State;

    // ------------------------------------------------------------------ relay-fee menu (F#13)
    // Declared after every older variable so existing slots (isDepositRoot = 83) don't move.

    /// Allowed non-zero relay fees per token and denomination index, at most
    /// MAX_RELAY_FEES each. Fee 0 (self-submission) is always allowed and never listed.
    mapping(address => mapping(uint256 => uint256[])) internal _relayFeeMenu;
    /// token => denomIndex => fee => on the menu. The O(1) lookup `release` uses.
    mapping(address => mapping(uint256 => mapping(uint256 => bool))) public relayFeeAllowed;

    // ------------------------------------------------------------------ release-delay history (F#12)
    // Declared after every older variable so existing slots don't move.

    /// One past release delay of a token: `delay` was in force until the change at `until`.
    struct DelayStep {
        uint64 delay;
        uint64 until;
    }

    /// Per token, the past delays that can still decide some root's wait: indices
    /// [0, delayStepCount) with `delay` strictly increasing, `until` non-decreasing, and every
    /// `delay` below the token's current `releaseDelay` (see `setReleaseDelay`). Entries at or
    /// above the count are stale and never read.
    mapping(address => mapping(uint256 => DelayStep)) public delaySteps;
    mapping(address => uint256) internal _delayStepCount;

    // ------------------------------------------------------------------ admin timelock (B1.5)
    // Declared after every older variable so existing slots (isDepositRoot = 83, exitRootTime = 84) don't move.

    /// Admin actions that can weaken safety and therefore require the timelock.
    /// Verification (verifier, image IDs, LEZ IDs, cfgId, minReleaseDelay, adminDelay) is
    /// immutable with no setter at all (S#8).
    enum Action {
        Unfreeze,        // abi.encode(address token, bool deposits, bool releases)
        UnfreezeAll,     // abi.encode(bool deposits, bool releases)
        SetFreezer,      // abi.encode(address next)
        ProposeAdmin,    // abi.encode(address next)
        SetFeeRecipient, // abi.encode(address next)
        SetCaps,         // abi.encode(address token, uint64 window, uint256 depositCap, uint256 releaseCap)
        SetGlobalCaps,   // abi.encode(uint64 window, uint256 depositCap, uint256 releaseCap)
        SetCapWeight,    // abi.encode(address token, uint256 weight)
        SetReleaseDelay, // abi.encode(address token, uint64 delay)
        SetFees          // abi.encode(address token, uint256[] depositFees, uint256[] withdrawFees)
    }

    /// Action id -> earliest execution timestamp (`eta`), or 0 if not scheduled (or executed/cancelled).
    mapping(bytes32 => uint64) public scheduledEta;
    /// Time of the latest `freeze`/`freezeAll`. An `Unfreeze`/`UnfreezeAll` scheduled before it
    /// can't be executed, so every unfreeze waits `adminDelay` after the latest freeze (no
    /// unfreeze queued ahead of an incident can lift the freeze at once).
    uint64 public lastFreezeAt;

    // ------------------------------------------------------------------ journal types

    struct ClaimDelta {
        address token;
        uint32 denomIndex;
        uint64 count;
    }

    struct Exit {
        address token;
        uint32 denomIndex;
        bytes32 commitment; // E = SHA256("RFP022/exit" | k_j | ethDest | token | denom | maxRelayFee | cfgId)
    }

    /// Public output of G2, ABI-encoded and committed as the guest journal.
    struct UpdateJournal {
        bytes32 cfgId;
        bytes32 prevState;
        bytes32 newState;
        bytes32[] depositRootsUsed;
        ClaimDelta[] claims;
        Exit[] exits;
    }

    // ------------------------------------------------------------------ events

    event Deposit(address indexed token, uint32 denomIndex, bytes32 commitment, uint64 leafIndex, bytes32 root);
    event StateUpdated(bytes32 prevState, bytes32 newState, bytes32 exitRoot, uint64 firstExitIndex, bytes32[] exitLeaves);
    event Released(bytes32 indexed nullifier, address indexed token, uint32 denomIndex, address recipient, address relayer, uint256 relayFee);
    event TokenRegistered(address indexed token, uint8 decimals, uint256[] denoms);
    event TokenDeregistered(address indexed token);
    event DenominationAdded(address indexed token, uint32 denomIndex, uint256 denom);
    event FeesSet(address indexed token, uint256[] depositFees, uint256[] withdrawFees);
    event RelayFeeMenuSet(address indexed token, uint256[][] menus);
    event CapsSet(address indexed token, uint64 window, uint256 depositCap, uint256 releaseCap);
    event GlobalCapsSet(uint64 window, uint256 depositCap, uint256 releaseCap);
    event CapWeightSet(address indexed token, uint256 weight);
    event ReleaseDelaySet(address indexed token, uint64 delay);
    /// The flags that were set (Frozen) or cleared (Unfrozen) by one call. token == GLOBAL
    /// for the global flags.
    event Frozen(address indexed token, bool deposits, bool releases);
    event Unfrozen(address indexed token, bool deposits, bool releases);
    event FeesWithdrawn(address indexed token, address to, uint256 amount);
    event AdminProposed(address indexed next);
    event AdminAccepted(address indexed admin);
    event FreezerSet(address indexed freezer);
    event FeeRecipientSet(address indexed feeRecipient);
    event ActionScheduled(bytes32 indexed id, Action indexed action, bytes params, uint64 eta);
    event ActionExecuted(bytes32 indexed id, Action indexed action, bytes params, uint64 eta);
    event ActionCancelled(bytes32 indexed id, address indexed cancelledBy);

    // ------------------------------------------------------------------ errors

    error NotAdmin();
    error NotFreezer();
    error NotPendingAdmin();
    error IsFrozen();
    error NotRegistered();
    error AlreadyRegistered();
    error BadDenomination();
    error BadDecimals();
    error FeeTooHigh();
    error WrongAmount();
    error UnexpectedValue();
    error CapExceeded();
    error DelayTooShort();
    error ZeroAddress();
    error WrongConfig();
    error StaleState();
    /// update: the journal's newState equals its prevState. Such a journal would stay valid
    /// after it is applied, so anyone could replay it and re-add its claims and exits.
    error NoStateAdvance();
    error UnknownDepositRoot();
    error ClaimsExceedDeposits();
    error BurnsExceedClaims();
    error UnknownExitRoot();
    error ReleaseTooEarly();
    error NullifierSpent();
    error ReleasesExceedBurns();
    error RelayFeeTooHigh();
    error RelayFeeNotOnMenu();
    error ZeroRelayer();
    error BadRelayFeeMenu();
    error EthTransferFailed();
    error CommitmentUsed();
    /// release: recipient is the vault itself or address(0).
    error BadRecipient();
    /// release: a non-zero relay fee named the vault itself as relayer.
    error BadRelayer();
    /// constructor: adminDelay_ outside [MIN_ADMIN_DELAY, MAX_ADMIN_DELAY].
    error AdminDelayOutOfBounds();
    /// direct admin setter: change weakens safety and must go through schedule/execute.
    error TimelockRequired();
    /// schedule: an active (unexpired) action with this id is already queued.
    error AlreadyScheduled();
    /// execute / cancel: no scheduled action with this id.
    error NotScheduled();
    /// execute: block.timestamp < eta.
    error ActionTooEarly();
    /// execute: block.timestamp >= eta + ADMIN_GRACE_PERIOD.
    error ActionExpired();
    /// execute: an Unfreeze/UnfreezeAll scheduled before the latest freeze.
    error ScheduledBeforeFreeze();

    // ------------------------------------------------------------------ constructor

    constructor(
        IRiscZeroVerifier verifier_,
        bytes32 updateImageId_,
        bytes32 releaseImageId_,
        bytes32 lezChainId_,
        bytes32 zoneId_,
        bytes32 programId_,
        bytes32 initialG2State,
        uint64 minReleaseDelay_,
        address admin_,
        address freezer_,
        address feeRecipient_,
        uint64 adminDelay_
    ) {
        if (address(verifier_) == address(0) || admin_ == address(0) || feeRecipient_ == address(0)) {
            revert ZeroAddress();
        }
        if (adminDelay_ < MIN_ADMIN_DELAY || adminDelay_ > MAX_ADMIN_DELAY) {
            revert AdminDelayOutOfBounds();
        }
        verifier = verifier_;
        updateImageId = updateImageId_;
        releaseImageId = releaseImageId_;
        lezChainId = lezChainId_;
        zoneId = zoneId_;
        programId = programId_;
        cfgId = computeCfgId(block.chainid, address(this), lezChainId_, zoneId_, programId_);
        minReleaseDelay = minReleaseDelay_;
        adminDelay = adminDelay_;
        g2State = initialG2State;
        admin = admin_;
        freezer = freezer_;
        feeRecipient = feeRecipient_;
        bytes32 empty = Sha256IncrementalTree.emptyRoot();
        _depositTree.root = empty;
        _exitTree.root = empty;
        isDepositRoot[empty] = true;
    }

    /// The config id a vault at `vault` on `chainId` gets for these LEZ ids.
    function computeCfgId(uint256 chainId, address vault, bytes32 lezChainId_, bytes32 zoneId_, bytes32 programId_)
        public
        pure
        returns (bytes32)
    {
        return sha256(abi.encodePacked(CFG_TAG, chainId, vault, lezChainId_, zoneId_, programId_));
    }

    // ------------------------------------------------------------------ modifiers

    modifier onlyAdmin() {
        if (msg.sender != admin) revert NotAdmin();
        _;
    }

    modifier onlyFreezer() {
        if (msg.sender != freezer && msg.sender != admin) revert NotFreezer();
        _;
    }

    // ------------------------------------------------------------------ deposits

    /// Lock `denoms[denomIndex]` of an ERC-20 (plus the deposit fee) behind commitment `C`.
    /// Works with tokens that return no bool (USDT). Refuses fee-on-transfer and other
    /// tokens whose received amount differs from the amount sent.
    function deposit(address token, uint32 denomIndex, bytes32 commitment)
        external
        nonReentrant
        returns (uint64 leafIndex)
    {
        if (token == ETH) revert BadDenomination();
        (uint256 denom, uint256 fee) = _depositTerms(token, denomIndex);
        uint256 total = denom + fee;
        IERC20 t = IERC20(token);
        uint256 before = t.balanceOf(address(this));
        t.safeTransferFrom(msg.sender, address(this), total);
        if (t.balanceOf(address(this)) - before != total) revert WrongAmount();
        leafIndex = _recordDeposit(token, denomIndex, denom, fee, commitment);
    }

    /// Lock native ETH. `msg.value` must equal the denomination plus the deposit fee.
    function depositETH(uint32 denomIndex, bytes32 commitment)
        external
        payable
        nonReentrant
        returns (uint64 leafIndex)
    {
        (uint256 denom, uint256 fee) = _depositTerms(ETH, denomIndex);
        if (msg.value != denom + fee) revert WrongAmount();
        leafIndex = _recordDeposit(ETH, denomIndex, denom, fee, commitment);
    }

    function _depositTerms(address token, uint32 denomIndex) internal view returns (uint256 denom, uint256 fee) {
        TokenConfig storage c = _tokens[token];
        if (globalDepositsFrozen || c.depositsFrozen) revert IsFrozen();
        if (!c.registered) revert NotRegistered();
        if (denomIndex >= c.denoms.length) revert BadDenomination();
        denom = c.denoms[denomIndex];
        fee = c.depositFees[denomIndex];
    }

    function _recordDeposit(address token, uint32 denomIndex, uint256 denom, uint256 fee, bytes32 commitment)
        internal
        returns (uint64 leafIndex)
    {
        if (commitmentUsed[commitment]) revert CommitmentUsed();
        commitmentUsed[commitment] = true;
        TokenConfig storage c = _tokens[token];
        _charge(c, token, denom, true);
        c.accruedFees += fee;
        counters[token][denomIndex].deposits += 1;
        bytes32 leaf = depositLeaf(token, denom, commitment);
        bytes32 root;
        (leafIndex, root) = _depositTree.insert(leaf);
        isDepositRoot[root] = true;
        emit Deposit(token, denomIndex, commitment, leafIndex, root);
    }

    function depositLeaf(address token, uint256 denom, bytes32 commitment) public view returns (bytes32) {
        return sha256(abi.encodePacked(DEPOSIT_LEAF_TAG, cfgId, token, denom, commitment));
    }

    function exitLeaf(address token, uint256 denom, bytes32 commitment) public view returns (bytes32) {
        return sha256(abi.encodePacked(EXIT_LEAF_TAG, cfgId, token, denom, commitment));
    }

    // ------------------------------------------------------------------ outbound state (G2)

    /// Apply a G2 proof: new exits from finalised LEZ burns, and claim counts for the
    /// conservation bound (claims <= deposits per denomination, burned value <= claimed
    /// value per token). Permissionless: anyone may submit; a stale update is refused, and so
    /// is one that does not move the state (newState == prevState), which could otherwise be
    /// replayed (a real G2 batch is never empty and always advances the LEZ tip).
    /// TODO(M3): contest window over competing G2 chains (DESIGN §4.5).
    /// Only the global release freeze blocks it: deposit freezes and per-token freezes don't.
    function update(bytes calldata journal, bytes calldata seal) external nonReentrant {
        if (globalReleasesFrozen) revert IsFrozen();
        verifier.verify(seal, updateImageId, sha256(journal));
        UpdateJournal memory j = abi.decode(journal, (UpdateJournal));
        if (j.cfgId != cfgId) revert WrongConfig();
        if (j.prevState != g2State) revert StaleState();
        if (j.newState == j.prevState) revert NoStateAdvance();

        for (uint256 i; i < j.depositRootsUsed.length; ++i) {
            if (!isDepositRoot[j.depositRootsUsed[i]]) revert UnknownDepositRoot();
        }
        for (uint256 i; i < j.claims.length; ++i) {
            ClaimDelta memory d = j.claims[i];
            uint256[] storage ds = _tokens[d.token].denoms;
            if (d.denomIndex >= ds.length) revert BadDenomination();
            Counters storage k = counters[d.token][d.denomIndex];
            k.claims += d.count;
            if (k.claims > k.deposits) revert ClaimsExceedDeposits();
            claimedValue[d.token] += uint256(d.count) * ds[d.denomIndex];
        }

        uint64 first = _exitTree.nextIndex;
        bytes32[] memory leaves = new bytes32[](j.exits.length);
        bytes32 root = _exitTree.root;
        for (uint256 i; i < j.exits.length; ++i) {
            Exit memory e = j.exits[i];
            TokenConfig storage c = _tokens[e.token];
            if (e.denomIndex >= c.denoms.length) revert BadDenomination();
            uint256 denom = c.denoms[e.denomIndex];
            counters[e.token][e.denomIndex].burns += 1;
            uint256 burned = burnedValue[e.token] + denom;
            if (burned > claimedValue[e.token]) revert BurnsExceedClaims();
            burnedValue[e.token] = burned;
            leaves[i] = exitLeaf(e.token, denom, e.commitment);
            (, root) = _exitTree.insert(leaves[i]);
        }
        if (j.exits.length != 0) exitRootTime[root] = uint64(block.timestamp);
        g2State = j.newState;
        emit StateUpdated(j.prevState, j.newState, root, first, leaves);
    }

    // ------------------------------------------------------------------ release (G3)

    /// Pay out one exit. The G3 proof shows, without revealing which exit, that the caller
    /// knows `k_j` for an exit leaf under `exitRoot` whose commitment binds `recipient`,
    /// `token`, the denomination and a max relay fee >= `relayFee`, and that `nullifier`
    /// is derived from `k_j`. Anyone may submit; funds can only go where the proof says.
    /// A non-zero `relayFee` must be on the token's relay-fee menu for that denomination
    /// and needs a non-zero `relayer` (see `setRelayFeeMenu`).
    ///
    /// `recipient` may not be the vault or address(0) (BadRecipient), and a fee-earning
    /// `relayer` may not be the vault (BadRelayer): a payout to the vault would count as a
    /// release while the funds stay inside it outside `accruedFees`, unwithdrawable forever
    /// (ERC-20), or always fail (ETH: the vault has no receive function); address(0) would
    /// burn ETH or revert inside the token. The recipient is fixed in the exit commitment at
    /// burn time, so an exit burned to such a recipient can never be released and nobody can
    /// redirect it: that is the user's own error, and clients must refuse to build such a burn.
    function release(
        bytes calldata seal,
        bytes32 root,
        bytes32 nullifier,
        address token,
        uint32 denomIndex,
        address recipient,
        address relayer,
        uint256 relayFee
    ) external nonReentrant {
        if (recipient == address(this) || recipient == address(0)) revert BadRecipient();
        TokenConfig storage c = _tokens[token];
        if (globalReleasesFrozen || c.releasesFrozen) revert IsFrozen();
        if (denomIndex >= c.denoms.length) revert BadDenomination();
        uint256 seen = exitRootTime[root];
        if (seen == 0) revert UnknownExitRoot();
        // F#12: the root waits the lowest delay in force for this token at any time since it
        // was recorded (see `setReleaseDelay`). That is never above the current delay, so the
        // history is read only when the current delay alone would refuse a root recorded at
        // or before the last change.
        uint256 delay = c.releaseDelay;
        if (block.timestamp < seen + delay && seen <= c.delayChangedAt) delay = _delayFor(token, delay, seen);
        if (block.timestamp < seen + delay) revert ReleaseTooEarly();
        if (releaseNullifierSpent[nullifier]) revert NullifierSpent();

        uint256 denom = c.denoms[denomIndex];
        uint256 wfee = c.withdrawFees[denomIndex];
        // defensive: unreachable for menu values (relay and withdraw fees are each <= 2%)
        if (relayFee > denom - wfee) revert RelayFeeTooHigh();
        if (relayFee != 0) {
            // a fee paid to address(0) would burn it (ETH) or revert inside the token (ERC-20)
            if (relayer == address(0)) revert ZeroRelayer();
            // a fee paid to the vault would be stranded outside accruedFees (ERC-20) or fail (ETH)
            if (relayer == address(this)) revert BadRelayer();
            // F#13: only menu values, so the fee doesn't single out a release
            if (!relayFeeAllowed[token][denomIndex][relayFee]) revert RelayFeeNotOnMenu();
        }

        bytes32 digest = sha256(
            abi.encode(cfgId, root, nullifier, token, denom, recipient, relayer, relayFee)
        );
        verifier.verify(seal, releaseImageId, digest);

        releaseNullifierSpent[nullifier] = true;
        Counters storage k = counters[token][denomIndex];
        k.releases += 1;
        if (k.releases > k.burns) revert ReleasesExceedBurns();

        _charge(c, token, denom, false);

        c.accruedFees += wfee;
        _pay(token, recipient, denom - wfee - relayFee);
        if (relayFee != 0) _pay(token, relayer, relayFee);
        emit Released(nullifier, token, denomIndex, recipient, relayer, relayFee);
    }

    /// The delay a root recorded at `seen` waits for `token`: the first history step still in
    /// force at `seen` (until >= seen), else the current delay `current`. Steps have increasing
    /// delays and non-decreasing `until`, so the first such step is the minimum over every
    /// delay in force from `seen` on. Binary search: ceil(log2(n + 1)) step reads.
    function _delayFor(address token, uint256 current, uint256 seen) internal view returns (uint256) {
        mapping(uint256 => DelayStep) storage s = delaySteps[token];
        uint256 n = _delayStepCount[token];
        uint256 lo;
        uint256 hi = n;
        while (lo < hi) {
            uint256 mid = (lo + hi) >> 1;
            if (s[mid].until < seen) lo = mid + 1;
            else hi = mid;
        }
        return lo < n ? s[lo].delay : current;
    }

    function _pay(address token, address to, uint256 amount) internal {
        if (amount == 0) return;
        if (token == ETH) {
            (bool ok,) = to.call{value: amount}("");
            if (!ok) revert EthTransferFailed();
        } else {
            IERC20(token).safeTransfer(to, amount);
        }
    }

    // ------------------------------------------------------------------ caps (F#11)

    /// Count `denom` against the token's cap and, if the token has a weight, the global cap.
    /// A rejection reverts the whole call, so nothing is consumed and it can be retried later.
    function _charge(TokenConfig storage c, address token, uint256 denom, bool isDeposit) internal {
        uint256 w = c.capWindow;
        if (w != 0) _use(isDeposit ? c.dep : c.rel, w, isDeposit ? c.depositCap : c.releaseCap, denom);
        w = globalCapWindow;
        if (w != 0) {
            uint256 weight = capWeight[token];
            if (weight != 0) {
                // saturate instead of overflowing, so a huge weight can't panic a release
                uint256 g = denom <= type(uint256).max / weight ? denom * weight / 1e18 : type(uint256).max;
                _use(
                    isDeposit ? _globalDep : _globalRel, w, isDeposit ? globalDepositCap : globalReleaseCap, g
                );
            }
        }
    }

    function _use(Bucket storage b, uint256 window, uint256 cap, uint256 amount) internal {
        (uint256 used, uint256 prev, uint256 cur, uint256 index) = _usage(b, window);
        if (amount > cap || used > cap - amount) revert CapExceeded();
        cur = amount > MAX112 - cur ? MAX112 : cur + amount;
        b.index = uint32(index);
        b.prev = uint112(prev);
        b.cur = uint112(cur);
    }

    /// Current estimate for a bucket pair; buckets older than the previous one count as empty.
    function _usage(Bucket storage b, uint256 window)
        internal
        view
        returns (uint256 used, uint256 prev, uint256 cur, uint256 index)
    {
        if (window == 0) return (0, 0, 0, 0);
        index = uint32(block.timestamp / window); // wraps only if window < 2 s, after 2106
        uint256 stored = b.index;
        if (stored == index) {
            prev = b.prev;
            cur = b.cur;
        } else if (stored + 1 == index) {
            prev = b.cur;
        }
        used = prev * (window - block.timestamp % window) / window + cur;
    }

    /// On a window change, carry the current estimate into the new window's current bucket,
    /// so changing caps neither resets usage nor leaves state that could underflow.
    function _rewindow(Bucket storage b, uint256 oldWindow, uint256 newWindow) internal {
        if (oldWindow == newWindow || newWindow == 0) return;
        (uint256 used,,,) = _usage(b, oldWindow);
        b.index = uint32(block.timestamp / newWindow);
        b.prev = 0;
        b.cur = uint112(used > MAX112 ? MAX112 : used);
    }

    // ------------------------------------------------------------------ admin: registry

    function registerToken(
        address token,
        uint8 decimals_,
        uint256[] calldata denoms,
        uint256[] calldata depositFees,
        uint256[] calldata wFees,
        uint64 releaseDelay
    ) external onlyAdmin {
        TokenConfig storage c = _tokens[token];
        if (c.registered || c.denoms.length != 0) revert AlreadyRegistered();
        uint8 actual = token == ETH ? 18 : IERC20Metadata(token).decimals();
        if (actual != decimals_) revert BadDecimals();
        if (denoms.length == 0 || denoms.length > MAX_DENOMS) revert BadDenomination();
        if (depositFees.length != denoms.length || wFees.length != denoms.length) revert BadDenomination();
        if (releaseDelay < minReleaseDelay) revert DelayTooShort();
        for (uint256 i; i < denoms.length; ++i) {
            if (denoms[i] == 0) revert BadDenomination();
            for (uint256 m; m < i; ++m) {
                if (denoms[m] == denoms[i]) revert BadDenomination();
            }
            _checkFee(denoms[i], depositFees[i]);
            _checkFee(denoms[i], wFees[i]);
        }
        c.registered = true;
        c.decimals = decimals_;
        c.releaseDelay = releaseDelay;
        c.denoms = denoms;
        c.depositFees = depositFees;
        c.withdrawFees = wFees;
        tokenList.push(token);
        emit TokenRegistered(token, decimals_, denoms);
    }

    /// Stop new deposits of a token. Existing positions still claim and release (F#10).
    function deregisterToken(address token) external onlyAdmin {
        if (!_tokens[token].registered) revert NotRegistered();
        _tokens[token].registered = false;
        emit TokenDeregistered(token);
    }

    /// Denominations are append-only, so counters and indices never change meaning.
    function addDenomination(address token, uint256 denom, uint256 depositFee, uint256 withdrawFee)
        external
        onlyAdmin
    {
        TokenConfig storage c = _tokens[token];
        if (!c.registered) revert NotRegistered();
        if (denom == 0 || c.denoms.length >= MAX_DENOMS) revert BadDenomination();
        for (uint256 i; i < c.denoms.length; ++i) {
            if (c.denoms[i] == denom) revert BadDenomination();
        }
        _checkFee(denom, depositFee);
        _checkFee(denom, withdrawFee);
        c.denoms.push(denom);
        c.depositFees.push(depositFee);
        c.withdrawFees.push(withdrawFee);
        emit DenominationAdded(token, uint32(c.denoms.length - 1), denom);
    }

    /// Lowering or keeping fees unchanged is instant; raising any deposit or withdraw fee
    /// weakens safety for users and must go through `schedule(Action.SetFees, ...)` / `execute`.
    function setFees(address token, uint256[] calldata depositFees, uint256[] calldata wFees)
        external
        onlyAdmin
    {
        _setFees(token, depositFees, wFees, true);
    }

    function _setFees(address token, uint256[] memory depositFees, uint256[] memory wFees, bool mustNotRaise)
        internal
    {
        TokenConfig storage c = _tokens[token];
        if (c.denoms.length == 0) revert NotRegistered();
        if (depositFees.length != c.denoms.length || wFees.length != c.denoms.length) {
            revert BadDenomination();
        }
        for (uint256 i; i < c.denoms.length; ++i) {
            _checkFee(c.denoms[i], depositFees[i]);
            _checkFee(c.denoms[i], wFees[i]);
            if (mustNotRaise && (depositFees[i] > c.depositFees[i] || wFees[i] > c.withdrawFees[i])) {
                revert TimelockRequired();
            }
        }
        c.depositFees = depositFees;
        c.withdrawFees = wFees;
        emit FeesSet(token, depositFees, wFees);
    }

    /// F#13: replace the relay-fee menu of every denomination of `token`. `menus[i]` holds up
    /// to MAX_RELAY_FEES distinct non-zero fees for denomination i, each within MAX_FEE_BPS of
    /// the denomination and with fee + withdraw fee <= denomination. An empty list leaves
    /// only fee 0. Denominations added later start with an empty menu.
    ///
    /// A menu change never strands an exit. The exit commitment fixes only a maxRelayFee, not
    /// the fee itself; the fee is chosen when the G3 proof is made. Fee 0 is always accepted
    /// (the user submits the release themselves), and so is any value on the current menu
    /// that is <= their maxRelayFee. Changing the menu does not touch the nullifier, so an
    /// exit whose seal named a fee that was removed is still unspent: that seal stops working,
    /// and the user makes a new proof with fee 0 or a value still listed.
    function setRelayFeeMenu(address token, uint256[][] calldata menus) external onlyAdmin {
        TokenConfig storage c = _tokens[token];
        uint256 n = c.denoms.length;
        if (n == 0) revert NotRegistered();
        if (menus.length != n) revert BadDenomination();
        for (uint256 i; i < n; ++i) {
            uint256[] calldata m = menus[i];
            if (m.length > MAX_RELAY_FEES) revert BadRelayFeeMenu();
            mapping(uint256 => bool) storage allowed = relayFeeAllowed[token][i];
            uint256[] storage old = _relayFeeMenu[token][i];
            for (uint256 k; k < old.length; ++k) allowed[old[k]] = false;
            uint256 denom = c.denoms[i];
            for (uint256 k; k < m.length; ++k) {
                uint256 fee = m[k];
                if (fee == 0 || allowed[fee]) revert BadRelayFeeMenu(); // 0 is implicit; no duplicates
                _checkFee(denom, fee);
                if (fee + c.withdrawFees[i] > denom) revert FeeTooHigh();
                allowed[fee] = true;
            }
            _relayFeeMenu[token][i] = m;
        }
        emit RelayFeeMenuSet(token, menus);
    }

    /// Enabling caps from disabled, lowering either cap, or lengthening the window is instant;
    /// raising either cap, shortening the window, or disabling caps (`window == 0`) requires the
    /// admin timelock (`Action.SetCaps`).
    function setCaps(address token, uint64 window, uint256 depositCap, uint256 releaseCap) external onlyAdmin {
        TokenConfig storage c = _tokens[token];
        if (c.denoms.length == 0) revert NotRegistered();
        if (!_isCapTightening(c.capWindow, c.depositCap, c.releaseCap, window, depositCap, releaseCap)) {
            revert TimelockRequired();
        }
        _setCaps(token, window, depositCap, releaseCap);
    }

    function _setCaps(address token, uint64 window, uint256 depositCap, uint256 releaseCap) internal {
        TokenConfig storage c = _tokens[token];
        if (c.denoms.length == 0) revert NotRegistered();
        // usage already recorded is kept (carried over on a window change)
        _rewindow(c.dep, c.capWindow, window);
        _rewindow(c.rel, c.capWindow, window);
        c.capWindow = window;
        c.depositCap = depositCap;
        c.releaseCap = releaseCap;
        emit CapsSet(token, window, depositCap, releaseCap);
    }

    /// Caps across all tokens with a nonzero `capWeight`, in normalised units. Window 0 disables
    /// (requires the admin timelock). Enabling, lowering caps, or lengthening the window is instant;
    /// loosening requires `Action.SetGlobalCaps`.
    function setGlobalCaps(uint64 window, uint256 depositCap, uint256 releaseCap) external onlyAdmin {
        if (!_isCapTightening(globalCapWindow, globalDepositCap, globalReleaseCap, window, depositCap, releaseCap)) {
            revert TimelockRequired();
        }
        _setGlobalCaps(window, depositCap, releaseCap);
    }

    function _setGlobalCaps(uint64 window, uint256 depositCap, uint256 releaseCap) internal {
        _rewindow(_globalDep, globalCapWindow, window);
        _rewindow(_globalRel, globalCapWindow, window);
        globalCapWindow = window;
        globalDepositCap = depositCap;
        globalReleaseCap = releaseCap;
        emit GlobalCapsSet(window, depositCap, releaseCap);
    }

    function _isCapTightening(
        uint64 curWindow,
        uint256 curDep,
        uint256 curRel,
        uint64 newWindow,
        uint256 newDep,
        uint256 newRel
    ) internal pure returns (bool) {
        if (newWindow == 0) return false;
        if (curWindow == 0) return true;
        return newWindow >= curWindow && newDep <= curDep && newRel <= curRel;
    }

    /// Global units per smallest token unit, scaled by 1e18 (e.g. USD with 6 decimals: USDC
    /// 1e18, ETH at $3000 3e9). 0 exempts the token from the global caps. Raising or keeping
    /// the weight is instant; lowering it (including to 0) requires `Action.SetCapWeight`.
    function setCapWeight(address token, uint256 weight) external onlyAdmin {
        if (_tokens[token].denoms.length == 0) revert NotRegistered();
        if (weight < capWeight[token]) revert TimelockRequired();
        _setCapWeight(token, weight);
    }

    function _setCapWeight(address token, uint256 weight) internal {
        if (_tokens[token].denoms.length == 0) revert NotRegistered();
        capWeight[token] = weight;
        emit CapWeightSet(token, weight);
    }

    /// F#12: an exit root recorded at time t waits, for this token, the lowest delay in force at
    /// any moment from t to now: the delay in force when it was recorded, cut by any later
    /// lowering. A raise applies only to roots recorded after it, never lengthens the wait of a
    /// root already recorded, and never makes a releasable root wait again. A root recorded in
    /// the same second as a change gets the lower of the two (the order inside a block is not
    /// recorded). Every delay is >= minReleaseDelay, so no root ever waits less than that.
    ///
    /// Raising or keeping the delay is instant (an emergency brake); lowering it weakens safety
    /// and requires `Action.SetReleaseDelay` through the admin timelock.
    ///
    /// The delay is per token but an exit root covers exits of many tokens, so nothing per root
    /// is stored; each token keeps the history it needs (`delaySteps`), a stack where a past
    /// delay survives only while it is below every later delay (one it isn't below can never be
    /// the minimum for any root). A raise pushes the outgoing delay with `until` = now (O(1),
    /// no search: an emergency raise always goes through). A lowering to d drops every step
    /// with delay >= d by binary search (O(log n)), and the outgoing delay too. So the stack
    /// grows by at most one per raise, and `release` reads at most ceil(log2(n + 1)) steps
    /// plus the count, and none when the root was recorded after the last change or the
    /// current delay already allows it. No cap: refusing a raise when full would block an
    /// emergency raise, and merging steps would change some root's wait.
    function setReleaseDelay(address token, uint64 delay) external onlyAdmin {
        TokenConfig storage c = _tokens[token];
        if (c.denoms.length == 0) revert NotRegistered();
        if (delay < minReleaseDelay) revert DelayTooShort();
        if (delay < c.releaseDelay) revert TimelockRequired();
        _setReleaseDelay(token, delay);
    }

    function _setReleaseDelay(address token, uint64 delay) internal {
        TokenConfig storage c = _tokens[token];
        if (c.denoms.length == 0) revert NotRegistered();
        if (delay < minReleaseDelay) revert DelayTooShort();
        uint256 d = c.releaseDelay;
        if (delay != d) {
            mapping(uint256 => DelayStep) storage s = delaySteps[token];
            uint256 n = _delayStepCount[token];
            if (delay > d) {
                // every step is below d < delay: nothing to drop
                s[n] = DelayStep(uint64(d), uint64(block.timestamp));
                n += 1;
            } else {
                // keep the steps below `delay` (a prefix, delays increase); drop d and the rest
                uint256 lo;
                while (lo < n) {
                    uint256 mid = (lo + n) >> 1;
                    if (s[mid].delay < delay) lo = mid + 1;
                    else n = mid;
                }
            }
            _delayStepCount[token] = n;
            c.delayChangedAt = uint64(block.timestamp);
            c.releaseDelay = delay;
        }
        emit ReleaseDelaySet(token, delay);
    }

    function _checkFee(uint256 denom, uint256 fee) internal pure {
        if (fee * 10_000 > denom * MAX_FEE_BPS) revert FeeTooHigh();
    }

    /// Fees only: principal can't be withdrawn, because accruedFees counts fees alone.
    function withdrawFees(address token) external nonReentrant {
        TokenConfig storage c = _tokens[token];
        uint256 amount = c.accruedFees;
        c.accruedFees = 0;
        _pay(token, feeRecipient, amount);
        emit FeesWithdrawn(token, feeRecipient, amount);
    }

    // ------------------------------------------------------------------ admin: freeze, roles and timelock

    /// The freezer (or admin) can freeze immediately; lifting a freeze goes through the admin
    /// timelock (`Action.Unfreeze` / `Action.UnfreezeAll`) (F#14, B1.5). Deposits and releases
    /// are separate flags: a true argument sets (or clears) that flag, false leaves it as it is.
    /// A release freeze also stops `update` when global, not when per token.
    function freeze(address token, bool deposits, bool releases) external onlyFreezer {
        TokenConfig storage c = _tokens[token];
        if (deposits) c.depositsFrozen = true;
        if (releases) c.releasesFrozen = true;
        lastFreezeAt = uint64(block.timestamp);
        emit Frozen(token, deposits, releases);
    }

    function freezeAll(bool deposits, bool releases) external onlyFreezer {
        if (deposits) globalDepositsFrozen = true;
        if (releases) globalReleasesFrozen = true;
        lastFreezeAt = uint64(block.timestamp);
        emit Frozen(GLOBAL, deposits, releases);
    }

    /// Second step of the two-step admin handover. `pendingAdmin` can only be set through
    /// `schedule(Action.ProposeAdmin, ...)` / `execute`.
    function acceptAdmin() external {
        if (msg.sender != pendingAdmin) revert NotPendingAdmin();
        admin = msg.sender;
        pendingAdmin = address(0);
        emit AdminAccepted(msg.sender);
    }

    /// Deterministic id of a timelocked admin action.
    function actionId(Action action, bytes calldata params) public pure returns (bytes32) {
        return keccak256(abi.encode(action, params));
    }

    /// Queue a safety-weakening admin action for execution at `eta = block.timestamp + adminDelay`.
    /// An already-scheduled action cannot be overwritten until it is executed, cancelled, or
    /// expired (`block.timestamp >= eta + ADMIN_GRACE_PERIOD`).
    function schedule(Action action, bytes calldata params) external onlyAdmin returns (bytes32 id) {
        id = actionId(action, params);
        uint64 existing = scheduledEta[id];
        if (existing != 0 && block.timestamp < uint256(existing) + ADMIN_GRACE_PERIOD) {
            revert AlreadyScheduled();
        }
        uint64 eta = uint64(block.timestamp) + adminDelay;
        scheduledEta[id] = eta;
        emit ActionScheduled(id, action, params, eta);
    }

    /// Execute a scheduled admin action once `block.timestamp >= eta` and strictly before
    /// `eta + ADMIN_GRACE_PERIOD`. Clears `scheduledEta[id]` first so it cannot execute twice.
    function execute(Action action, bytes calldata params) external onlyAdmin {
        bytes32 id = actionId(action, params);
        uint64 eta = scheduledEta[id];
        if (eta == 0) revert NotScheduled();
        if (block.timestamp < eta) revert ActionTooEarly();
        if (block.timestamp >= uint256(eta) + ADMIN_GRACE_PERIOD) revert ActionExpired();
        if ((action == Action.Unfreeze || action == Action.UnfreezeAll) && eta - adminDelay < lastFreezeAt) {
            revert ScheduledBeforeFreeze();
        }
        scheduledEta[id] = 0;

        if (action == Action.Unfreeze) {
            (address token, bool deposits, bool releases) = abi.decode(params, (address, bool, bool));
            TokenConfig storage c = _tokens[token];
            if (deposits) c.depositsFrozen = false;
            if (releases) c.releasesFrozen = false;
            emit Unfrozen(token, deposits, releases);
        } else if (action == Action.UnfreezeAll) {
            (bool deposits, bool releases) = abi.decode(params, (bool, bool));
            if (deposits) globalDepositsFrozen = false;
            if (releases) globalReleasesFrozen = false;
            emit Unfrozen(GLOBAL, deposits, releases);
        } else if (action == Action.SetFreezer) {
            address next = abi.decode(params, (address));
            freezer = next;
            emit FreezerSet(next);
        } else if (action == Action.ProposeAdmin) {
            address next = abi.decode(params, (address));
            pendingAdmin = next;
            emit AdminProposed(next);
        } else if (action == Action.SetFeeRecipient) {
            address next = abi.decode(params, (address));
            if (next == address(0)) revert ZeroAddress();
            feeRecipient = next;
            emit FeeRecipientSet(next);
        } else if (action == Action.SetCaps) {
            (address token, uint64 window, uint256 depositCap, uint256 releaseCap) =
                abi.decode(params, (address, uint64, uint256, uint256));
            _setCaps(token, window, depositCap, releaseCap);
        } else if (action == Action.SetGlobalCaps) {
            (uint64 window, uint256 depositCap, uint256 releaseCap) =
                abi.decode(params, (uint64, uint256, uint256));
            _setGlobalCaps(window, depositCap, releaseCap);
        } else if (action == Action.SetCapWeight) {
            (address token, uint256 weight) = abi.decode(params, (address, uint256));
            _setCapWeight(token, weight);
        } else if (action == Action.SetReleaseDelay) {
            (address token, uint64 delay) = abi.decode(params, (address, uint64));
            _setReleaseDelay(token, delay);
        } else {
            (address token, uint256[] memory depositFees, uint256[] memory wFees) =
                abi.decode(params, (address, uint256[], uint256[]));
            _setFees(token, depositFees, wFees, false);
        }

        emit ActionExecuted(id, action, params, eta);
    }

    /// Cancel a scheduled admin action. Callable by the admin OR the freezer, so the freezer
    /// can veto a hostile change (e.g. swapping the freezer or lifting a freeze).
    function cancel(bytes32 id) external onlyFreezer {
        if (scheduledEta[id] == 0) revert NotScheduled();
        scheduledEta[id] = 0;
        emit ActionCancelled(id, msg.sender);
    }

    // ------------------------------------------------------------------ views

    function depositRoot() external view returns (bytes32) {
        return _depositTree.root;
    }

    function depositCount() external view returns (uint64) {
        return _depositTree.nextIndex;
    }

    function exitRoot() external view returns (bytes32) {
        return _exitTree.root;
    }

    function exitCount() external view returns (uint64) {
        return _exitTree.nextIndex;
    }

    function tokenConfig(address token)
        external
        view
        returns (
            bool registered,
            bool depositsFrozen,
            bool releasesFrozen,
            uint8 decimals,
            uint64 releaseDelay,
            uint256[] memory denoms,
            uint256[] memory depositFees,
            uint256[] memory withdrawFees_,
            uint256 accruedFees
        )
    {
        TokenConfig storage c = _tokens[token];
        return (
            c.registered,
            c.depositsFrozen,
            c.releasesFrozen,
            c.decimals,
            c.releaseDelay,
            c.denoms,
            c.depositFees,
            c.withdrawFees,
            c.accruedFees
        );
    }

    /// The allowed non-zero relay fees of one denomination. Fee 0 is always allowed too.
    function relayFeeMenu(address token, uint32 denomIndex) external view returns (uint256[] memory) {
        return _relayFeeMenu[token][denomIndex];
    }

    /// Per-token caps and the rolling usage estimate now, in token units.
    function capState(address token)
        external
        view
        returns (uint64 window, uint256 depositCap, uint256 releaseCap, uint256 depUsed, uint256 relUsed)
    {
        TokenConfig storage c = _tokens[token];
        window = c.capWindow;
        (depUsed,,,) = _usage(c.dep, window);
        (relUsed,,,) = _usage(c.rel, window);
        return (window, c.depositCap, c.releaseCap, depUsed, relUsed);
    }

    /// Global caps and the rolling usage estimate now, in normalised units.
    function globalCapState()
        external
        view
        returns (uint64 window, uint256 depositCap, uint256 releaseCap, uint256 depUsed, uint256 relUsed)
    {
        window = globalCapWindow;
        (depUsed,,,) = _usage(_globalDep, window);
        (relUsed,,,) = _usage(_globalRel, window);
        return (window, globalDepositCap, globalReleaseCap, depUsed, relUsed);
    }

    /// A token's current release delay, the time of its last effective change (0: none), and
    /// the number of history steps (read them with `delaySteps(token, i)`, i < steps) (F#12).
    function releaseDelayState(address token)
        external
        view
        returns (uint64 delay, uint64 changedAt, uint256 steps)
    {
        TokenConfig storage c = _tokens[token];
        return (c.releaseDelay, c.delayChangedAt, _delayStepCount[token]);
    }

    /// When exits of `token` under `root` become releasable as things stand now (a later
    /// lowering can only bring it forward); 0 if the root is unknown.
    function releasableAt(address token, bytes32 root) external view returns (uint256) {
        uint256 seen = exitRootTime[root];
        if (seen == 0) return 0;
        uint256 delay = _tokens[token].releaseDelay;
        return seen + _delayFor(token, delay, seen);
    }
}
