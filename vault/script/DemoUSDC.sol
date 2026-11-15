// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity ^0.8.24;

import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";

/// DEMO ONLY (Sepolia): a 6-decimal stand-in for USDC with no value. Only the deployer can
/// mint (unlike test/mocks TestToken, whose mint is open to anyone).
contract DemoUSDC is ERC20 {
    address public immutable minter;

    error NotMinter();

    constructor() ERC20("RFP-022 Demo USDC (no value)", "dUSDC") {
        minter = msg.sender;
    }

    function decimals() public pure override returns (uint8) {
        return 6;
    }

    function mint(address to, uint256 amount) external {
        if (msg.sender != minter) revert NotMinter();
        _mint(to, amount);
    }
}
