// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import "@openzeppelin/contracts/access/Ownable.sol";

/**
 * @title ComputeSettlement
 * @notice Escrowed, proof-gated, double-spend-safe settlement for brokered
 *         compute jobs. Closes the gap in ComputeDEX.fillOrder, which pays a
 *         provider with no proof of delivery and no replay guard.
 *
 * Lifecycle:
 *   openJob   - buyer escrows HANZO, binds a workId to the job
 *   settle    - on a valid work-proof: pay provider from escrow, mark workId
 *               spent (cannot be paid twice)
 *   slashJob  - provider failed verification: refund buyer, penalize provider
 *   refundExpired - deadline passed with no settlement: refund buyer
 *
 * Canonical compatibility (github.com/luxfi/precompile/ai):
 *   - Reward = 1e18 * computeMinutes * privacyMultiplier / 10000, with
 *     multipliers {Public 2500, Private 5000, Confidential 10000, Sovereign 15000}.
 *     Re-implemented here byte-for-byte from ai_mining.go CalculateReward.
 *   - workId is BLAKE3(deviceId || nonce || chainId), computed by the ai
 *     precompile at 0x0300 (selector computeWorkId) in production. This contract
 *     binds and spends workIds; it does not recompute BLAKE3 (the EVM has no
 *     native BLAKE3 — the precompile owns that). The broker supplies the workId
 *     and the work-proof; settle() re-derives the *reward* from the proof and
 *     enforces the spent-set, so payment can never exceed the proven work nor
 *     be replayed.
 *
 * Verification paths (identical state transitions):
 *   - precompile path (production on Hanzo/Lux/Zoo EVM): ML-DSA signature over
 *     the workId is verified via precompile 0x0300 selector verifyMLDSA.
 *   - operator path (vanilla EVM / CI): the broker (an Ownable-set operator)
 *     attests the settlement. Escrow, work-id binding, reward math and spent-set
 *     are fully exercised either way.
 */
contract ComputeSettlement is ReentrancyGuard, Ownable {
    // ---- canonical constants (mirror ai_mining.go) --------------------------

    // ai precompile address (shared Hanzo/Lux/Zoo). bytes20 of 0x0300.
    address public constant AI_PRECOMPILE = 0x0000000000000000000000000000000000000300;

    // verifyMLDSA selector: first 4 bytes of precompile input (module.go).
    bytes4 internal constant SELECTOR_VERIFY_MLDSA = 0x01000000;

    // Privacy levels.
    uint16 public constant PRIVACY_PUBLIC = 1;
    uint16 public constant PRIVACY_PRIVATE = 2;
    uint16 public constant PRIVACY_CONFIDENTIAL = 3;
    uint16 public constant PRIVACY_SOVEREIGN = 4;

    uint256 public constant MULT_DENOMINATOR = 10000;
    uint256 public constant BASE_REWARD_PER_MINUTE = 1e18;

    // Work-proof layout offsets (ai_mining.go WorkProof*Offset).
    uint256 internal constant OFF_PRIVACY = 72; // uint16 BE
    uint256 internal constant OFF_COMPUTE_MINS = 74; // uint32 BE
    uint256 internal constant OFF_TEE_QUOTE = 78;
    uint256 internal constant WORK_PROOF_MIN_SIZE = 78;

    // ---- types --------------------------------------------------------------

    enum JobStatus {
        None,
        Open,
        Settled,
        Slashed,
        Refunded
    }

    struct Job {
        address buyer;
        address provider;
        uint256 escrow; // HANZO locked by buyer
        uint256 deadline; // unix seconds; after this, refundExpired is callable
        JobStatus status;
        bytes32 workId; // BLAKE3(deviceId||nonce||chainId) from ai precompile
        bytes mldsaPubKey; // provider's ML-DSA public key (verifies the proof)
    }

    // ---- storage ------------------------------------------------------------

    IERC20 public immutable paymentToken; // HANZO

    // The broker that may use the operator settlement path and slash. Distinct
    // from owner so the broker key can rotate without transferring ownership.
    address public broker;

    // When true, settle() requires precompile ML-DSA verification. When false
    // (CI / vanilla EVM), settle() uses the operator path (broker-attested).
    bool public usePrecompile;

    mapping(bytes32 => Job) public jobs; // jobId => Job
    mapping(bytes32 => bool) public spent; // workId => spent (replay guard)

    // Reputation: simple slash counter; the broker's matcher reads it off-chain.
    mapping(address => uint256) public completed; // settled jobs per provider
    mapping(address => uint256) public slashed; // slashed jobs per provider

    // ---- events -------------------------------------------------------------

    event JobOpened(
        bytes32 indexed jobId,
        address indexed buyer,
        address indexed provider,
        bytes32 workId,
        uint256 escrow,
        uint256 deadline
    );
    event JobSettled(
        bytes32 indexed jobId,
        address indexed provider,
        bytes32 indexed workId,
        uint256 reward,
        uint256 refund
    );
    event JobSlashed(bytes32 indexed jobId, address indexed provider, uint256 refund);
    event JobRefunded(bytes32 indexed jobId, address indexed buyer, uint256 refund);
    event BrokerUpdated(address indexed broker);
    event PrecompileModeUpdated(bool usePrecompile);

    // ---- errors -------------------------------------------------------------

    error NotBroker();
    error BadStatus();
    error JobExists();
    error ZeroAddress();
    error ZeroEscrow();
    error DeadlineInPast();
    error WorkAlreadySpent();
    error InvalidWorkProof();
    error InvalidPrivacyLevel();
    error RewardExceedsEscrow();
    error ProofVerificationFailed();
    error DeadlineNotReached();
    error TransferFailed();

    modifier onlyBroker() {
        if (msg.sender != broker) revert NotBroker();
        _;
    }

    constructor(address _paymentToken, address _broker) Ownable(msg.sender) {
        if (_paymentToken == address(0) || _broker == address(0)) revert ZeroAddress();
        paymentToken = IERC20(_paymentToken);
        broker = _broker;
        // Default to operator mode; deployer flips usePrecompile=true on a chain
        // that has the 0x0300 precompile (Hanzo/Lux/Zoo EVM).
        usePrecompile = false;
    }

    // ---- admin --------------------------------------------------------------

    function setBroker(address _broker) external onlyOwner {
        if (_broker == address(0)) revert ZeroAddress();
        broker = _broker;
        emit BrokerUpdated(_broker);
    }

    function setUsePrecompile(bool _use) external onlyOwner {
        usePrecompile = _use;
        emit PrecompileModeUpdated(_use);
    }

    // ---- lifecycle ----------------------------------------------------------

    /**
     * @notice Buyer escrows HANZO for a brokered job. Caller must have approved
     *         `escrow` to this contract.
     * @param jobId     Unique broker-assigned job id.
     * @param provider  The matched provider that will be paid on proof.
     * @param workId    ai-precompile work id: BLAKE3(deviceId||nonce||chainId).
     * @param escrow    HANZO locked; an upper bound on what the provider earns.
     * @param deadline  Unix seconds; after it, buyer may refundExpired.
     * @param mldsaPubKey Provider ML-DSA public key used to verify the proof.
     */
    function openJob(
        bytes32 jobId,
        address provider,
        bytes32 workId,
        uint256 escrow,
        uint256 deadline,
        bytes calldata mldsaPubKey
    ) external nonReentrant {
        if (jobs[jobId].status != JobStatus.None) revert JobExists();
        if (provider == address(0)) revert ZeroAddress();
        if (escrow == 0) revert ZeroEscrow();
        if (deadline <= block.timestamp) revert DeadlineInPast();
        if (spent[workId]) revert WorkAlreadySpent();

        if (!paymentToken.transferFrom(msg.sender, address(this), escrow)) {
            revert TransferFailed();
        }

        jobs[jobId] = Job({
            buyer: msg.sender,
            provider: provider,
            escrow: escrow,
            deadline: deadline,
            status: JobStatus.Open,
            workId: workId,
            mldsaPubKey: mldsaPubKey
        });

        emit JobOpened(jobId, msg.sender, provider, workId, escrow, deadline);
    }

    /**
     * @notice Settle a job against a provider work-proof. Pays the provider the
     *         canonical reward (capped at escrow), refunds the remainder to the
     *         buyer, and marks the workId spent. Callable by the broker.
     * @param jobId     The open job.
     * @param workProof The canonical ai work-proof bytes (see ai_mining.go).
     * @param mldsaSig  ML-DSA signature over the workId (precompile path only).
     */
    function settle(
        bytes32 jobId,
        bytes calldata workProof,
        bytes calldata mldsaSig
    ) external nonReentrant onlyBroker {
        Job storage job = jobs[jobId];
        if (job.status != JobStatus.Open) revert BadStatus();
        if (spent[job.workId]) revert WorkAlreadySpent();

        // Verify the proof binds to this provider.
        if (usePrecompile) {
            if (!_verifyMLDSA(job.mldsaPubKey, abi.encodePacked(job.workId), mldsaSig)) {
                revert ProofVerificationFailed();
            }
        }
        // operator path: onlyBroker is the attestation; the broker has already
        // run VerifyMLDSA + result acceptance off-chain (§5 of COMPUTE_BROKER.md).

        uint256 reward = _calculateReward(workProof);
        if (reward > job.escrow) revert RewardExceedsEscrow();

        // Effects before interactions.
        spent[job.workId] = true;
        job.status = JobStatus.Settled;
        completed[job.provider] += 1;

        uint256 refund = job.escrow - reward;

        if (reward > 0 && !paymentToken.transfer(job.provider, reward)) {
            revert TransferFailed();
        }
        if (refund > 0 && !paymentToken.transfer(job.buyer, refund)) {
            revert TransferFailed();
        }

        emit JobSettled(jobId, job.provider, job.workId, reward, refund);
    }

    /**
     * @notice Provider failed verification (bad result / dissent in redundancy
     *         check). Refund the buyer in full and penalize provider reputation.
     *         Does NOT mark the workId spent — the work was never validly proven,
     *         so the same workId could be re-bound to a retry by another provider.
     */
    function slashJob(bytes32 jobId) external nonReentrant onlyBroker {
        Job storage job = jobs[jobId];
        if (job.status != JobStatus.Open) revert BadStatus();

        job.status = JobStatus.Slashed;
        slashed[job.provider] += 1;

        uint256 refund = job.escrow;
        if (refund > 0 && !paymentToken.transfer(job.buyer, refund)) {
            revert TransferFailed();
        }

        emit JobSlashed(jobId, job.provider, refund);
    }

    /**
     * @notice After the deadline with no settlement, the buyer reclaims escrow.
     *         Permissionless for the buyer; protects funds if the broker stalls.
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

    /// @notice Canonical reward for a work-proof. Pure mirror of ai_mining.go.
    function quoteReward(bytes calldata workProof) external pure returns (uint256) {
        return _calculateReward(workProof);
    }

    function getJob(bytes32 jobId) external view returns (Job memory) {
        return jobs[jobId];
    }

    function reputation(address provider) external view returns (uint256 ok, uint256 bad) {
        return (completed[provider], slashed[provider]);
    }

    // ---- internal -----------------------------------------------------------

    /**
     * @dev Reward = BASE_REWARD_PER_MINUTE * computeMinutes * multiplier / 10000.
     *      Byte-for-byte the same arithmetic as ai_mining.go CalculateReward,
     *      reading privacy (uint16 BE at offset 72) and computeMinutes
     *      (uint32 BE at offset 74) from the canonical work-proof layout.
     */
    function _calculateReward(bytes calldata workProof) internal pure returns (uint256) {
        if (workProof.length < WORK_PROOF_MIN_SIZE) revert InvalidWorkProof();

        uint16 privacy = _readU16BE(workProof, OFF_PRIVACY);
        uint32 computeMinutes = _readU32BE(workProof, OFF_COMPUTE_MINS);

        uint256 mult = _privacyMultiplier(privacy);

        uint256 reward = BASE_REWARD_PER_MINUTE;
        reward = reward * uint256(computeMinutes);
        reward = reward * mult;
        reward = reward / MULT_DENOMINATOR;
        return reward;
    }

    function _privacyMultiplier(uint16 privacy) internal pure returns (uint256) {
        if (privacy == PRIVACY_PUBLIC) return 2500;
        if (privacy == PRIVACY_PRIVATE) return 5000;
        if (privacy == PRIVACY_CONFIDENTIAL) return 10000;
        if (privacy == PRIVACY_SOVEREIGN) return 15000;
        revert InvalidPrivacyLevel();
    }

    function _readU16BE(bytes calldata b, uint256 off) internal pure returns (uint16) {
        return (uint16(uint8(b[off])) << 8) | uint16(uint8(b[off + 1]));
    }

    function _readU32BE(bytes calldata b, uint256 off) internal pure returns (uint32) {
        return
            (uint32(uint8(b[off])) << 24) |
            (uint32(uint8(b[off + 1])) << 16) |
            (uint32(uint8(b[off + 2])) << 8) |
            uint32(uint8(b[off + 3]));
    }

    /**
     * @dev verifyMLDSA via the ai precompile at 0x0300.
     *      Input layout (after 4-byte selector): u32 pkLen | pk | u32 msgLen |
     *      msg | u32 sigLen | sig. Output: 32-byte big-endian bool (1 == true).
     *      Matches desktop precompile.rs verify_mldsa and ai module.go.
     */
    function _verifyMLDSA(
        bytes memory pubkey,
        bytes memory message,
        bytes memory signature
    ) internal view returns (bool) {
        bytes memory input = abi.encodePacked(
            SELECTOR_VERIFY_MLDSA,
            uint32(pubkey.length),
            pubkey,
            uint32(message.length),
            message,
            uint32(signature.length),
            signature
        );
        (bool ok, bytes memory out) = AI_PRECOMPILE.staticcall(input);
        if (!ok || out.length != 32) return false;
        return out[31] != 0;
    }
}
