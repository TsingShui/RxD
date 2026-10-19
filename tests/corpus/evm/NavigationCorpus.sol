// SPDX-License-Identifier: MIT
pragma solidity 0.8.26;

/// Trusted authored compiler fixture. This contract is compiled, never executed.
contract NavigationCorpus {
    uint256 private value;

    function addSeven(uint256 x) external pure returns (uint256) {
        unchecked { return x + 7; }
    }

    function choose(uint256 x) external pure returns (uint256 result) {
        unchecked {
            if (x < 10) { result = x + 7; }
            else { result = x - 3; }
        }
    }

    function sum(uint256 n) external pure returns (uint256 result) {
        unchecked {
            for (uint256 i = 0; i < n; ++i) { result += i; }
        }
    }

    function storeAndRead(uint256 x) external returns (uint256) {
        value = x;
        unchecked { return value + 1; }
    }

    function readValue() external view returns (uint256) {
        return value;
    }

    function transientValue(uint256 x) external returns (uint256 result) {
        assembly {
            tstore(0x42, x)
            result := tload(0x42)
        }
    }

    function marker() external pure returns (bytes32) {
        return "RXD_LITERAL_NAVIGATION";
    }

    function message() external pure returns (string memory) {
        return "RXD_DATA_NAVIGATION: trusted authored compiler corpus with a long readable literal and no execution.";
    }
}
