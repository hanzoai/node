// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import "@openzeppelin/contracts/token/ERC20/extensions/ERC20Burnable.sol";
import "@openzeppelin/contracts/token/ERC20/extensions/ERC20Permit.sol";
import "@openzeppelin/contracts/access/Ownable.sol";
import "@openzeppelin/contracts/security/ReentrancyGuard.sol";

/**
 * @title HANZOToken
 * @dev Utility token for compute marketplace transactions
 *
 * Features:
 * - Payment for compute resources
 * - Liquidity provision rewards
 * - Provider incentives
 * - Burn mechanism
 * - Bridge compatibility
 */
contract HANZOToken is ERC20, ERC20Burnable, ERC20Permit, Ownable, ReentrancyGuard {
    // Token economics
    uint256 public constant MAX_SUPPLY = 10_000_000_000 * 10**18; // 10 billion tokens
    uint256 public constant INITIAL_SUPPLY = 2_000_000_000 * 10**18; // 2 billion initial

    // Minting schedule
    uint256 public constant ANNUAL_INFLATION_RATE = 200; // 2% in basis points
    uint256 public constant INFLATION_DENOMINATOR = 10000;
    uint256 public lastMintTime;
    uint256 public mintedThisYear;
    uint256 public currentYearCap;

    // Resource provider rewards
    mapping(address => uint256) public providerRewards;
    mapping(address => uint256) public lastClaimTime;
    uint256 public totalRewardsAllocated;
    uint256 public rewardPool;

    // Liquidity mining
    struct LiquidityMining {
        uint256 totalShares;
        uint256 rewardPerShare;
        uint256 lastUpdateTime;
        uint256 rewardRate; // Tokens per second
        mapping(address => uint256) shares;
        mapping(address => uint256) rewardDebt;
        mapping(address => uint256) pendingRewards;
    }

    LiquidityMining public liquidityMining;

    // Bridge support
    mapping(address => bool) public bridges;
    mapping(bytes32 => bool) public processedNonces;

    // Fee collection
    uint256 public accumulatedFees;
    address public feeCollector;

    // Events
    event RewardsClaimed(address indexed provider, uint256 amount);
    event LiquidityRewardsClaimed(address indexed user, uint256 amount);
    event BridgeTransfer(address indexed from, address indexed to, uint256 amount, bytes32 nonce);
    event ProviderRewarded(address indexed provider, uint256 amount);
    event MintingCompleted(uint256 amount, uint256 timestamp);
    event RewardRateUpdated(uint256 newRate);
    event BridgeAdded(address bridge);
    event BridgeRemoved(address bridge);

    modifier onlyBridge() {
        require(bridges[msg.sender], "Not authorized bridge");
        _;
    }

    constructor(
        address _feeCollector
    ) ERC20("HANZO Token", "HANZO") ERC20Permit("HANZO Token") Ownable(msg.sender) {
        feeCollector = _feeCollector;

        // Mint initial supply
        _mint(msg.sender, INITIAL_SUPPLY / 2); // 50% to deployer
        _mint(address(this), INITIAL_SUPPLY / 2); // 50% to contract for rewards

        rewardPool = INITIAL_SUPPLY / 2;
        lastMintTime = block.timestamp;
        currentYearCap = (totalSupply() * ANNUAL_INFLATION_RATE) / INFLATION_DENOMINATOR;

        // Initialize liquidity mining
        liquidityMining.rewardRate = 100 * 10**18; // 100 tokens per second initially
        liquidityMining.lastUpdateTime = block.timestamp;
    }

    /**
     * @dev Annual minting for inflation (callable by anyone after 1 year)
     */
    function mintAnnualInflation() external nonReentrant {
        require(block.timestamp >= lastMintTime + 365 days, "Year not passed");
        require(totalSupply() < MAX_SUPPLY, "Max supply reached");

        uint256 currentSupply = totalSupply();
        uint256 mintAmount = (currentSupply * ANNUAL_INFLATION_RATE) / INFLATION_DENOMINATOR;

        // Cap at max supply
        if (currentSupply + mintAmount > MAX_SUPPLY) {
            mintAmount = MAX_SUPPLY - currentSupply;
        }

        // Distribution: 40% rewards, 30% treasury, 30% liquidity
        uint256 rewardsAmount = (mintAmount * 40) / 100;
        uint256 treasuryAmount = (mintAmount * 30) / 100;
        uint256 liquidityAmount = (mintAmount * 30) / 100;

        _mint(address(this), rewardsAmount + liquidityAmount);
        _mint(feeCollector, treasuryAmount);

        rewardPool += rewardsAmount;
        lastMintTime = block.timestamp;
        mintedThisYear = mintAmount;
        currentYearCap = (totalSupply() * ANNUAL_INFLATION_RATE) / INFLATION_DENOMINATOR;

        emit MintingCompleted(mintAmount, block.timestamp);
    }

    /**
     * @dev Reward compute providers
     */
    function rewardProvider(address provider, uint256 amount) external onlyOwner {
        require(provider != address(0), "Invalid provider");
        require(amount > 0 && amount <= rewardPool, "Invalid amount");

        providerRewards[provider] += amount;
        totalRewardsAllocated += amount;
        rewardPool -= amount;

        emit ProviderRewarded(provider, amount);
    }

    /**
     * @dev Claim provider rewards
     */
    function claimProviderRewards() external nonReentrant {
        uint256 rewards = providerRewards[msg.sender];
        require(rewards > 0, "No rewards");

        // Implement cooldown
        require(
            block.timestamp >= lastClaimTime[msg.sender] + 1 days,
            "Cooldown active"
        );

        providerRewards[msg.sender] = 0;
        lastClaimTime[msg.sender] = block.timestamp;

        _transfer(address(this), msg.sender, rewards);

        emit RewardsClaimed(msg.sender, rewards);
    }

    /**
     * @dev Deposit liquidity for mining rewards
     */
    function depositLiquidity(uint256 amount) external nonReentrant {
        require(amount > 0, "Invalid amount");

        _updateLiquidityRewards(msg.sender);

        _transfer(msg.sender, address(this), amount);

        liquidityMining.shares[msg.sender] += amount;
        liquidityMining.totalShares += amount;
        liquidityMining.rewardDebt[msg.sender] =
            (liquidityMining.shares[msg.sender] * liquidityMining.rewardPerShare) / 1e18;
    }

    /**
     * @dev Withdraw liquidity
     */
    function withdrawLiquidity(uint256 amount) external nonReentrant {
        require(amount > 0, "Invalid amount");
        require(liquidityMining.shares[msg.sender] >= amount, "Insufficient shares");

        _updateLiquidityRewards(msg.sender);

        liquidityMining.shares[msg.sender] -= amount;
        liquidityMining.totalShares -= amount;
        liquidityMining.rewardDebt[msg.sender] =
            (liquidityMining.shares[msg.sender] * liquidityMining.rewardPerShare) / 1e18;

        _transfer(address(this), msg.sender, amount);
    }

    /**
     * @dev Claim liquidity mining rewards
     */
    function claimLiquidityRewards() external nonReentrant {
        _updateLiquidityRewards(msg.sender);

        uint256 pending = liquidityMining.pendingRewards[msg.sender];
        require(pending > 0, "No rewards");

        liquidityMining.pendingRewards[msg.sender] = 0;
        liquidityMining.rewardDebt[msg.sender] =
            (liquidityMining.shares[msg.sender] * liquidityMining.rewardPerShare) / 1e18;

        _transfer(address(this), msg.sender, pending);

        emit LiquidityRewardsClaimed(msg.sender, pending);
    }

    /**
     * @dev Update liquidity rewards
     */
    function _updateLiquidityRewards(address user) private {
        if (liquidityMining.totalShares > 0) {
            uint256 timeDelta = block.timestamp - liquidityMining.lastUpdateTime;
            uint256 rewards = timeDelta * liquidityMining.rewardRate;

            if (rewards > rewardPool) {
                rewards = rewardPool;
            }

            liquidityMining.rewardPerShare += (rewards * 1e18) / liquidityMining.totalShares;
            rewardPool -= rewards;
        }

        liquidityMining.lastUpdateTime = block.timestamp;

        if (user != address(0) && liquidityMining.shares[user] > 0) {
            uint256 userRewards =
                (liquidityMining.shares[user] * liquidityMining.rewardPerShare) / 1e18
                - liquidityMining.rewardDebt[user];

            liquidityMining.pendingRewards[user] += userRewards;
        }
    }

    /**
     * @dev Bridge transfer
     */
    function bridgeTransfer(
        address to,
        uint256 amount,
        bytes32 nonce
    ) external onlyBridge nonReentrant {
        require(!processedNonces[nonce], "Nonce already processed");
        processedNonces[nonce] = true;

        _mint(to, amount);

        emit BridgeTransfer(msg.sender, to, amount, nonce);
    }

    /**
     * @dev Bridge burn (for cross-chain transfers)
     */
    function bridgeBurn(
        uint256 amount,
        bytes32 destinationChain
    ) external nonReentrant {
        _burn(msg.sender, amount);

        emit BridgeTransfer(msg.sender, address(0), amount, destinationChain);
    }

    /**
     * @dev Add bridge
     */
    function addBridge(address bridge) external onlyOwner {
        require(bridge != address(0), "Invalid bridge");
        bridges[bridge] = true;
        emit BridgeAdded(bridge);
    }

    /**
     * @dev Remove bridge
     */
    function removeBridge(address bridge) external onlyOwner {
        bridges[bridge] = false;
        emit BridgeRemoved(bridge);
    }

    /**
     * @dev Update reward rate
     */
    function updateRewardRate(uint256 newRate) external onlyOwner {
        _updateLiquidityRewards(address(0));
        liquidityMining.rewardRate = newRate;
        emit RewardRateUpdated(newRate);
    }

    /**
     * @dev Collect accumulated fees
     */
    function collectFees() external {
        require(msg.sender == feeCollector, "Not fee collector");
        require(accumulatedFees > 0, "No fees");

        uint256 amount = accumulatedFees;
        accumulatedFees = 0;

        _transfer(address(this), feeCollector, amount);
    }

    /**
     * @dev Override transfer to collect fees
     */
    function _transfer(
        address from,
        address to,
        uint256 amount
    ) internal override {
        // Collect 0.05% fee on transfers (except minting/burning)
        uint256 feeAmount = 0;
        if (
            from != address(0) &&
            to != address(0) &&
            from != address(this) &&
            to != address(this)
        ) {
            feeAmount = (amount * 5) / 10000; // 0.05%
            accumulatedFees += feeAmount;
        }

        uint256 transferAmount = amount - feeAmount;
        super._transfer(from, to, transferAmount);

        if (feeAmount > 0) {
            super._transfer(from, address(this), feeAmount);
        }
    }

    /**
     * @dev View functions
     */
    function getPendingLiquidityRewards(address user) external view returns (uint256) {
        uint256 pending = liquidityMining.pendingRewards[user];

        if (liquidityMining.totalShares > 0 && liquidityMining.shares[user] > 0) {
            uint256 timeDelta = block.timestamp - liquidityMining.lastUpdateTime;
            uint256 rewards = timeDelta * liquidityMining.rewardRate;

            if (rewards > rewardPool) {
                rewards = rewardPool;
            }

            uint256 newRewardPerShare = liquidityMining.rewardPerShare +
                (rewards * 1e18) / liquidityMining.totalShares;

            pending += (liquidityMining.shares[user] * newRewardPerShare) / 1e18
                - liquidityMining.rewardDebt[user];
        }

        return pending;
    }

    function getLiquidityInfo(address user)
        external
        view
        returns (
            uint256 shares,
            uint256 pendingRewards,
            uint256 totalShares,
            uint256 rewardRate
        )
    {
        return (
            liquidityMining.shares[user],
            this.getPendingLiquidityRewards(user),
            liquidityMining.totalShares,
            liquidityMining.rewardRate
        );
    }

    function getTokenomicsInfo()
        external
        view
        returns (
            uint256 currentSupply,
            uint256 maxSupply,
            uint256 rewardPoolSize,
            uint256 nextMintTime,
            uint256 nextMintAmount
        )
    {
        uint256 potentialMint = (totalSupply() * ANNUAL_INFLATION_RATE) / INFLATION_DENOMINATOR;
        if (totalSupply() + potentialMint > MAX_SUPPLY) {
            potentialMint = MAX_SUPPLY - totalSupply();
        }

        return (
            totalSupply(),
            MAX_SUPPLY,
            rewardPool,
            lastMintTime + 365 days,
            potentialMint
        );
    }
}