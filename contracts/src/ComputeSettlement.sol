// SPDX-License-Identifier: MIT
pragma solidity ^0.8.30;

import "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import "@openzeppelin/contracts/access/Ownable.sol";

import {ComputeProof, ComputeProofLib} from "@luxfi/standard/ai/compute/ComputeProofLib.sol";
import {IComputeVerifier} from "@luxfi/standard/ai/interfaces/IComputeVerifier.sol";

/**
 * @title ComputeSettlement
 * @notice Escrowed, proof-gated, double-spend-safe settlement for brokered
 *         compute jobs. Closes the gap in ComputeDEX.fillOrder, which pays a
 *         provider with no proof of delivery and no replay guard.
 *
 * Lifecycle:
 *   openJob       - buyer escrows HANZO, binds the canonical compute-proof
 *                   fields (task/model/prompt/output/runtime/operator) to the job
 *   settle        - on a valid CANONICAL compute proof: pay the provider the
 *                   escrowed amount, mark the binding (reportData) spent so the
 *                   same computation can never settle twice (replay guard)
 *   slashJob      - provider failed verification: refund buyer, penalize provider
 *   refundExpired - deadline passed with no settlement: refund buyer
 *
 * Proof gate (LP-302, the law):
 *   There is ONE canonical compute-proof implementation: luxfi/standard
 *   contracts/ai, consumed via the luxfi-standard/ai package. This contract
 *   does NOT implement a second proof system. It computes the canonical binding
 *   `reportData = ComputeProofLib.expectedReportData(...)` over the job's bound
 *   fields and requires the canonical `IComputeVerifier.verify(proof, expected,
 *   runtimeMeasurement) == true` before paying. The proof is a recomputable
 *   Freivalds side-effect (proofType 3 = OptimisticEvidence); there is NO TEE,
 *   NO GPU attestation. Pricing (HMM) happens OFF-chain in the broker and
 *   arrives as the escrow amount — there is no competing on-chain reward formula.
 *
 * Replay guard:
 *   The spent-set keys on the canonical `reportData` binding. A given
 *   (taskId, intentID, modelSpecHash, promptHash, openBlockHash, operator,
 *   outputHash, runtimeMeasurement) collapses to one 32-byte reportData; it
 *   settles exactly once. A failed/slashed job does NOT mark it spent, so the
 *   identical work may be re-bound to a retry.
 */
contract ComputeSettlement is ReentrancyGuard, Ownable {
    using ComputeProofLib for *;

    // ---- types --------------------------------------------------------------

    enum JobStatus {
        None,
        Open,
        Settled,
        Slashed,
        Refunded
    }

    /// @notice The canonical compute-proof binding for a job. These are exactly
    ///         the inputs to ComputeProofLib.expectedReportData — the proof must
    ///         bind to this tuple or settle() reverts. `operator` is the provider
    ///         that produced the output and is paid on a valid proof.
    struct Binding {
        uint256 taskId;
        bytes32 intentID;
        bytes32 modelSpecHash;
        bytes32 promptHash;
        bytes32 openBlockHash;
        address operator; // provider; the address paid on a valid proof
        bytes32 outputHash;
        bytes32 runtimeMeasurement; // runtime+sampler measurement (e.g. temp==0)
    }

    struct Job {
        address buyer;
        uint256 escrow; // HANZO locked by buyer; paid in full to operator on proof
        uint256 deadline; // unix seconds; after this, refundExpired is callable
        JobStatus status;
        Binding binding;
    }

    // ---- storage ------------------------------------------------------------

    IERC20 public immutable paymentToken; // HANZO

    /// @notice The canonical compute-proof gate (luxfi/standard ComputeVerifier).
    ///         Owner-set so the verifier deployment can be wired/rotated without
    ///         redeploying settlement; the proof system itself is canon.
    IComputeVerifier public verifier;

    // The broker that may settle and slash. Distinct from owner so the broker
    // key can rotate without transferring ownership.
    address public broker;

    mapping(bytes32 => Job) public jobs; // jobId => Job
    mapping(bytes32 => bool) public spent; // reportData => spent (replay guard)

    // Reputation: simple counters; the broker's matcher reads them off-chain.
    mapping(address => uint256) public completed; // settled jobs per provider
    mapping(address => uint256) public slashed; // slashed jobs per provider

    // ---- events -------------------------------------------------------------

    event JobOpened(
        bytes32 indexed jobId,
        address indexed buyer,
        address indexed operator,
        uint256 escrow,
        uint256 deadline
    );
    event JobSettled(
        bytes32 indexed jobId,
        address indexed operator,
        bytes32 indexed reportData,
        uint256 paid
    );
    event JobSlashed(bytes32 indexed jobId, address indexed operator, uint256 refund);
    event JobRefunded(bytes32 indexed jobId, address indexed buyer, uint256 refund);
    event BrokerUpdated(address indexed broker);
    event VerifierUpdated(address indexed verifier);

    // ---- errors -------------------------------------------------------------

    error NotBroker();
    error BadStatus();
    error JobExists();
    error ZeroAddress();
    error ZeroEscrow();
    error DeadlineInPast();
    error WorkAlreadySpent();
    error ProofVerificationFailed();
    error DeadlineNotReached();
    error TransferFailed();

    modifier onlyBroker() {
        if (msg.sender != broker) revert NotBroker();
        _;
    }

    constructor(address _paymentToken, address _broker, address _verifier) Ownable(msg.sender) {
        if (_paymentToken == address(0) || _broker == address(0) || _verifier == address(0)) {
            revert ZeroAddress();
        }
        paymentToken = IERC20(_paymentToken);
        broker = _broker;
        verifier = IComputeVerifier(_verifier);
    }

    // ---- admin --------------------------------------------------------------

    function setBroker(address _broker) external onlyOwner {
        if (_broker == address(0)) revert ZeroAddress();
        broker = _broker;
        emit BrokerUpdated(_broker);
    }

    function setVerifier(address _verifier) external onlyOwner {
        if (_verifier == address(0)) revert ZeroAddress();
        verifier = IComputeVerifier(_verifier);
        emit VerifierUpdated(_verifier);
    }

    // ---- lifecycle ----------------------------------------------------------

    /**
     * @notice Buyer escrows HANZO for a brokered job and binds the canonical
     *         compute-proof fields. Caller must have approved `escrow` to this
     *         contract. The full escrow is paid to `binding.operator` when a
     *         valid canonical proof for this binding is presented to settle().
     * @param jobId    Unique broker-assigned job id.
     * @param escrow   HANZO locked; paid in full to the operator on a valid proof
     *                 (the HMM price was already computed off-chain by the broker).
     * @param deadline Unix seconds; after it, buyer may refundExpired.
     * @param binding  The canonical proof binding (task/model/prompt/output/
     *                 runtime/operator) the settlement proof must reproduce.
     */
    function openJob(
        bytes32 jobId,
        uint256 escrow,
        uint256 deadline,
        Binding calldata binding
    ) external nonReentrant {
        if (jobs[jobId].status != JobStatus.None) revert JobExists();
        if (binding.operator == address(0)) revert ZeroAddress();
        if (escrow == 0) revert ZeroEscrow();
        if (deadline <= block.timestamp) revert DeadlineInPast();

        // A binding already settled cannot be reopened for a second payout.
        if (spent[_reportData(binding)]) revert WorkAlreadySpent();

        if (!paymentToken.transferFrom(msg.sender, address(this), escrow)) {
            revert TransferFailed();
        }

        jobs[jobId] = Job({
            buyer: msg.sender,
            escrow: escrow,
            deadline: deadline,
            status: JobStatus.Open,
            binding: binding
        });

        emit JobOpened(jobId, msg.sender, binding.operator, escrow, deadline);
    }

    /**
     * @notice Settle a job against a CANONICAL compute proof. Recomputes the
     *         canonical binding from the job's bound fields, requires the
     *         canonical verifier to accept the proof against that binding, then
     *         pays the full escrow to the operator and marks the binding spent.
     *         Callable by the broker.
     * @param jobId The open job.
     * @param proof The canonical ComputeProof (proofType 3 = Freivalds re-exec).
     *              Its reportData must equal expectedReportData(job.binding).
     */
    function settle(bytes32 jobId, ComputeProof calldata proof) external nonReentrant onlyBroker {
        Job storage job = jobs[jobId];
        if (job.status != JobStatus.Open) revert BadStatus();

        bytes32 reportData = _reportData(job.binding);
        if (spent[reportData]) revert WorkAlreadySpent();

        // THE canonical gate: binding + runtime policy + evidence backend.
        // No proof / wrong binding / unaccepted runtime → false → no payout.
        if (!verifier.verify(proof, reportData, job.binding.runtimeMeasurement)) {
            revert ProofVerificationFailed();
        }

        address operator = job.binding.operator;
        uint256 amount = job.escrow;

        // Effects before interactions.
        spent[reportData] = true;
        job.status = JobStatus.Settled;
        completed[operator] += 1;

        if (!paymentToken.transfer(operator, amount)) revert TransferFailed();

        emit JobSettled(jobId, operator, reportData, amount);
    }

    /**
     * @notice Provider failed verification (bad result / dissent in a redundancy
     *         check). Refund the buyer in full and penalize provider reputation.
     *         Does NOT mark the binding spent — the work was never validly proven,
     *         so the identical computation may be re-bound to a retry.
     */
    function slashJob(bytes32 jobId) external nonReentrant onlyBroker {
        Job storage job = jobs[jobId];
        if (job.status != JobStatus.Open) revert BadStatus();

        job.status = JobStatus.Slashed;
        address operator = job.binding.operator;
        slashed[operator] += 1;

        uint256 refund = job.escrow;
        if (refund > 0 && !paymentToken.transfer(job.buyer, refund)) {
            revert TransferFailed();
        }

        emit JobSlashed(jobId, operator, refund);
    }

    /**
     * @notice After the deadline with no settlement, the buyer reclaims escrow.
     *         Callable by the buyer or the broker; protects funds if settlement
     *         stalls.
     */
    function refundExpired(bytes32 jobId) external nonReentrant {
        Job storage job = jobs[jobId];
        if (job.status != JobStatus.Open) revert BadStatus();
        if (block.timestamp < job.deadline) revert DeadlineNotReached();
        if (msg.sender != job.buyer && msg.sender != broker) revert NotBroker();

        job.status = JobStatus.Refunded;
        uint256 refund = job.escrow;
        if (refund > 0 && !paymentToken.transfer(job.buyer, refund)) {
            revert TransferFailed();
        }

        emit JobRefunded(jobId, job.buyer, refund);
    }

    // ---- views --------------------------------------------------------------

    function getJob(bytes32 jobId) external view returns (Job memory) {
        return jobs[jobId];
    }

    /// @notice The canonical reportData binding for a job's fields — the value a
    ///         settlement proof must carry, and the replay-guard key.
    function reportDataOf(bytes32 jobId) external view returns (bytes32) {
        return _reportData(jobs[jobId].binding);
    }

    /// @notice The canonical reportData binding for an arbitrary binding tuple.
    ///         Pure mirror of ComputeProofLib.expectedReportData; lets the broker
    ///         precompute the value it must put in the proof.
    function expectedReportData(Binding calldata binding) external pure returns (bytes32) {
        return _reportData(binding);
    }

    function reputation(address provider) external view returns (uint256 ok, uint256 bad) {
        return (completed[provider], slashed[provider]);
    }

    // ---- internal -----------------------------------------------------------

    /// @dev The canonical binding via the one canonical library — no second
    ///      implementation. Identical to the off-chain Go/Rust derivation.
    function _reportData(Binding memory b) internal pure returns (bytes32) {
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
}
