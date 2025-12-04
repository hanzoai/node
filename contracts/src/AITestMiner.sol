// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

contract AITestMiner {
    string public name = "AI Test Miner Token";
    string public symbol = "AITM";
    uint8 public decimals = 18;
    uint256 public totalSupply;

    mapping(address => uint256) public balanceOf;
    mapping(bytes32 => bool) public usedAttestations;

    uint256 public constant TOKENS_PER_MINE = 100 * 10**18;

    event Transfer(address indexed from, address indexed to, uint256 value);
    event Mined(address indexed miner, bytes32 indexed attestationHash, uint256 amount);

    function mine(bytes32 attestationHash) external returns (bool) {
        require(!usedAttestations[attestationHash], "Attestation already used");
        require(attestationHash != bytes32(0), "Invalid attestation");

        usedAttestations[attestationHash] = true;
        balanceOf[msg.sender] += TOKENS_PER_MINE;
        totalSupply += TOKENS_PER_MINE;

        emit Transfer(address(0), msg.sender, TOKENS_PER_MINE);
        emit Mined(msg.sender, attestationHash, TOKENS_PER_MINE);

        return true;
    }
}
