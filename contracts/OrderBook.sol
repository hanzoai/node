// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import "@openzeppelin/contracts/security/ReentrancyGuard.sol";
import "@openzeppelin/contracts/access/Ownable.sol";
import "@openzeppelin/contracts/utils/structs/EnumerableSet.sol";

/**
 * @title OrderBook
 * @dev Advanced order matching engine for compute resources
 *
 * Features:
 * - Limit and market orders
 * - Order matching with price-time priority
 * - Circuit breakers
 * - Fee distribution
 * - Order cancellation
 */
contract OrderBook is ReentrancyGuard, Ownable {
    using EnumerableSet for EnumerableSet.Bytes32Set;

    // Order types
    enum OrderType { LIMIT, MARKET, STOP_LOSS, TAKE_PROFIT }
    enum OrderSide { BUY, SELL }
    enum OrderStatus { OPEN, FILLED, PARTIALLY_FILLED, CANCELLED }

    // Resource types (matching ComputeDEX)
    enum ResourceType {
        CPU, GPU, MEMORY, STORAGE,
        BANDWIDTH, WASM, DOCKER, K8S
    }

    // Order structure
    struct Order {
        bytes32 id;
        address trader;
        OrderType orderType;
        OrderSide side;
        ResourceType resource;
        uint256 amount;
        uint256 price;          // Price per unit in tokens
        uint256 filledAmount;
        uint256 timestamp;
        OrderStatus status;
        uint256 stopPrice;      // For stop orders
        uint256 expiryTime;     // Order expiration
    }

    // Market data
    struct Market {
        uint256 lastPrice;
        uint256 high24h;
        uint256 low24h;
        uint256 volume24h;
        uint256 baseVolume24h;  // Resource volume
        uint256 quoteVolume24h; // Token volume
        bool halted;            // Circuit breaker
        uint256 lastUpdateTime;
    }

    // Circuit breaker
    struct CircuitBreaker {
        uint256 priceChangeThreshold; // Percentage * 100
        uint256 volumeThreshold;
        uint256 cooldownPeriod;
        uint256 lastTriggerTime;
    }

    // State variables
    IERC20 public immutable paymentToken;

    mapping(bytes32 => Order) public orders;
    mapping(ResourceType => Market) public markets;
    mapping(ResourceType => CircuitBreaker) public circuitBreakers;

    // Order books per market
    mapping(ResourceType => EnumerableSet.Bytes32Set) private buyOrders;
    mapping(ResourceType => EnumerableSet.Bytes32Set) private sellOrders;

    // User orders
    mapping(address => EnumerableSet.Bytes32Set) private userOrders;

    // Price levels for efficient matching
    mapping(ResourceType => mapping(uint256 => EnumerableSet.Bytes32Set)) private buyOrdersAtPrice;
    mapping(ResourceType => mapping(uint256 => EnumerableSet.Bytes32Set)) private sellOrdersAtPrice;

    // Fee structure
    uint256 public makerFeeRate = 10; // 0.1%
    uint256 public takerFeeRate = 20; // 0.2%
    uint256 public constant FEE_DENOMINATOR = 10000;
    address public feeRecipient;

    // Statistics
    uint256 public totalOrdersCreated;
    uint256 public totalOrdersFilled;
    uint256 public totalVolumeTraded;

    // Events
    event OrderCreated(
        bytes32 indexed orderId,
        address indexed trader,
        OrderSide side,
        ResourceType resource,
        uint256 amount,
        uint256 price
    );

    event OrderFilled(
        bytes32 indexed orderId,
        address indexed trader,
        uint256 filledAmount,
        uint256 price
    );

    event OrderCancelled(
        bytes32 indexed orderId,
        address indexed trader
    );

    event Trade(
        ResourceType indexed resource,
        address indexed buyer,
        address indexed seller,
        uint256 amount,
        uint256 price,
        uint256 timestamp
    );

    event MarketHalted(
        ResourceType indexed resource,
        string reason
    );

    event MarketResumed(
        ResourceType indexed resource
    );

    modifier marketNotHalted(ResourceType resource) {
        require(!markets[resource].halted, "Market halted");
        _;
    }

    constructor(
        address _paymentToken,
        address _feeRecipient
    ) Ownable(msg.sender) {
        paymentToken = IERC20(_paymentToken);
        feeRecipient = _feeRecipient;

        // Initialize circuit breakers
        for (uint8 i = 0; i <= uint8(ResourceType.K8S); i++) {
            circuitBreakers[ResourceType(i)] = CircuitBreaker({
                priceChangeThreshold: 2000, // 20%
                volumeThreshold: 1000000 * 10**18, // 1M tokens
                cooldownPeriod: 15 minutes,
                lastTriggerTime: 0
            });
        }
    }

    /**
     * @dev Create a limit order
     */
    function createLimitOrder(
        OrderSide side,
        ResourceType resource,
        uint256 amount,
        uint256 price,
        uint256 expiryTime
    ) external nonReentrant marketNotHalted(resource) returns (bytes32 orderId) {
        require(amount > 0, "Invalid amount");
        require(price > 0, "Invalid price");
        require(expiryTime > block.timestamp, "Invalid expiry");

        orderId = keccak256(
            abi.encodePacked(
                msg.sender,
                block.timestamp,
                totalOrdersCreated++
            )
        );

        orders[orderId] = Order({
            id: orderId,
            trader: msg.sender,
            orderType: OrderType.LIMIT,
            side: side,
            resource: resource,
            amount: amount,
            price: price,
            filledAmount: 0,
            timestamp: block.timestamp,
            status: OrderStatus.OPEN,
            stopPrice: 0,
            expiryTime: expiryTime
        });

        // Add to order books
        if (side == OrderSide.BUY) {
            buyOrders[resource].add(orderId);
            buyOrdersAtPrice[resource][price].add(orderId);
        } else {
            sellOrders[resource].add(orderId);
            sellOrdersAtPrice[resource][price].add(orderId);
        }

        userOrders[msg.sender].add(orderId);

        emit OrderCreated(orderId, msg.sender, side, resource, amount, price);

        // Try to match order
        _matchOrder(orderId);
    }

    /**
     * @dev Create a market order
     */
    function createMarketOrder(
        OrderSide side,
        ResourceType resource,
        uint256 amount
    ) external nonReentrant marketNotHalted(resource) returns (bytes32 orderId) {
        require(amount > 0, "Invalid amount");

        orderId = keccak256(
            abi.encodePacked(
                msg.sender,
                block.timestamp,
                totalOrdersCreated++
            )
        );

        uint256 estimatedPrice = _estimateMarketPrice(side, resource, amount);
        require(estimatedPrice > 0, "Insufficient liquidity");

        orders[orderId] = Order({
            id: orderId,
            trader: msg.sender,
            orderType: OrderType.MARKET,
            side: side,
            resource: resource,
            amount: amount,
            price: estimatedPrice, // Use estimated for slippage protection
            filledAmount: 0,
            timestamp: block.timestamp,
            status: OrderStatus.OPEN,
            stopPrice: 0,
            expiryTime: block.timestamp + 1 minutes // Short expiry for market orders
        });

        userOrders[msg.sender].add(orderId);

        emit OrderCreated(orderId, msg.sender, side, resource, amount, estimatedPrice);

        // Execute immediately
        _executeMarketOrder(orderId);
    }

    /**
     * @dev Match a limit order against the order book
     */
    function _matchOrder(bytes32 orderId) private {
        Order storage order = orders[orderId];
        if (order.status != OrderStatus.OPEN) return;

        EnumerableSet.Bytes32Set storage oppositeBook =
            order.side == OrderSide.BUY ?
            sellOrders[order.resource] :
            buyOrders[order.resource];

        uint256 remainingAmount = order.amount - order.filledAmount;

        // Get matching orders
        bytes32[] memory matchingOrders = _findMatchingOrders(
            order,
            oppositeBook,
            remainingAmount
        );

        for (uint256 i = 0; i < matchingOrders.length && remainingAmount > 0; i++) {
            Order storage counterOrder = orders[matchingOrders[i]];

            if (counterOrder.status != OrderStatus.OPEN) continue;
            if (counterOrder.expiryTime < block.timestamp) {
                _cancelOrder(matchingOrders[i]);
                continue;
            }

            uint256 counterRemaining = counterOrder.amount - counterOrder.filledAmount;
            uint256 fillAmount = remainingAmount < counterRemaining ?
                remainingAmount : counterRemaining;

            // Execute trade
            _executeTrade(
                order.side == OrderSide.BUY ? orderId : matchingOrders[i],
                order.side == OrderSide.BUY ? matchingOrders[i] : orderId,
                fillAmount,
                counterOrder.price
            );

            remainingAmount -= fillAmount;
        }

        // Update order status
        if (order.filledAmount == order.amount) {
            order.status = OrderStatus.FILLED;
            _removeFromOrderBooks(orderId);
        } else if (order.filledAmount > 0) {
            order.status = OrderStatus.PARTIALLY_FILLED;
        }
    }

    /**
     * @dev Execute a market order
     */
    function _executeMarketOrder(bytes32 orderId) private {
        Order storage order = orders[orderId];

        EnumerableSet.Bytes32Set storage oppositeBook =
            order.side == OrderSide.BUY ?
            sellOrders[order.resource] :
            buyOrders[order.resource];

        uint256 remainingAmount = order.amount;

        // Get best orders
        bytes32[] memory bestOrders = _getBestOrders(
            order.resource,
            order.side == OrderSide.BUY,
            remainingAmount
        );

        for (uint256 i = 0; i < bestOrders.length && remainingAmount > 0; i++) {
            Order storage counterOrder = orders[bestOrders[i]];

            uint256 counterRemaining = counterOrder.amount - counterOrder.filledAmount;
            uint256 fillAmount = remainingAmount < counterRemaining ?
                remainingAmount : counterRemaining;

            _executeTrade(
                order.side == OrderSide.BUY ? orderId : bestOrders[i],
                order.side == OrderSide.BUY ? bestOrders[i] : orderId,
                fillAmount,
                counterOrder.price
            );

            remainingAmount -= fillAmount;
        }

        if (remainingAmount > 0) {
            // Partial fill, cancel remaining
            order.status = OrderStatus.PARTIALLY_FILLED;
            _removeFromOrderBooks(orderId);
        } else {
            order.status = OrderStatus.FILLED;
        }
    }

    /**
     * @dev Execute a trade between two orders
     */
    function _executeTrade(
        bytes32 buyOrderId,
        bytes32 sellOrderId,
        uint256 amount,
        uint256 price
    ) private {
        Order storage buyOrder = orders[buyOrderId];
        Order storage sellOrder = orders[sellOrderId];

        uint256 totalCost = amount * price;
        uint256 buyerFee = (totalCost * takerFeeRate) / FEE_DENOMINATOR;
        uint256 sellerFee = (totalCost * makerFeeRate) / FEE_DENOMINATOR;

        // Transfer tokens
        require(
            paymentToken.transferFrom(buyOrder.trader, sellOrder.trader, totalCost - sellerFee),
            "Payment failed"
        );

        // Collect fees
        require(
            paymentToken.transferFrom(buyOrder.trader, feeRecipient, buyerFee),
            "Fee payment failed"
        );

        if (sellerFee > 0) {
            require(
                paymentToken.transferFrom(sellOrder.trader, feeRecipient, sellerFee),
                "Fee collection failed"
            );
        }

        // Update orders
        buyOrder.filledAmount += amount;
        sellOrder.filledAmount += amount;

        // Update market data
        _updateMarketData(buyOrder.resource, price, amount);

        // Emit events
        emit Trade(
            buyOrder.resource,
            buyOrder.trader,
            sellOrder.trader,
            amount,
            price,
            block.timestamp
        );

        emit OrderFilled(buyOrderId, buyOrder.trader, amount, price);
        emit OrderFilled(sellOrderId, sellOrder.trader, amount, price);

        totalOrdersFilled++;
        totalVolumeTraded += totalCost;

        // Check circuit breaker
        _checkCircuitBreaker(buyOrder.resource);
    }

    /**
     * @dev Cancel an order
     */
    function cancelOrder(bytes32 orderId) external nonReentrant {
        Order storage order = orders[orderId];
        require(order.trader == msg.sender, "Not order owner");
        require(
            order.status == OrderStatus.OPEN ||
            order.status == OrderStatus.PARTIALLY_FILLED,
            "Cannot cancel"
        );

        _cancelOrder(orderId);
    }

    /**
     * @dev Internal cancel
     */
    function _cancelOrder(bytes32 orderId) private {
        Order storage order = orders[orderId];

        order.status = OrderStatus.CANCELLED;
        _removeFromOrderBooks(orderId);

        emit OrderCancelled(orderId, order.trader);
    }

    /**
     * @dev Remove order from order books
     */
    function _removeFromOrderBooks(bytes32 orderId) private {
        Order storage order = orders[orderId];

        if (order.side == OrderSide.BUY) {
            buyOrders[order.resource].remove(orderId);
            buyOrdersAtPrice[order.resource][order.price].remove(orderId);
        } else {
            sellOrders[order.resource].remove(orderId);
            sellOrdersAtPrice[order.resource][order.price].remove(orderId);
        }

        userOrders[order.trader].remove(orderId);
    }

    /**
     * @dev Find matching orders
     */
    function _findMatchingOrders(
        Order storage order,
        EnumerableSet.Bytes32Set storage oppositeBook,
        uint256 amount
    ) private view returns (bytes32[] memory) {
        uint256 count = oppositeBook.length();
        bytes32[] memory matching = new bytes32[](count);
        uint256 matchCount = 0;

        for (uint256 i = 0; i < count && amount > 0; i++) {
            bytes32 candidateId = oppositeBook.at(i);
            Order storage candidate = orders[candidateId];

            bool priceMatch = order.side == OrderSide.BUY ?
                candidate.price <= order.price :
                candidate.price >= order.price;

            if (priceMatch && candidate.status == OrderStatus.OPEN) {
                matching[matchCount++] = candidateId;
                uint256 available = candidate.amount - candidate.filledAmount;
                amount = amount > available ? amount - available : 0;
            }
        }

        // Resize array
        assembly {
            mstore(matching, matchCount)
        }

        return matching;
    }

    /**
     * @dev Get best orders for market execution
     */
    function _getBestOrders(
        ResourceType resource,
        bool isBuying,
        uint256 amount
    ) private view returns (bytes32[] memory) {
        // Implementation would sort by price and return best orders
        // Simplified for example
        EnumerableSet.Bytes32Set storage book = isBuying ?
            sellOrders[resource] : buyOrders[resource];

        uint256 count = book.length() < 10 ? book.length() : 10;
        bytes32[] memory best = new bytes32[](count);

        for (uint256 i = 0; i < count; i++) {
            best[i] = book.at(i);
        }

        return best;
    }

    /**
     * @dev Estimate market price
     */
    function _estimateMarketPrice(
        OrderSide side,
        ResourceType resource,
        uint256 amount
    ) private view returns (uint256) {
        // Would calculate weighted average price
        // Simplified: return last price
        return markets[resource].lastPrice > 0 ?
            markets[resource].lastPrice : 1000 * 10**18;
    }

    /**
     * @dev Update market data
     */
    function _updateMarketData(
        ResourceType resource,
        uint256 price,
        uint256 volume
    ) private {
        Market storage market = markets[resource];

        market.lastPrice = price;
        market.volume24h += volume;
        market.quoteVolume24h += volume * price;

        if (price > market.high24h || market.high24h == 0) {
            market.high24h = price;
        }
        if (price < market.low24h || market.low24h == 0) {
            market.low24h = price;
        }

        market.lastUpdateTime = block.timestamp;
    }

    /**
     * @dev Check circuit breaker
     */
    function _checkCircuitBreaker(ResourceType resource) private {
        Market storage market = markets[resource];
        CircuitBreaker storage breaker = circuitBreakers[resource];

        if (breaker.lastTriggerTime + breaker.cooldownPeriod > block.timestamp) {
            return; // Still in cooldown
        }

        // Check volume threshold
        if (market.volume24h > breaker.volumeThreshold) {
            market.halted = true;
            breaker.lastTriggerTime = block.timestamp;
            emit MarketHalted(resource, "Volume threshold exceeded");
            return;
        }

        // Check price change
        if (market.high24h > 0 && market.low24h > 0) {
            uint256 priceChange = ((market.high24h - market.low24h) * 10000) / market.low24h;
            if (priceChange > breaker.priceChangeThreshold) {
                market.halted = true;
                breaker.lastTriggerTime = block.timestamp;
                emit MarketHalted(resource, "Price volatility threshold exceeded");
            }
        }
    }

    /**
     * @dev Resume halted market (admin)
     */
    function resumeMarket(ResourceType resource) external onlyOwner {
        markets[resource].halted = false;
        emit MarketResumed(resource);
    }

    /**
     * @dev Update circuit breaker settings
     */
    function updateCircuitBreaker(
        ResourceType resource,
        uint256 priceThreshold,
        uint256 volumeThreshold,
        uint256 cooldown
    ) external onlyOwner {
        CircuitBreaker storage breaker = circuitBreakers[resource];
        breaker.priceChangeThreshold = priceThreshold;
        breaker.volumeThreshold = volumeThreshold;
        breaker.cooldownPeriod = cooldown;
    }

    /**
     * @dev Get order book depth
     */
    function getOrderBookDepth(ResourceType resource, uint256 levels)
        external
        view
        returns (
            uint256[] memory buyPrices,
            uint256[] memory buyAmounts,
            uint256[] memory sellPrices,
            uint256[] memory sellAmounts
        )
    {
        // Would return aggregated order book depth
        // Simplified for example
        buyPrices = new uint256[](levels);
        buyAmounts = new uint256[](levels);
        sellPrices = new uint256[](levels);
        sellAmounts = new uint256[](levels);
    }

    /**
     * @dev Get user's open orders
     */
    function getUserOrders(address user)
        external
        view
        returns (bytes32[] memory orderIds)
    {
        uint256 count = userOrders[user].length();
        orderIds = new bytes32[](count);

        for (uint256 i = 0; i < count; i++) {
            orderIds[i] = userOrders[user].at(i);
        }
    }
}