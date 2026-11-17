// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.28;

import {IRiscZeroVerifier} from "./IRiscZeroVerifier.sol";

/// The vault side the gate drives (`BridgeVault`, unchanged).
interface ISeqSigVault {
    function update(bytes calldata journal, bytes calldata seal) external;
    function verifier() external view returns (IRiscZeroVerifier);
    function updateImageId() external view returns (bytes32);
    function cfgId() external view returns (bytes32);
    function g2State() external view returns (bytes32);
}

/// ONE vault's verifier for the "sequencer signatures" route (branch `seq-sig`): an update counts
/// when THRESHOLD of a fixed set of SIGNERS Ed25519 keys signed the LEZ block it ends at, and anyone
/// may submit it. No submitter key, no admin.
///
/// Bound to one vault for good: `VAULT` and its `CFG_ID` are immutables. The vault's constructor
/// needs this gate's address, so the gate is deployed first with the vault's PREDICTED address (the
/// deployer's next CREATE address, `vm.computeCreateAddress(deployer, nonce + 1)` in
/// `script/SeqSigDeploy.s.sol`, which then checks the vault landed there); the gate computes
/// `CFG_ID` itself with the vault's formula (`BridgeVault.computeCfgId`) and compares journals and
/// evidence with it, never with a live call. `submit` also checks the deployed vault is wired to
/// this gate with that cfgId and image (reads of the vault's immutables).
///
/// The update image (`outbound_guest_seqsig`) proves the TrustedTip statement (block linkage,
/// extraction, the claim accumulator) AND at least `threshold` valid signatures from distinct,
/// not ejected keys of the set over the checkpoint (cfgId, previous LEZ tip, end block id and
/// hash). Its journal is the vault's `abi.encode(UpdateJournal)` followed by 256 bytes:
/// `abi.encode(signerSetHash, threshold, ejectedMask, signerBitmap, prevId, prevHash, blockId,
/// blockHash)`. The evidence image (`seqsig_evidence_guest`) proves which set keys signed some
/// checkpoint: `abi.encode(cfgId, signerSetHash, signerBitmap, prevId, prevHash, blockId,
/// blockHash)`.
///
/// - `submit` (anyone): the tail's set hash and threshold must equal this gate's, its ejected mask
///   must be a past value of the gate's (a subset: ejections only add), and no signer in its bitmap
///   may be ejected now (so an ejection invalidates only proofs that counted the ejected key);
///   `VERIFIER` must accept the seal over sha256 of the WHOLE journal; the vault part must name the
///   vault's cfgId and current `g2State` and advance it; the vault must accept it (a simulated,
///   reverted `update`). The update is then PENDING, with its signer bitmap and checkpoint; nothing
///   in the vault changes.
/// - `contest` (anyone, inside the window): evidence over the SAME (cfgId, previous tip, block id)
///   with ANOTHER block hash.
///   * Keys in it that also signed the pending update equivocated: they are EJECTED for good (their
///     signatures never count again). The update is cancelled, and its checkpoint can never be
///     staged again, only if fewer than THRESHOLD of its signers remain active; otherwise it stays
///     staged (it still has a quorum of keys that did not equivocate).
///   * If the evidence carries at least THRESHOLD not ejected keys, two quorums signed conflicting
///     blocks: the gate HALTS for good (no submit, no finalize, no contest; releases of already
///     applied exits still pass). That is the safe failure: a deployer then brings up a new gate and
///     vault version, and the vault's freezer can freeze releases meanwhile.
///   * Evidence only from keys that did not sign the update cancels nothing.
/// - `finalize` (anyone, after CONTEST_WINDOW): applies the update to the vault through a transient
///   mark. (A staged update never holds fewer than THRESHOLD active signers: the contest that would
///   leave it so cancels it in the same transaction, and no other path ejects keys.)
/// - `verify`: the update image passes only during `finalize` or the simulation, for that vault
///   and that vault journal (its seal was checked over the whole journal at `submit`); the release
///   image passes to `VERIFIER`; any other image is refused.
///
/// THRESHOLD stays measured against the original n: ejections never lower it. When fewer than
/// THRESHOLD keys remain active, the gate is dead (`submit` refuses). A real gate needs
/// n/2 < THRESHOLD <= n - 1 (at T = n one silent key stops every update); T = n only behind the
/// test-only flag.
///
/// Conditions, for n keys, threshold T with n/2 < T <= n - 1, f dishonest keys and o honest keys
/// offline (honest keys sign only checkpoints they replayed and consider final, and keep a record
/// so they never sign two hashes for one (cfgId, previous tip, block id)):
/// - Safety: an update over blocks the honest keys did not vouch for needs T signatures from keys
///   not ejected: impossible while f <= T - 1.
/// - Halt: honest keys sign no conflict on one LEZ history, so a conflicting quorum needs f >= T.
///   (If honest keys could be shown different forks of LEZ, honest signatures could conflict too,
///   and a halt would need only f >= 2T - n.)
/// - Liveness: updates can be signed while n - f - o >= T. A cancellation needs an equivocation that
///   leaves fewer than T active signers of the staged update, and ejects every equivocator, so at
///   most f updates are ever cancelled through this gate.
/// - Together: f + o <= n - T (then f <= T - 1 holds too, since T > n/2).
///
/// In this repository's demo the keys are STAND-INS (keys of ours), not LEZ sequencer keys.
contract SeqSigGate is IRiscZeroVerifier {
    IRiscZeroVerifier public immutable VERIFIER;
    /// The one vault this gate serves (its predicted address at deploy time).
    ISeqSigVault public immutable VAULT;
    /// That vault's cfgId, computed here with `BridgeVault.computeCfgId`'s formula.
    bytes32 public immutable CFG_ID;
    bytes32 public immutable UPDATE_IMAGE_ID;
    bytes32 public immutable EVIDENCE_IMAGE_ID;
    bytes32 public immutable RELEASE_IMAGE_ID;
    /// SHA256("RFP022/seqsig/set/v1" | u32be(n) | keys), computed here from the constructor's keys.
    bytes32 public immutable SIGNER_SET_HASH;
    uint256 public immutable SIGNERS;
    uint256 public immutable THRESHOLD;
    uint64 public immutable CONTEST_WINDOW;
    /// True only for a test gate: a window under MIN_CONTEST_WINDOW and THRESHOLD = n are allowed
    /// (visible on chain).
    bool public immutable TEST_ONLY_SHORT_WINDOW;

    /// The shortest window a real gate may have: time for a watcher to notice a pending update, prove
    /// the evidence (about 0.93M cycles, one segment), wrap it to Groth16 (about 4 min on a hosted
    /// x86 runner, `DESIGN.md` §4) and get it included under congestion, with a wide margin.
    uint64 public constant MIN_CONTEST_WINDOW = 6 hours;
    bytes32 public constant CFG_TAG = keccak256("RFP022/cfg/v1");
    uint256 public constant MAX_SIGNERS = 64;

    string public constant KIND =
        "seq-sig: RISC Zero verifies every proof; an update needs THRESHOLD of SIGNERS keys over its LEZ end block (in the proof), anyone submits; contest window: equivocators ejected, two quorums halt";

    uint256 internal constant TAIL = 256;
    uint256 internal constant EVIDENCE = 224;
    /// The shortest `abi.encode(UpdateJournal)`: offset, 6 head words, 3 empty array lengths.
    uint256 internal constant MIN_VAULT_JOURNAL = 320;
    uint256 internal constant P = 0x7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffed;
    /// The y coordinates of the two pairs of points of order 8 (`Y8A + Y8B = P`).
    uint256 internal constant Y8A = 0x7a03ac9277fdc74ec6cc392cfa53202a0f67100d760b3cba4fd84d3d706a17c7;
    uint256 internal constant Y8B = 0x05fc536d880238b13933c6d305acdfd5f098eff289f4c345b027b2c28f95e826;

    struct Tail {
        bytes32 setHash;
        uint256 threshold;
        uint256 ejected;
        uint256 signers;
        uint256 prevId;
        bytes32 prevHash;
        uint256 blockId;
        bytes32 blockHash;
    }

    struct Ev {
        bytes32 cfgId;
        bytes32 setHash;
        uint256 signers;
        uint256 prevId;
        bytes32 prevHash;
        uint256 blockId;
        bytes32 blockHash;
    }

    struct Pending {
        bytes32 journalDigest; // sha256 of the vault part of the journal
        bytes32 prevState;
        bytes32 newState;
        bytes32 prevHash;
        bytes32 blockHash;
        uint64 prevId;
        uint64 blockId;
        uint64 acceptedAt;
        uint64 signers;
    }

    /// Keys proven to have equivocated (bit i = key i of the set). Their signatures never count.
    uint64 public ejected;
    /// Two conflicting quorums were shown: nothing is staged or applied any more.
    bool public halted;
    Pending internal _pending;
    /// keccak256(prevId, prevHash, blockId, blockHash) of a cancelled update.
    mapping(bytes32 => bool) public refuted;

    /// keccak256(VAULT, vaultJournalDigest) while `finalize` or the simulation runs; zero otherwise.
    bytes32 private transient _open;

    error ZeroAddress();
    error BadImmutable();
    error BadSignerSet();
    error BadKey(uint256 index);
    error BadThreshold();
    error WindowTooShort();
    error WrongVault();
    error BadJournal();
    error WrongSignerSet();
    error WrongThreshold();
    error WrongEjectedMask();
    error BadSignerBitmap();
    error WrongConfig();
    error StaleState();
    error NoStateAdvance();
    error PendingExists();
    error NoPending();
    error ContestWindowOpen();
    error ContestWindowClosed();
    error WrongJournal();
    error Refuted();
    error BadEvidence();
    error NotTheSameCheckpoint();
    error NoConflict();
    error NoEquivocation();
    error Halted();
    error GateDead();
    error UnknownImage();
    error UpdateNotThroughGate();
    error OnlySelf();
    error SimulationOk();

    event UpdateAccepted(
        bytes32 journalDigest,
        bytes32 newState,
        uint64 blockId,
        bytes32 blockHash,
        uint64 signers,
        uint256 finalizableAt
    );
    event UpdateApplied(bytes32 journalDigest, bytes32 newState);
    /// `cancelled`: the update lost its quorum and was removed (and refuted); else it stays staged.
    event Equivocation(bytes32 journalDigest, uint64 ejectedKeys, bool cancelled, uint64 blockId, bytes32 blockHash, bytes32 conflictingHash);
    event GateHalted(uint64 conflictingQuorum, uint64 blockId, bytes32 blockHash, bytes32 conflictingHash);

    constructor(
        IRiscZeroVerifier verifier_,
        ISeqSigVault vault_,
        bytes32 lezChainId_,
        bytes32 zoneId_,
        bytes32 programId_,
        bytes32 updateImageId_,
        bytes32 evidenceImageId_,
        bytes32 releaseImageId_,
        bytes32[] memory keys,
        uint256 threshold_,
        uint64 contestWindow_,
        bool testOnlyShortWindow_
    ) {
        if (address(verifier_) == address(0) || address(vault_) == address(0)) revert ZeroAddress();
        if (
            updateImageId_ == bytes32(0) || evidenceImageId_ == bytes32(0) || releaseImageId_ == bytes32(0)
                || updateImageId_ == evidenceImageId_ || updateImageId_ == releaseImageId_
                || evidenceImageId_ == releaseImageId_
        ) revert BadImmutable();
        uint256 n = keys.length;
        if (n == 0 || n > MAX_SIGNERS) revert BadSignerSet();
        for (uint256 i; i < n; ++i) {
            if (i != 0 && uint256(keys[i - 1]) >= uint256(keys[i])) revert BadSignerSet();
            if (!keyOk(keys[i])) revert BadKey(i);
        }
        if (threshold_ > n || 2 * threshold_ <= n) revert BadThreshold();
        // T = n: one silent key stops every update and one ejection kills the gate.
        if (threshold_ == n && !testOnlyShortWindow_) revert BadThreshold();
        if (!testOnlyShortWindow_ && contestWindow_ < MIN_CONTEST_WINDOW) revert WindowTooShort();
        VERIFIER = verifier_;
        VAULT = vault_;
        CFG_ID = sha256(abi.encodePacked(CFG_TAG, block.chainid, address(vault_), lezChainId_, zoneId_, programId_));
        UPDATE_IMAGE_ID = updateImageId_;
        EVIDENCE_IMAGE_ID = evidenceImageId_;
        RELEASE_IMAGE_ID = releaseImageId_;
        SIGNER_SET_HASH = sha256(abi.encodePacked("RFP022/seqsig/set/v1", uint32(n), keys));
        SIGNERS = n;
        THRESHOLD = threshold_;
        CONTEST_WINDOW = contestWindow_;
        TEST_ONLY_SHORT_WINDOW = testOnlyShortWindow_;
    }

    /// A canonical encoding (y < p) whose y is not that of a point of small order (orders 1, 2, 4
    /// and 8: y in {1, p-1, 0, Y8A, Y8B}). Whether y is on the curve at all is left to the guests,
    /// which refuse a set holding such a key (the gate would then take no update).
    function keyOk(bytes32 key) public pure returns (bool) {
        uint256 le;
        for (uint256 i; i < 32; ++i) {
            le |= uint256(uint8(key[i])) << (8 * i);
        }
        uint256 y = le & ((1 << 255) - 1);
        return y < P && y != 0 && y != 1 && y != P - 1 && y != Y8A && y != Y8B;
    }

    // ------------------------------------------------------------------ submit

    /// Anyone: stage an update. `journal` is the seq-sig guest's whole journal, `seal` its proof.
    function submit(bytes calldata journal, bytes calldata seal) external {
        if (halted) revert Halted();
        if (activeSigners() < THRESHOLD) revert GateDead();
        if (_pending.acceptedAt != 0) revert PendingExists();
        if (address(VAULT.verifier()) != address(this) || VAULT.updateImageId() != UPDATE_IMAGE_ID || VAULT.cfgId() != CFG_ID) {
            revert WrongVault();
        }
        uint256 n = journal.length;
        if (n < MIN_VAULT_JOURNAL + TAIL) revert BadJournal();
        Tail memory t = abi.decode(journal[n - TAIL:], (Tail));
        if (t.setHash != SIGNER_SET_HASH) revert WrongSignerSet();
        if (t.threshold != THRESHOLD) revert WrongThreshold();
        // The proof's mask must be a past value of `ejected` (ejections only add keys); its signers
        // must all be active now. A proof made before an ejection stays valid unless it counted the
        // ejected key.
        if (t.ejected & ~uint256(ejected) != 0) revert WrongEjectedMask();
        if (t.signers >> SIGNERS != 0 || t.signers & ejected != 0 || _popcount(t.signers) < THRESHOLD) {
            revert BadSignerBitmap();
        }
        if (t.prevId > type(uint64).max || t.blockId > type(uint64).max) revert BadJournal();
        if (refuted[keccak256(abi.encode(t.prevId, t.prevHash, t.blockId, t.blockHash))]) revert Refuted();
        VERIFIER.verify(seal, UPDATE_IMAGE_ID, sha256(journal));

        bytes calldata vj = journal[:n - TAIL];
        bytes32 prev = bytes32(vj[64:96]);
        bytes32 next = bytes32(vj[96:128]);
        if (bytes32(vj[32:64]) != CFG_ID) revert WrongConfig();
        if (prev != VAULT.g2State()) revert StaleState();
        if (next == prev) revert NoStateAdvance();
        bytes32 digest = sha256(vj);
        // The vault's own checks, run now and reverted: an update the vault would refuse is never
        // staged. They only get easier to pass later (deposits only grow, roots are kept, and only
        // this gate moves g2State); they run again, for real, in `finalize`.
        _bubbleUnlessOk(vj);

        _pending = Pending({
            journalDigest: digest,
            prevState: prev,
            newState: next,
            prevHash: t.prevHash,
            blockHash: t.blockHash,
            prevId: uint64(t.prevId),
            blockId: uint64(t.blockId),
            acceptedAt: uint64(block.timestamp),
            signers: uint64(t.signers)
        });
        emit UpdateAccepted(digest, next, uint64(t.blockId), t.blockHash, uint64(t.signers), block.timestamp + CONTEST_WINDOW);
    }

    // ------------------------------------------------------------------ contest

    /// Anyone, inside the window: `evidence` is the evidence image's journal, `seal` its proof.
    function contest(bytes calldata evidence, bytes calldata seal) external {
        if (halted) revert Halted();
        Pending memory p = _pending;
        if (p.acceptedAt == 0) revert NoPending();
        if (block.timestamp >= uint256(p.acceptedAt) + CONTEST_WINDOW) revert ContestWindowClosed();
        if (evidence.length != EVIDENCE) revert BadEvidence();
        Ev memory e = abi.decode(evidence, (Ev));
        if (e.cfgId != CFG_ID) revert WrongConfig();
        if (e.setHash != SIGNER_SET_HASH) revert WrongSignerSet();
        if (e.prevId != p.prevId || e.prevHash != p.prevHash || e.blockId != p.blockId) revert NotTheSameCheckpoint();
        if (e.blockHash == p.blockHash) revert NoConflict();
        if (e.signers == 0 || e.signers >> SIGNERS != 0) revert BadEvidence();
        VERIFIER.verify(seal, EVIDENCE_IMAGE_ID, sha256(evidence));

        uint64 active = uint64(e.signers) & ~ejected;
        if (_popcount(active) >= THRESHOLD) {
            halted = true;
            delete _pending;
            emit GateHalted(active, p.blockId, p.blockHash, e.blockHash);
            return;
        }
        uint64 equivocators = active & p.signers;
        if (equivocators == 0) revert NoEquivocation();
        ejected |= equivocators;
        bool cancelled = _popcount(p.signers & ~ejected) < THRESHOLD;
        if (cancelled) {
            refuted[keccak256(abi.encode(uint256(p.prevId), p.prevHash, uint256(p.blockId), p.blockHash))] = true;
            delete _pending;
        }
        emit Equivocation(p.journalDigest, equivocators, cancelled, p.blockId, p.blockHash, e.blockHash);
    }

    // ------------------------------------------------------------------ finalize

    /// Anyone, once the window has passed: apply the pending update. `vaultJournal` is the vault
    /// part of the journal given to `submit`.
    function finalize(bytes calldata vaultJournal) external {
        if (halted) revert Halted();
        Pending memory p = _pending;
        if (p.acceptedAt == 0) revert NoPending();
        if (block.timestamp < uint256(p.acceptedAt) + CONTEST_WINDOW) revert ContestWindowOpen();
        if (sha256(vaultJournal) != p.journalDigest) revert WrongJournal();
        delete _pending;
        _open = keccak256(abi.encode(address(VAULT), p.journalDigest));
        VAULT.update(vaultJournal, "");
        _open = bytes32(0);
        emit UpdateApplied(p.journalDigest, p.newState);
    }

    // ------------------------------------------------------------------ simulation

    /// Only this contract: runs `VAULT.update` with the gate open, then always reverts
    /// (`SimulationOk` if the vault accepted).
    function simulate(bytes calldata vaultJournal) external {
        if (msg.sender != address(this)) revert OnlySelf();
        _open = keccak256(abi.encode(address(VAULT), sha256(vaultJournal)));
        VAULT.update(vaultJournal, "");
        revert SimulationOk();
    }

    function _bubbleUnlessOk(bytes calldata vaultJournal) internal {
        try this.simulate(vaultJournal) {}
        catch (bytes memory r) {
            if (r.length >= 4 && bytes4(r) == SimulationOk.selector) return;
            assembly ("memory-safe") {
                revert(add(r, 32), mload(r))
            }
        }
    }

    // ------------------------------------------------------------------ IRiscZeroVerifier

    /// The update image passes only for `VAULT`, during `finalize` or the simulation, for the journal
    /// they run; the release image passes to `VERIFIER`; any other image is refused.
    function verify(bytes calldata seal, bytes32 imageId, bytes32 journalDigest) external view {
        if (imageId == UPDATE_IMAGE_ID) {
            if (msg.sender != address(VAULT) || _open != keccak256(abi.encode(msg.sender, journalDigest))) {
                revert UpdateNotThroughGate();
            }
            return;
        }
        if (imageId != RELEASE_IMAGE_ID) revert UnknownImage();
        VERIFIER.verify(seal, imageId, journalDigest);
    }

    // ------------------------------------------------------------------ views

    function pending() external view returns (Pending memory) {
        return _pending;
    }

    /// When the pending update may be finalized (0: none).
    function finalizableAt() external view returns (uint256) {
        uint64 at = _pending.acceptedAt;
        return at == 0 ? 0 : uint256(at) + CONTEST_WINDOW;
    }

    /// Keys whose signatures still count.
    function activeSigners() public view returns (uint256) {
        return SIGNERS - _popcount(ejected);
    }

    /// Fewer than THRESHOLD active keys, or halted: no update can ever be applied again.
    function dead() external view returns (bool) {
        return halted || activeSigners() < THRESHOLD;
    }

    function _popcount(uint256 x) internal pure returns (uint256 c) {
        while (x != 0) {
            x &= x - 1;
            ++c;
        }
    }
}
