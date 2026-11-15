// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";

/// The parts of BridgeVault the desk uses.
interface IBridgeVaultDeposits {
    function deposit(address token, uint32 denomIndex, bytes32 commitment) external returns (uint64 leafIndex);
    function depositETH(uint32 denomIndex, bytes32 commitment) external payable returns (uint64 leafIndex);
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
            uint256[] memory withdrawFees,
            uint256 accruedFees
        );
}

/// @title DepositDesk: a DEMO-NETWORK convenience in front of an immutable BridgeVault.
///
/// One transaction for the depositor: the desk takes the vault's exact deposit amount
/// (denomination + the vault's own deposit fee) plus a bridge fee of `feeBps` of the
/// denomination, in the SAME asset, forwards the vault's amount to `vault` in the same call,
/// keeps the bridge fee, and emits `FeePaid`. The bridge fee pays the LEZ side of the demo: an
/// operator grants native LEZ to the public account named by `lezPublicKey` so a claimer with an
/// empty account can claim; a zero key (private and demo accounts) is served by the bridge's
/// float. The fee is charged on every deposit made through the desk.
///
/// What the desk can do: hold bridge fees until the owner withdraws them. What it can't do:
/// keep, redirect or delay a deposit (the vault records it in the same call, or the whole
/// transaction reverts), change a commitment, or touch the vault (it has no role there). The
/// leaf is the same as a direct deposit's: the vault binds only token, denomination and
/// commitment, never the caller. The owner can lower or raise the fee within MAX_FEE_BPS and
/// withdraw collected fees; nothing else. No upgrade path, no receive function.
///
/// Rounding: fee = denom * feeBps / 10_000, rounded DOWN (in the depositor's favour). Every
/// current demo denomination divides exactly at 300 bps.
///
/// Not part of the trust-minimised bridge: the grant on the LEZ side is the operator's promise,
/// not something this contract or the vault enforces.
contract DepositDesk is ReentrancyGuard {
    using SafeERC20 for IERC20;

    address public constant ETH = address(0);
    /// Hard cap: the bridge fee can never exceed 3% of a denomination.
    uint256 public constant MAX_FEE_BPS = 300;
    uint256 public constant BPS = 10_000;

    IBridgeVaultDeposits public immutable vault;
    address public immutable owner;

    /// Bridge fee in basis points of the denomination.
    uint256 public feeBps;
    /// Bridge fees collected and not yet withdrawn, per token (ETH = address(0)).
    mapping(address => uint256) public feesHeld;

    /// One per deposit made through the desk. `lezPublicKey` is the 32-byte public key of the
    /// LEZ public account the operator should fund for the claim's fee, or zero for none.
    event FeePaid(
        address indexed token,
        bytes32 indexed lezPublicKey,
        bytes32 indexed commitment,
        uint32 denomIndex,
        uint64 leafIndex,
        uint256 fee
    );
    event FeeBpsSet(uint256 feeBps);
    event FeesWithdrawn(address indexed token, address to, uint256 amount);

    error NotOwner();
    error FeeTooHigh();
    error ZeroAddress();
    error NoVaultCode();
    error WrongAmount();
    error UnexpectedValue();
    error BadDenomination();
    error EthTransferFailed();

    constructor(IBridgeVaultDeposits vault_, address owner_, uint256 feeBps_) {
        if (address(vault_) == address(0) || owner_ == address(0)) revert ZeroAddress();
        if (address(vault_).code.length == 0) revert NoVaultCode();
        if (feeBps_ > MAX_FEE_BPS) revert FeeTooHigh();
        vault = vault_;
        owner = owner_;
        feeBps = feeBps_;
        emit FeeBpsSet(feeBps_);
    }

    // ------------------------------------------------------------------ quotes

    /// The bridge fee on `denom` at the current rate (rounded down).
    function feeFor(uint256 denom) public view returns (uint256) {
        return denom * feeBps / BPS;
    }

    /// What a deposit through the desk costs, read from the vault: the denomination, the vault's
    /// own deposit fee, the bridge fee, and their sum (what the depositor sends or approves).
    /// Reverts like the vault for an unknown token or index.
    function quote(address token, uint32 denomIndex)
        public
        view
        returns (uint256 denom, uint256 vaultFee, uint256 bridgeFee, uint256 total)
    {
        (,,,,, uint256[] memory denoms, uint256[] memory fees,,) = vault.tokenConfig(token);
        if (denomIndex >= denoms.length) revert BadDenomination();
        denom = denoms[denomIndex];
        vaultFee = fees[denomIndex];
        bridgeFee = feeFor(denom);
        total = denom + vaultFee + bridgeFee;
    }

    // ------------------------------------------------------------------ deposits

    /// Deposit native ETH: `msg.value` must be exactly `quote(ETH, denomIndex).total`.
    function depositETHWithFee(uint32 denomIndex, bytes32 commitment, bytes32 lezPublicKey)
        external
        payable
        nonReentrant
        returns (uint64 leafIndex)
    {
        (uint256 denom, uint256 vaultFee, uint256 fee, uint256 total) = quote(ETH, denomIndex);
        if (msg.value != total) revert WrongAmount();
        feesHeld[ETH] += fee;
        // The vault checks msg.value == denom + its fee itself, and records the leaf.
        leafIndex = vault.depositETH{value: denom + vaultFee}(denomIndex, commitment);
        emit FeePaid(ETH, lezPublicKey, commitment, denomIndex, leafIndex, fee);
    }

    /// Deposit an ERC-20: the depositor approves the desk for exactly
    /// `quote(token, denomIndex).total`; the desk pulls that in one transfer, approves the vault
    /// for the vault's amount, and the vault pulls it in the same call. No ETH.
    function depositWithFee(address token, uint32 denomIndex, bytes32 commitment, bytes32 lezPublicKey)
        external
        payable
        nonReentrant
        returns (uint64 leafIndex)
    {
        if (msg.value != 0) revert UnexpectedValue();
        if (token == ETH) revert BadDenomination();
        (uint256 denom, uint256 vaultFee, uint256 fee, uint256 total) = quote(token, denomIndex);
        IERC20 t = IERC20(token);
        uint256 before = t.balanceOf(address(this));
        t.safeTransferFrom(msg.sender, address(this), total);
        // Fee-on-transfer and similar tokens: the desk must receive exactly what it pulled.
        if (t.balanceOf(address(this)) - before != total) revert WrongAmount();
        uint256 toVault = denom + vaultFee;
        t.forceApprove(address(vault), toVault);
        leafIndex = vault.deposit(token, denomIndex, commitment);
        // The vault took exactly its amount: the desk is left with its balance before plus the
        // fee (compared as a difference, so tokens sent to the desk by others change nothing).
        if (t.balanceOf(address(this)) != before + fee) revert WrongAmount();
        if (t.allowance(address(this), address(vault)) != 0) t.forceApprove(address(vault), 0);
        feesHeld[token] += fee;
        emit FeePaid(token, lezPublicKey, commitment, denomIndex, leafIndex, fee);
    }

    // ------------------------------------------------------------------ owner

    function setFeeBps(uint256 next) external {
        if (msg.sender != owner) revert NotOwner();
        if (next > MAX_FEE_BPS) revert FeeTooHigh();
        feeBps = next;
        emit FeeBpsSet(next);
    }

    /// Send the collected bridge fees of `token` (ETH = address(0)) to `to`. Only fees: the
    /// desk never holds a deposit past the call that made it.
    function withdrawFees(address token, address to) external nonReentrant {
        if (msg.sender != owner) revert NotOwner();
        if (to == address(0)) revert ZeroAddress();
        uint256 amount = feesHeld[token];
        feesHeld[token] = 0;
        if (token == ETH) {
            (bool ok,) = to.call{value: amount}("");
            if (!ok) revert EthTransferFailed();
        } else {
            IERC20(token).safeTransfer(to, amount);
        }
        emit FeesWithdrawn(token, to, amount);
    }
}
