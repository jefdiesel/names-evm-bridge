// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {IRiscZeroVerifier} from "../../src/IRiscZeroVerifier.sol";

/// Accepts a seal only if it equals abi.encodePacked(imageId, journalDigest), so tests
/// still catch any change to what the vault binds into the journal.
contract MockVerifier is IRiscZeroVerifier {
    error BadSeal();

    function verify(bytes calldata seal, bytes32 imageId, bytes32 journalDigest) external pure {
        if (keccak256(seal) != keccak256(abi.encodePacked(imageId, journalDigest))) revert BadSeal();
    }

    function sealFor(bytes32 imageId, bytes32 journalDigest) external pure returns (bytes memory) {
        return abi.encodePacked(imageId, journalDigest);
    }
}

/// Stands in ONLY for RISC Zero's RiscZeroGroth16Verifier: a SELECTOR() like the real one and a
/// seal that must equal selector | imageId | journalDigest (the real seal is selector | 256 bytes
/// of Groth16 proof).
contract MockGroth16Verifier is IRiscZeroVerifier {
    error BadSeal();

    bytes4 public immutable SELECTOR;

    constructor(bytes4 selector) {
        SELECTOR = selector;
    }

    function verify(bytes calldata seal, bytes32 imageId, bytes32 journalDigest) external view {
        if (keccak256(seal) != keccak256(abi.encodePacked(SELECTOR, imageId, journalDigest))) revert BadSeal();
    }

    function sealFor(bytes32 imageId, bytes32 journalDigest) external view returns (bytes memory) {
        return abi.encodePacked(SELECTOR, imageId, journalDigest);
    }
}

contract TestToken is ERC20 {
    uint8 private immutable _dec;

    constructor(string memory n, uint8 d) ERC20(n, n) {
        _dec = d;
    }

    function decimals() public view override returns (uint8) {
        return _dec;
    }

    function mint(address to, uint256 amount) external {
        _mint(to, amount);
    }
}

/// USDT-style: transfer/transferFrom/approve return nothing.
contract NoReturnToken {
    string public name = "Tether-like";
    uint8 public decimals = 6;
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    function mint(address to, uint256 a) external {
        balanceOf[to] += a;
    }

    function approve(address s, uint256 a) external {
        allowance[msg.sender][s] = a;
    }

    function transfer(address to, uint256 a) external {
        balanceOf[msg.sender] -= a;
        balanceOf[to] += a;
    }

    function transferFrom(address f, address to, uint256 a) external {
        allowance[f][msg.sender] -= a;
        balanceOf[f] -= a;
        balanceOf[to] += a;
    }
}

/// Burns 1% on every transfer.
contract FeeOnTransferToken is ERC20 {
    constructor() ERC20("FOT", "FOT") {}

    function mint(address to, uint256 amount) external {
        _mint(to, amount);
    }

    function _update(address from, address to, uint256 value) internal override {
        if (from != address(0) && to != address(0)) {
            uint256 burn = value / 100;
            super._update(from, address(0), burn);
            super._update(from, to, value - burn);
        } else {
            super._update(from, to, value);
        }
    }
}

/// Rejects ETH, to test that a failed ETH payout reverts the release.
contract EthRejecter {
    receive() external payable {
        revert("no");
    }
}
