// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import "@openzeppelin/contracts/security/ReentrancyGuard.sol";
import "@openzeppelin/contracts/access/Ownable.sol";
import "@openzeppelin/contracts/utils/math/Math.sol";

/**
 * @title ComputeDEX
 * @dev Decentralized Exchange for Compute Resources with AMM
 *
 * This contract enables:
 * - Trading compute resources (CPU, GPU, Memory, Storage)
 * - Automated Market Making for compute pricing
 * - Liquidity provision for compute markets
 * - Dynamic pricing based on supply/demand
 */
contract ComputeDEX is ReentrancyGuard, Ownable {
    using Math for uint256;

    // Compute resource types
    enum ResourceType {
        CPU,        // CPU cores/hours
        GPU,        // GPU compute units
        MEMORY,     // RAM GB/hours
        STORAGE,    // Storage GB/months
        BANDWIDTH,  // Network bandwidth Mbps
        WASM,       // WASM execution units
        DOCKER,     // Docker container hours
        K8S         // Kubernetes pod hours
    }

    // Resource pool for AMM
    struct ResourcePool {
        uint256 resourceAmount;     // Amount of compute resource
        uint256 tokenAmount;         // Amount of payment tokens (HANZO)
        uint256 totalLiquidity;      // Total liquidity shares
        uint256 feeRate;             // Trading fee (basis points)
        mapping(address => uint256) liquidity;  // LP shares per user
        mapping(address => uint256) resourceBalance; // User's resource contribution
    }

    // Order for compute resources
    struct ComputeOrder {
        address provider;
        ResourceType resourceType;
        uint256 amount;
        uint256 pricePerUnit;
        uint256 duration;
        uint256 slaScore;          // Service Level Agreement score (0-100)
        bool isActive;
        bytes32 attestation;        // TEE attestation hash
    }

    // Resource utilization tracking
    struct ResourceMetrics {
        uint256 totalSupply;
        uint256 totalDemand;
        uint256 avgPrice;
        uint256 utilizationRate;    // Percentage * 100
        uint256 lastUpdate;
    }

    // State variables
    IERC20 public immutable paymentToken;  // HANZO token for payments

    mapping(ResourceType => ResourcePool) public resourcePools;
    mapping(ResourceType => ResourceMetrics) public resourceMetrics;
    mapping(bytes32 => ComputeOrder) public computeOrders;
    mapping(address => mapping(ResourceType => uint256)) public userResources;

    uint256 public constant MIN_LIQUIDITY = 1000;
    uint256 public constant FEE_DENOMINATOR = 10000;
    uint256 public defaultFeeRate = 30; // 0.3%

    // Events
    event LiquidityAdded(
        address indexed provider,
        ResourceType indexed resourceType,
        uint256 resourceAmount,
        uint256 tokenAmount,
        uint256 liquidityMinted
    );

    event LiquidityRemoved(
        address indexed provider,
        ResourceType indexed resourceType,
        uint256 resourceAmount,
        uint256 tokenAmount,
        uint256 liquidityBurned
    );

    event ComputeSwap(
        address indexed user,
        ResourceType indexed resourceType,
        bool isBuyingResource,
        uint256 amountIn,
        uint256 amountOut
    );

    event OrderCreated(
        bytes32 indexed orderId,
        address indexed provider,
        ResourceType resourceType,
        uint256 amount,
        uint256 pricePerUnit
    );

    event OrderFulfilled(
        bytes32 indexed orderId,
        address indexed consumer,
        uint256 amount
    );

    constructor(address _paymentToken) Ownable(msg.sender) {
        paymentToken = IERC20(_paymentToken);

        // Initialize pools
        for (uint8 i = 0; i <= uint8(ResourceType.K8S); i++) {
            resourcePools[ResourceType(i)].feeRate = defaultFeeRate;
        }
    }

    /**
     * @dev Add liquidity to a resource pool
     * @param resourceType Type of compute resource
     * @param resourceAmount Amount of compute resources to add
     * @param tokenAmount Amount of HANZO tokens to add
     */
    function addLiquidity(
        ResourceType resourceType,
        uint256 resourceAmount,
        uint256 tokenAmount
    ) external nonReentrant returns (uint256 liquidity) {
        require(resourceAmount > 0 && tokenAmount > 0, "Invalid amounts");

        ResourcePool storage pool = resourcePools[resourceType];

        // Transfer tokens from user
        require(
            paymentToken.transferFrom(msg.sender, address(this), tokenAmount),
            "Token transfer failed"
        );

        if (pool.totalLiquidity == 0) {
            // First liquidity provider
            liquidity = Math.sqrt(resourceAmount * tokenAmount) - MIN_LIQUIDITY;
            pool.totalLiquidity = MIN_LIQUIDITY; // Lock minimum liquidity
        } else {
            // Calculate proportional liquidity
            uint256 liquidityResource = (resourceAmount * pool.totalLiquidity) / pool.resourceAmount;
            uint256 liquidityToken = (tokenAmount * pool.totalLiquidity) / pool.tokenAmount;
            liquidity = Math.min(liquidityResource, liquidityToken);
        }

        require(liquidity > 0, "Insufficient liquidity minted");

        // Update pool state
        pool.resourceAmount += resourceAmount;
        pool.tokenAmount += tokenAmount;
        pool.totalLiquidity += liquidity;
        pool.liquidity[msg.sender] += liquidity;
        pool.resourceBalance[msg.sender] += resourceAmount;

        // Update metrics
        _updateMetrics(resourceType, resourceAmount, 0);

        emit LiquidityAdded(
            msg.sender,
            resourceType,
            resourceAmount,
            tokenAmount,
            liquidity
        );
    }

    /**
     * @dev Remove liquidity from a resource pool
     * @param resourceType Type of compute resource
     * @param liquidity Amount of liquidity shares to burn
     */
    function removeLiquidity(
        ResourceType resourceType,
        uint256 liquidity
    ) external nonReentrant returns (uint256 resourceAmount, uint256 tokenAmount) {
        ResourcePool storage pool = resourcePools[resourceType];
        require(pool.liquidity[msg.sender] >= liquidity, "Insufficient liquidity");

        // Calculate proportional amounts
        resourceAmount = (liquidity * pool.resourceAmount) / pool.totalLiquidity;
        tokenAmount = (liquidity * pool.tokenAmount) / pool.totalLiquidity;

        // Update pool state
        pool.resourceAmount -= resourceAmount;
        pool.tokenAmount -= tokenAmount;
        pool.totalLiquidity -= liquidity;
        pool.liquidity[msg.sender] -= liquidity;
        pool.resourceBalance[msg.sender] -= resourceAmount;

        // Transfer tokens back to user
        require(
            paymentToken.transfer(msg.sender, tokenAmount),
            "Token transfer failed"
        );

        // Update user's resource balance
        userResources[msg.sender][resourceType] += resourceAmount;

        emit LiquidityRemoved(
            msg.sender,
            resourceType,
            resourceAmount,
            tokenAmount,
            liquidity
        );
    }

    /**
     * @dev Buy compute resources with tokens (AMM swap)
     * @param resourceType Type of compute resource
     * @param tokenAmountIn Amount of tokens to spend
     * @param minResourceOut Minimum resources expected (slippage protection)
     */
    function buyResources(
        ResourceType resourceType,
        uint256 tokenAmountIn,
        uint256 minResourceOut
    ) external nonReentrant returns (uint256 resourceOut) {
        require(tokenAmountIn > 0, "Invalid input amount");

        ResourcePool storage pool = resourcePools[resourceType];
        require(pool.resourceAmount > 0 && pool.tokenAmount > 0, "Empty pool");

        // Calculate output with fee
        uint256 amountInWithFee = tokenAmountIn * (FEE_DENOMINATOR - pool.feeRate);
        uint256 numerator = amountInWithFee * pool.resourceAmount;
        uint256 denominator = (pool.tokenAmount * FEE_DENOMINATOR) + amountInWithFee;
        resourceOut = numerator / denominator;

        require(resourceOut >= minResourceOut, "Insufficient output");
        require(resourceOut < pool.resourceAmount, "Insufficient liquidity");

        // Transfer tokens from user
        require(
            paymentToken.transferFrom(msg.sender, address(this), tokenAmountIn),
            "Token transfer failed"
        );

        // Update pool state
        pool.tokenAmount += tokenAmountIn;
        pool.resourceAmount -= resourceOut;

        // Credit user with resources
        userResources[msg.sender][resourceType] += resourceOut;

        // Update metrics
        _updateMetrics(resourceType, 0, resourceOut);

        emit ComputeSwap(
            msg.sender,
            resourceType,
            true,
            tokenAmountIn,
            resourceOut
        );
    }

    /**
     * @dev Sell compute resources for tokens (AMM swap)
     * @param resourceType Type of compute resource
     * @param resourceAmountIn Amount of resources to sell
     * @param minTokenOut Minimum tokens expected (slippage protection)
     */
    function sellResources(
        ResourceType resourceType,
        uint256 resourceAmountIn,
        uint256 minTokenOut
    ) external nonReentrant returns (uint256 tokenOut) {
        require(resourceAmountIn > 0, "Invalid input amount");
        require(
            userResources[msg.sender][resourceType] >= resourceAmountIn,
            "Insufficient resources"
        );

        ResourcePool storage pool = resourcePools[resourceType];
        require(pool.resourceAmount > 0 && pool.tokenAmount > 0, "Empty pool");

        // Calculate output with fee
        uint256 amountInWithFee = resourceAmountIn * (FEE_DENOMINATOR - pool.feeRate);
        uint256 numerator = amountInWithFee * pool.tokenAmount;
        uint256 denominator = (pool.resourceAmount * FEE_DENOMINATOR) + amountInWithFee;
        tokenOut = numerator / denominator;

        require(tokenOut >= minTokenOut, "Insufficient output");
        require(tokenOut < pool.tokenAmount, "Insufficient liquidity");

        // Update user's resource balance
        userResources[msg.sender][resourceType] -= resourceAmountIn;

        // Update pool state
        pool.resourceAmount += resourceAmountIn;
        pool.tokenAmount -= tokenOut;

        // Transfer tokens to user
        require(
            paymentToken.transfer(msg.sender, tokenOut),
            "Token transfer failed"
        );

        // Update metrics
        _updateMetrics(resourceType, resourceAmountIn, 0);

        emit ComputeSwap(
            msg.sender,
            resourceType,
            false,
            resourceAmountIn,
            tokenOut
        );
    }

    /**
     * @dev Create a compute order (limit order)
     * @param resourceType Type of compute resource
     * @param amount Amount of resources
     * @param pricePerUnit Price per resource unit
     * @param duration Duration of the offer
     * @param slaScore Service level agreement score
     * @param attestation TEE attestation hash
     */
    function createOrder(
        ResourceType resourceType,
        uint256 amount,
        uint256 pricePerUnit,
        uint256 duration,
        uint256 slaScore,
        bytes32 attestation
    ) external returns (bytes32 orderId) {
        require(amount > 0 && pricePerUnit > 0, "Invalid parameters");
        require(slaScore <= 100, "Invalid SLA score");
        require(
            userResources[msg.sender][resourceType] >= amount,
            "Insufficient resources"
        );

        orderId = keccak256(
            abi.encodePacked(
                msg.sender,
                resourceType,
                amount,
                pricePerUnit,
                block.timestamp
            )
        );

        computeOrders[orderId] = ComputeOrder({
            provider: msg.sender,
            resourceType: resourceType,
            amount: amount,
            pricePerUnit: pricePerUnit,
            duration: duration,
            slaScore: slaScore,
            isActive: true,
            attestation: attestation
        });

        // Lock resources
        userResources[msg.sender][resourceType] -= amount;

        emit OrderCreated(
            orderId,
            msg.sender,
            resourceType,
            amount,
            pricePerUnit
        );
    }

    /**
     * @dev Fill a compute order
     * @param orderId Order identifier
     * @param amount Amount to fill
     */
    function fillOrder(bytes32 orderId, uint256 amount) external nonReentrant {
        ComputeOrder storage order = computeOrders[orderId];
        require(order.isActive, "Order not active");
        require(amount > 0 && amount <= order.amount, "Invalid amount");

        uint256 totalCost = amount * order.pricePerUnit;

        // Transfer payment
        require(
            paymentToken.transferFrom(msg.sender, order.provider, totalCost),
            "Payment failed"
        );

        // Transfer resources
        userResources[msg.sender][order.resourceType] += amount;

        // Update order
        order.amount -= amount;
        if (order.amount == 0) {
            order.isActive = false;
        }

        emit OrderFulfilled(orderId, msg.sender, amount);
    }

    /**
     * @dev Get current price for a resource swap
     * @param resourceType Type of compute resource
     * @param isBuying True if buying resources, false if selling
     * @param amount Amount to swap
     */
    function getPrice(
        ResourceType resourceType,
        bool isBuying,
        uint256 amount
    ) external view returns (uint256) {
        ResourcePool storage pool = resourcePools[resourceType];
        require(pool.resourceAmount > 0 && pool.tokenAmount > 0, "Empty pool");

        if (isBuying) {
            // Calculate token amount needed to buy resources
            uint256 numerator = amount * pool.tokenAmount * FEE_DENOMINATOR;
            uint256 denominator = (pool.resourceAmount - amount) * (FEE_DENOMINATOR - pool.feeRate);
            return (numerator / denominator) + 1;
        } else {
            // Calculate tokens received for selling resources
            uint256 amountInWithFee = amount * (FEE_DENOMINATOR - pool.feeRate);
            uint256 numerator = amountInWithFee * pool.tokenAmount;
            uint256 denominator = (pool.resourceAmount * FEE_DENOMINATOR) + amountInWithFee;
            return numerator / denominator;
        }
    }

    /**
     * @dev Update resource metrics
     */
    function _updateMetrics(
        ResourceType resourceType,
        uint256 supplyDelta,
        uint256 demandDelta
    ) internal {
        ResourceMetrics storage metrics = resourceMetrics[resourceType];
        ResourcePool storage pool = resourcePools[resourceType];

        metrics.totalSupply += supplyDelta;
        metrics.totalDemand += demandDelta;

        if (pool.resourceAmount > 0 && pool.tokenAmount > 0) {
            // Calculate average price (tokens per resource unit)
            metrics.avgPrice = (pool.tokenAmount * 1e18) / pool.resourceAmount;
        }

        if (metrics.totalSupply > 0) {
            metrics.utilizationRate = (metrics.totalDemand * 10000) / metrics.totalSupply;
        }

        metrics.lastUpdate = block.timestamp;
    }

    /**
     * @dev Get pool information
     */
    function getPoolInfo(ResourceType resourceType)
        external
        view
        returns (
            uint256 resourceAmount,
            uint256 tokenAmount,
            uint256 totalLiquidity,
            uint256 feeRate
        )
    {
        ResourcePool storage pool = resourcePools[resourceType];
        return (
            pool.resourceAmount,
            pool.tokenAmount,
            pool.totalLiquidity,
            pool.feeRate
        );
    }

    /**
     * @dev Get user's liquidity position
     */
    function getUserLiquidity(address user, ResourceType resourceType)
        external
        view
        returns (uint256 liquidity, uint256 resourceBalance)
    {
        ResourcePool storage pool = resourcePools[resourceType];
        return (pool.liquidity[user], pool.resourceBalance[user]);
    }

    /**
     * @dev Update fee rate for a pool (owner only)
     */
    function setFeeRate(ResourceType resourceType, uint256 newFeeRate)
        external
        onlyOwner
    {
        require(newFeeRate <= 100, "Fee too high"); // Max 1%
        resourcePools[resourceType].feeRate = newFeeRate;
    }
}