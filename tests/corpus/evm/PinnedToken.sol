// SPDX-License-Identifier: MIT
pragma solidity 0.8.26;

import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";

/// Authored minimal wrapper over the unmodified OpenZeppelin Contracts 5.1.0
/// production ERC20 implementation. Only compilation is performed.
contract PinnedToken is ERC20 {
    constructor() ERC20("Pinned Corpus Token", "RCT") {}
}
