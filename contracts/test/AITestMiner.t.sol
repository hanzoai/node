// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import "forge-std/Test.sol";
import "../src/AITestMiner.sol";

contract AITestMinerTest is Test {
    AITestMiner public miner;
    address public user1;
    address public user2;

    event Transfer(address indexed from, address indexed to, uint256 value);
    event Mined(address indexed miner, bytes32 indexed attestationHash, uint256 amount);

    function setUp() public {
        miner = new AITestMiner();
        user1 = address(0x1);
        user2 = address(0x2);
    }

    function test_InitialState() public view {
        assertEq(miner.name(), "AI Test Miner Token");
        assertEq(miner.symbol(), "AITM");
        assertEq(miner.decimals(), 18);
        assertEq(miner.totalSupply(), 0);
        assertEq(miner.TOKENS_PER_MINE(), 100 * 10**18);
    }

    function test_MineWithValidAttestation() public {
        bytes32 attestation = keccak256("test-attestation-1");

        vm.prank(user1);
        vm.expectEmit(true, true, false, true);
        emit Transfer(address(0), user1, 100 * 10**18);
        vm.expectEmit(true, true, false, true);
        emit Mined(user1, attestation, 100 * 10**18);

        bool success = miner.mine(attestation);

        assertTrue(success);
        assertEq(miner.balanceOf(user1), 100 * 10**18);
        assertEq(miner.totalSupply(), 100 * 10**18);
        assertTrue(miner.usedAttestations(attestation));
    }

    function test_MineMultipleAttestations() public {
        bytes32 attestation1 = keccak256("attestation-1");
        bytes32 attestation2 = keccak256("attestation-2");
        bytes32 attestation3 = keccak256("attestation-3");

        vm.startPrank(user1);
        miner.mine(attestation1);
        miner.mine(attestation2);
        miner.mine(attestation3);
        vm.stopPrank();

        assertEq(miner.balanceOf(user1), 300 * 10**18);
        assertEq(miner.totalSupply(), 300 * 10**18);
    }

    function test_DifferentUsersMinesSameAttestation() public {
        bytes32 attestation = keccak256("shared-attestation");

        vm.prank(user1);
        miner.mine(attestation);

        vm.prank(user2);
        vm.expectRevert("Attestation already used");
        miner.mine(attestation);

        assertEq(miner.balanceOf(user1), 100 * 10**18);
        assertEq(miner.balanceOf(user2), 0);
    }

    function test_RevertOnDuplicateAttestation() public {
        bytes32 attestation = keccak256("duplicate-test");

        vm.startPrank(user1);
        miner.mine(attestation);

        vm.expectRevert("Attestation already used");
        miner.mine(attestation);
        vm.stopPrank();
    }

    function test_RevertOnZeroAttestation() public {
        vm.prank(user1);
        vm.expectRevert("Invalid attestation");
        miner.mine(bytes32(0));
    }

    function test_MultipleUsersMining() public {
        bytes32 attestation1 = keccak256("user1-attestation");
        bytes32 attestation2 = keccak256("user2-attestation");

        vm.prank(user1);
        miner.mine(attestation1);

        vm.prank(user2);
        miner.mine(attestation2);

        assertEq(miner.balanceOf(user1), 100 * 10**18);
        assertEq(miner.balanceOf(user2), 100 * 10**18);
        assertEq(miner.totalSupply(), 200 * 10**18);
    }

    function testFuzz_MineWithRandomAttestation(bytes32 attestation) public {
        vm.assume(attestation != bytes32(0));

        vm.prank(user1);
        bool success = miner.mine(attestation);

        assertTrue(success);
        assertEq(miner.balanceOf(user1), 100 * 10**18);
        assertTrue(miner.usedAttestations(attestation));
    }

    function test_AttestationUsedTracking() public {
        bytes32 attestation = keccak256("tracking-test");

        assertFalse(miner.usedAttestations(attestation));

        vm.prank(user1);
        miner.mine(attestation);

        assertTrue(miner.usedAttestations(attestation));
    }
}
