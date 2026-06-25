// SPDX-License-Identifier: MIT
pragma solidity ^0.8.30;

import "forge-std/Test.sol";
import {ComputeSettlement} from "../src/ComputeSettlement.sol";
import {ComputeProof, ComputeProofLib} from "@luxfi/standard/ai/compute/ComputeProofLib.sol";
import {IComputeVerifier} from "@luxfi/standard/ai/interfaces/IComputeVerifier.sol";
import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";

/// @dev Minimal test-only HANZO. Not production — just enough to escrow/transfer.
contract MockHANZO is IERC20 {
    string public name = "Hanzo";
    string public symbol = "HANZO";
    uint8 public decimals = 18;
    uint256 public totalSupply;
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    function mint(address to, uint256 amount) external {
        balanceOf[to] += amount;
        totalSupply += amount;
        emit Transfer(address(0), to, amount);
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        emit Approval(msg.sender, spender, amount);
        return true;
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        return _move(msg.sender, to, amount);
    }

    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        uint256 a = allowance[from][msg.sender];
        require(a >= amount, "allowance");
        if (a != type(uint256).max) allowance[from][msg.sender] = a - amount;
        return _move(from, to, amount);
    }

    function _move(address from, address to, uint256 amount) internal returns (bool) {
        require(balanceOf[from] >= amount, "balance");
        balanceOf[from] -= amount;
        balanceOf[to] += amount;
        emit Transfer(from, to, amount);
        return true;
    }
}

/// @dev Test-only stand-in for the canonical luxfi/standard ComputeVerifier.
/// Implements the IComputeVerifier.verify signature and returns true iff the
/// proof's reportData equals the expectedReportData the consumer computed — i.e.
/// it simulates a VALID Freivalds binding deterministically, without running the
/// real OptimisticEvidence backend (that is exercised in luxfi/standard's own
/// suite). This isolates the property under test here: that ComputeSettlement
/// correctly COMPUTES expectedReportData from a job's binding and GATES payment
/// on the verifier accepting a proof bound to it. A toggle lets us also prove the
/// gate's fail-closed branch independently of the binding.
contract MockVerifier is IComputeVerifier {
    bool public force; // when set, return forceValue regardless of binding
    bool public forceValue;

    function setForce(bool on, bool value) external {
        force = on;
        forceValue = value;
    }

    function verify(
        ComputeProof calldata proof,
        bytes32 expectedReportData,
        bytes32 /*runtimeMeasurement*/
    ) external view override returns (bool) {
        if (force) return forceValue;
        // Valid iff the proof is bound to exactly this work.
        return proof.reportData == expectedReportData;
    }

    function backendFor(uint8) external pure override returns (address) {
        return address(0);
    }
}

contract ComputeSettlementTest is Test {
    ComputeSettlement internal settlement;
    MockHANZO internal hanzo;
    MockVerifier internal verifier;

    address internal owner = address(this);
    address internal broker = address(0xB0B);
    address internal buyer = address(0xB111);
    address internal provider = address(0x9A0); // == binding.operator

    uint256 internal constant ESCROW = 1_000 ether;

    bytes32 internal constant JOB_ID = keccak256("job-1");

    function setUp() public {
        hanzo = new MockHANZO();
        verifier = new MockVerifier();
        settlement = new ComputeSettlement(address(hanzo), broker, address(verifier));

        hanzo.mint(buyer, ESCROW);
        vm.prank(buyer);
        hanzo.approve(address(settlement), ESCROW);
    }

    // ---- helpers ------------------------------------------------------------

    function _binding() internal view returns (ComputeSettlement.Binding memory b) {
        b = ComputeSettlement.Binding({
            taskId: 42,
            intentID: keccak256("intent"),
            modelSpecHash: keccak256("model-70b-weights-root"),
            promptHash: keccak256("prompt"),
            openBlockHash: blockhash(block.number - 1),
            operator: provider,
            outputHash: keccak256("output"),
            runtimeMeasurement: keccak256("temp=0,kernel=v1")
        });
    }

    /// @dev The canonical reportData a valid proof must carry, computed the SAME
    /// way the contract does (via the one canonical library).
    function _expected(ComputeSettlement.Binding memory b) internal pure returns (bytes32) {
        return
            ComputeProofLib.expectedReportData(
                b.taskId,
                b.intentID,
                b.modelSpecHash,
                b.promptHash,
                b.openBlockHash,
                b.operator,
                b.outputHash,
                b.runtimeMeasurement
            );
    }

    function _proof(bytes32 reportData) internal pure returns (ComputeProof memory) {
        // proofType 3 = optimistic (Freivalds re-exec) — the no-TEE path.
        return ComputeProof({proofType: 3, reportData: reportData, evidence: hex""});
    }

    function _openDefaultJob() internal returns (ComputeSettlement.Binding memory b) {
        b = _binding();
        vm.prank(buyer);
        settlement.openJob(JOB_ID, ESCROW, block.timestamp + 1 days, b);
    }

    // ---- (a) openJob escrows ------------------------------------------------

    function test_OpenJob_EscrowsAndBinds() public {
        ComputeSettlement.Binding memory b = _openDefaultJob();

        assertEq(hanzo.balanceOf(address(settlement)), ESCROW, "escrow held by contract");
        assertEq(hanzo.balanceOf(buyer), 0, "buyer debited");

        ComputeSettlement.Job memory job = settlement.getJob(JOB_ID);
        assertEq(uint8(job.status), uint8(ComputeSettlement.JobStatus.Open), "status Open");
        assertEq(job.buyer, buyer, "buyer recorded");
        assertEq(job.escrow, ESCROW, "escrow recorded");
        assertEq(job.binding.operator, provider, "operator bound");

        // The contract's binding matches the canonical library.
        assertEq(settlement.reportDataOf(JOB_ID), _expected(b), "reportData binding");
    }

    // ---- (b) settle with matching reportData pays operator exactly -----------

    function test_Settle_ValidProof_PaysOperator_MarksSpent() public {
        ComputeSettlement.Binding memory b = _openDefaultJob();
        bytes32 expected = _expected(b);

        vm.prank(broker);
        settlement.settle(JOB_ID, _proof(expected));

        // Operator paid the FULL escrow; contract drained; buyer gets nothing back.
        assertEq(hanzo.balanceOf(provider), ESCROW, "operator paid full escrow");
        assertEq(hanzo.balanceOf(address(settlement)), 0, "contract drained");
        assertEq(hanzo.balanceOf(buyer), 0, "buyer not refunded on settle");

        ComputeSettlement.Job memory job = settlement.getJob(JOB_ID);
        assertEq(uint8(job.status), uint8(ComputeSettlement.JobStatus.Settled), "status Settled");

        assertTrue(settlement.spent(expected), "reportData marked spent");

        (uint256 ok, uint256 bad) = settlement.reputation(provider);
        assertEq(ok, 1, "completed bumped");
        assertEq(bad, 0, "no slash");
    }

    // ---- (c) settle with WRONG reportData reverts, pays nobody ---------------

    function test_Settle_WrongReportData_Reverts_NoPayout() public {
        _openDefaultJob();

        // Proof bound to DIFFERENT work — does not match expectedReportData.
        ComputeProof memory bad = _proof(keccak256("some-other-binding"));

        vm.prank(broker);
        vm.expectRevert(ComputeSettlement.ProofVerificationFailed.selector);
        settlement.settle(JOB_ID, bad);

        // Nobody paid; escrow still held; binding NOT spent; job still Open.
        assertEq(hanzo.balanceOf(provider), 0, "operator not paid");
        assertEq(hanzo.balanceOf(address(settlement)), ESCROW, "escrow retained");
        ComputeSettlement.Job memory job = settlement.getJob(JOB_ID);
        assertEq(uint8(job.status), uint8(ComputeSettlement.JobStatus.Open), "still Open");
    }

    // ---- (d) double-settle reverts (replay guard) ---------------------------

    function test_Settle_DoubleSettle_Reverts() public {
        ComputeSettlement.Binding memory b = _openDefaultJob();
        bytes32 expected = _expected(b);

        vm.prank(broker);
        settlement.settle(JOB_ID, _proof(expected));

        // Second settle of the same job: status is no longer Open.
        vm.prank(broker);
        vm.expectRevert(ComputeSettlement.BadStatus.selector);
        settlement.settle(JOB_ID, _proof(expected));

        assertEq(hanzo.balanceOf(provider), ESCROW, "operator paid exactly once");
    }

    /// @dev Replay guard at the binding level: a *new* jobId for the SAME binding
    /// cannot be reopened once that binding settled (spent-set keys on reportData).
    function test_Replay_SameBinding_CannotReopen() public {
        ComputeSettlement.Binding memory b = _openDefaultJob();
        bytes32 expected = _expected(b);

        vm.prank(broker);
        settlement.settle(JOB_ID, _proof(expected));

        // Buyer funds + approves a second escrow, tries to open a new job that
        // points at the already-settled binding.
        hanzo.mint(buyer, ESCROW);
        vm.prank(buyer);
        hanzo.approve(address(settlement), ESCROW);

        vm.prank(buyer);
        vm.expectRevert(ComputeSettlement.WorkAlreadySpent.selector);
        settlement.openJob(keccak256("job-2"), ESCROW, block.timestamp + 1 days, b);
    }

    // ---- (e) refundExpired after deadline refunds buyer ---------------------

    function test_RefundExpired_RefundsBuyer() public {
        _openDefaultJob();

        vm.warp(block.timestamp + 2 days); // past deadline

        vm.prank(buyer);
        settlement.refundExpired(JOB_ID);

        assertEq(hanzo.balanceOf(buyer), ESCROW, "buyer refunded full escrow");
        assertEq(hanzo.balanceOf(address(settlement)), 0, "contract drained");
        ComputeSettlement.Job memory job = settlement.getJob(JOB_ID);
        assertEq(uint8(job.status), uint8(ComputeSettlement.JobStatus.Refunded), "status Refunded");
    }

    function test_RefundExpired_BeforeDeadline_Reverts() public {
        _openDefaultJob();
        vm.prank(buyer);
        vm.expectRevert(ComputeSettlement.DeadlineNotReached.selector);
        settlement.refundExpired(JOB_ID);
    }

    // ---- (f) slashJob refunds buyer + bumps slashed count -------------------

    function test_SlashJob_RefundsBuyer_BumpsSlashed() public {
        _openDefaultJob();

        vm.prank(broker);
        settlement.slashJob(JOB_ID);

        assertEq(hanzo.balanceOf(buyer), ESCROW, "buyer refunded on slash");
        assertEq(hanzo.balanceOf(provider), 0, "operator not paid");
        ComputeSettlement.Job memory job = settlement.getJob(JOB_ID);
        assertEq(uint8(job.status), uint8(ComputeSettlement.JobStatus.Slashed), "status Slashed");

        (uint256 ok, uint256 bad) = settlement.reputation(provider);
        assertEq(ok, 0, "no completion");
        assertEq(bad, 1, "slashed bumped");

        // A slashed binding is NOT spent — the identical work may be retried.
        assertFalse(settlement.spent(_expected(_binding())), "binding not spent on slash");
    }

    // ---- access control -----------------------------------------------------

    function test_Settle_OnlyBroker() public {
        ComputeSettlement.Binding memory b = _openDefaultJob();
        vm.expectRevert(ComputeSettlement.NotBroker.selector);
        settlement.settle(JOB_ID, _proof(_expected(b)));
    }
}
