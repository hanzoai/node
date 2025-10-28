# Hanzo Free Tier Setup Guide
## Offer Free Inference to Your Users

**Last Updated:** October 26, 2025

---

## Overview

This guide shows you how to set up a **free tier** for your users using DigitalOcean's serverless inference, with proper rate limiting, quotas, and usage tracking.

### Free Tier Architecture

```
User (hanzo-desktop) 
    ↓
api.hanzo.ai (your free tier proxy)
    ↓
    ├─ Rate Limiting (requests/min, tokens/day)
    ├─ Usage Tracking (per user)
    ├─ Quota Management (daily/monthly limits)
    └─ Model Selection (free tier models only)
    ↓
inference.do-ai.run (DigitalOcean - you pay)
```

### Key Features:
- ✅ Free tier with usage limits
- ✅ Automatic rate limiting
- ✅ Per-user quota tracking
- ✅ Model restrictions (fast models only)
- ✅ Upgrade path to paid plans
- ✅ Usage analytics

---

## Free Tier Configuration Options

### Option 1: Simple Free Tier (Recommended to Start)

**Limits:**
- 100 requests per day per user
- 10,000 tokens per day per user
- 5 requests per minute per user
- Access to fast models only (8B/12B)

**Cost Estimation:**
- Average user: ~$0.10-0.50/month
- 1,000 free users: ~$100-500/month (manageable with DO credits)

### Option 2: Generous Free Tier

**Limits:**
- 500 requests per day per user
- 50,000 tokens per day per user
- 20 requests per minute per user
- Access to mid-size models (up to 32B)

**Cost Estimation:**
- Average user: ~$0.50-2.00/month
- 1,000 free users: ~$500-2,000/month

### Option 3: Trial + Paid Hybrid

**Free Trial (7 days):**
- Unlimited requests
- Access to all models (including 70B)
- After trial: require payment or downgrade to free tier

**Free Tier (After Trial):**
- 50 requests per day
- 5,000 tokens per day
- Access to 8B models only

---

## Implementation

### Step 1: Set Up api.hanzo.ai Proxy

Create a proxy service that sits between users and DigitalOcean:

```typescript
// /api.hanzo.ai/src/free-tier-proxy.ts

import { Hono } from 'hono';
import { RedisClient } from 'redis';

const app = new Hono();

// Rate limiting configuration
const RATE_LIMITS = {
  free: {
    requestsPerMinute: 5,
    requestsPerDay: 100,
    tokensPerDay: 10000,
    allowedModels: ['llama3-8b-instruct', 'mistral-nemo-instruct-2407']
  },
  trial: {
    requestsPerMinute: 20,
    requestsPerDay: 500,
    tokensPerDay: 50000,
    allowedModels: ['llama3.3-70b-instruct', 'llama3-8b-instruct', 'deepseek-r1-distill-llama-70b'],
    trialDurationDays: 7
  },
  paid: {
    requestsPerMinute: 100,
    requestsPerDay: 10000,
    tokensPerDay: 1000000,
    allowedModels: ['*'] // All models
  }
};

// User tier checker
async function getUserTier(userId: string): Promise<'free' | 'trial' | 'paid'> {
  const user = await db.users.findOne({ id: userId });
  
  // Check if trial is still valid
  if (user.trialExpiresAt && new Date() < user.trialExpiresAt) {
    return 'trial';
  }
  
  // Check if user has paid subscription
  if (user.subscriptionStatus === 'active') {
    return 'paid';
  }
  
  return 'free';
}

// Rate limiting middleware
async function rateLimitMiddleware(c: Context, next: Next) {
  const userId = c.req.header('X-User-ID');
  const tier = await getUserTier(userId);
  const limits = RATE_LIMITS[tier];
  
  // Check requests per minute
  const minuteKey = `ratelimit:${userId}:minute:${getCurrentMinute()}`;
  const minuteCount = await redis.incr(minuteKey);
  await redis.expire(minuteKey, 60);
  
  if (minuteCount > limits.requestsPerMinute) {
    return c.json({ error: 'Rate limit exceeded. Please wait.' }, 429);
  }
  
  // Check requests per day
  const dayKey = `ratelimit:${userId}:day:${getCurrentDay()}`;
  const dayCount = await redis.incr(dayKey);
  await redis.expire(dayKey, 86400);
  
  if (dayCount > limits.requestsPerDay) {
    return c.json({ 
      error: 'Daily quota exceeded. Upgrade to continue.',
      upgradeUrl: 'https://hanzo.ai/pricing'
    }, 429);
  }
  
  // Store limits for response headers
  c.set('rateLimitRemaining', limits.requestsPerDay - dayCount);
  c.set('rateLimitReset', getNextDayTimestamp());
  
  await next();
}

// Proxy inference requests
app.post('/v1/chat/completions', rateLimitMiddleware, async (c) => {
  const userId = c.req.header('X-User-ID');
  const tier = await getUserTier(userId);
  const limits = RATE_LIMITS[tier];
  const body = await c.req.json();
  
  // Validate model access
  if (!limits.allowedModels.includes('*') && !limits.allowedModels.includes(body.model)) {
    return c.json({
      error: `Model '${body.model}' not available in ${tier} tier.`,
      availableModels: limits.allowedModels,
      upgradeUrl: 'https://hanzo.ai/pricing'
    }, 403);
  }
  
  // Track token usage
  const dayKey = `tokens:${userId}:day:${getCurrentDay()}`;
  const tokensUsed = await redis.get(dayKey) || 0;
  
  if (tokensUsed > limits.tokensPerDay) {
    return c.json({
      error: 'Daily token quota exceeded.',
      upgradeUrl: 'https://hanzo.ai/pricing'
    }, 429);
  }
  
  // Forward to DigitalOcean
  const response = await fetch('https://inference.do-ai.run/v1/chat/completions', {
    method: 'POST',
    headers: {
      'Authorization': `Bearer ${process.env.DO_MODEL_ACCESS_KEY}`,
      'Content-Type': 'application/json'
    },
    body: JSON.stringify(body)
  });
  
  const result = await response.json();
  
  // Track token usage
  const tokensUsedInRequest = result.usage?.total_tokens || 0;
  await redis.incrby(dayKey, tokensUsedInRequest);
  await redis.expire(dayKey, 86400);
  
  // Add rate limit headers
  return c.json(result, {
    headers: {
      'X-RateLimit-Limit': limits.requestsPerDay.toString(),
      'X-RateLimit-Remaining': c.get('rateLimitRemaining').toString(),
      'X-RateLimit-Reset': c.get('rateLimitReset').toString(),
      'X-Token-Limit': limits.tokensPerDay.toString(),
      'X-Token-Used': tokensUsed.toString()
    }
  });
});

export default app;
```

### Step 2: Database Schema for User Tracking

```sql
-- PostgreSQL schema for user tracking

CREATE TABLE users (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  email VARCHAR(255) UNIQUE NOT NULL,
  created_at TIMESTAMP DEFAULT NOW(),
  
  -- Tier management
  tier VARCHAR(20) DEFAULT 'free', -- 'free', 'trial', 'paid'
  trial_started_at TIMESTAMP,
  trial_expires_at TIMESTAMP,
  subscription_status VARCHAR(20), -- 'active', 'cancelled', 'expired'
  subscription_expires_at TIMESTAMP,
  
  -- Usage tracking
  total_requests INTEGER DEFAULT 0,
  total_tokens INTEGER DEFAULT 0,
  last_request_at TIMESTAMP,
  
  -- Billing
  stripe_customer_id VARCHAR(255),
  stripe_subscription_id VARCHAR(255)
);

CREATE TABLE usage_logs (
  id SERIAL PRIMARY KEY,
  user_id UUID REFERENCES users(id),
  timestamp TIMESTAMP DEFAULT NOW(),
  model VARCHAR(100),
  tokens_used INTEGER,
  request_duration_ms INTEGER,
  cost_usd DECIMAL(10, 6)
);

CREATE INDEX idx_usage_user_timestamp ON usage_logs(user_id, timestamp);
CREATE INDEX idx_users_tier ON users(tier);
```

### Step 3: hanzo-desktop Configuration

Update hanzo-desktop to use your free tier proxy:

```rust
// /hanzo-desktop/apps/hanzo-desktop/src-tauri/src/local_hanzo_node/hanzo_node_options.rs

impl HanzoNodeOptions {
    pub fn default_with_free_tier() -> Self {
        HanzoNodeOptions {
            // ... other config ...
            
            // Free tier configuration
            initial_agent_urls: Some(
                "https://api.hanzo.ai,https://api.hanzo.ai".to_string(),
            ),
            initial_agent_names: Some("hanzo_free,hanzo_fast".to_string()),
            initial_agent_models: Some(
                // Only fast models for free tier
                "openai:llama3-8b-instruct,openai:mistral-nemo-instruct-2407".to_string(),
            ),
            initial_agent_api_keys: Some(
                // Users authenticate with their hanzo.ai account
                "user_session_token,user_session_token".to_string(),
            ),
            
            // Store URL
            hanzo_store_url: Some("https://store-api.hanzo.ai".to_string()),
        }
    }
}
```

### Step 4: Environment Variables for api.hanzo.ai

```bash
# /etc/hanzo-api/.env

# DigitalOcean credentials (you pay for this)
DO_MODEL_ACCESS_KEY=your_do_model_access_key_here

# Redis for rate limiting
REDIS_URL=redis://localhost:6379

# PostgreSQL for user tracking
DATABASE_URL=postgresql://user:password@localhost:5432/hanzo_api

# Pricing tiers
FREE_TIER_REQUESTS_PER_DAY=100
FREE_TIER_TOKENS_PER_DAY=10000
FREE_TIER_REQUESTS_PER_MINUTE=5

TRIAL_TIER_REQUESTS_PER_DAY=500
TRIAL_TIER_TOKENS_PER_DAY=50000
TRIAL_TIER_DURATION_DAYS=7

# Stripe for payments
STRIPE_SECRET_KEY=sk_live_...
STRIPE_WEBHOOK_SECRET=whsec_...

# Model restrictions
FREE_TIER_MODELS=llama3-8b-instruct,mistral-nemo-instruct-2407
TRIAL_TIER_MODELS=llama3.3-70b-instruct,llama3-8b-instruct,deepseek-r1-distill-llama-70b
```

### Step 5: User Registration Flow

```typescript
// /api.hanzo.ai/src/auth.ts

import { Stripe } from 'stripe';

const stripe = new Stripe(process.env.STRIPE_SECRET_KEY);

// User registration
app.post('/auth/register', async (c) => {
  const { email, password } = await c.req.json();
  
  // Create user with 7-day trial
  const trialExpiresAt = new Date();
  trialExpiresAt.setDate(trialExpiresAt.getDate() + 7);
  
  const user = await db.users.create({
    email,
    passwordHash: await hashPassword(password),
    tier: 'trial',
    trialStartedAt: new Date(),
    trialExpiresAt
  });
  
  // Create Stripe customer for future billing
  const stripeCustomer = await stripe.customers.create({
    email,
    metadata: { userId: user.id }
  });
  
  await db.users.update({
    where: { id: user.id },
    data: { stripeCustomerId: stripeCustomer.id }
  });
  
  return c.json({
    message: 'Registration successful!',
    trialExpiresAt,
    sessionToken: generateSessionToken(user)
  });
});

// Check user status
app.get('/auth/status', async (c) => {
  const userId = c.req.header('X-User-ID');
  const user = await db.users.findOne({ id: userId });
  const tier = await getUserTier(userId);
  
  const usage = {
    requestsToday: await getRequestsToday(userId),
    tokensToday: await getTokensToday(userId),
    totalRequests: user.totalRequests,
    totalTokens: user.totalTokens
  };
  
  return c.json({
    tier,
    trialExpiresAt: user.trialExpiresAt,
    subscriptionStatus: user.subscriptionStatus,
    usage,
    limits: RATE_LIMITS[tier]
  });
});
```

---

## Pricing Tiers Example

### Free Forever
**Price:** $0/month
- 100 requests/day
- 10,000 tokens/day
- Fast models only (8B/12B)
- 5 req/min rate limit
- Community support

### Starter (Paid)
**Price:** $9/month
- 1,000 requests/day
- 100,000 tokens/day
- Access to 70B models
- 50 req/min rate limit
- Email support

### Pro (Paid)
**Price:** $29/month
- 5,000 requests/day
- 500,000 tokens/day
- All open source models
- 100 req/min rate limit
- Priority support

### Enterprise (Paid)
**Price:** Custom
- Unlimited requests
- Unlimited tokens
- All models (including commercial)
- Custom rate limits
- Dedicated support
- SLA guarantees

---

## Cost Calculation

### Your Costs (DigitalOcean)

**Free Tier User (100 req/day):**
- Average request: 100 tokens (50 input + 50 output)
- Daily tokens: 100 req × 100 tokens = 10,000 tokens
- Monthly tokens: 10,000 × 30 = 300,000 tokens
- Cost: ~$0.15-0.50/month per user (8B model)

**1,000 Free Users:**
- Monthly cost: $150-500
- Manageable with $200 DO credit!

**Optimization:**
- Use smallest models for free tier (8B)
- Set aggressive token limits
- Encourage upgrades to paid tiers

### Revenue Potential

**100 paid users @ $9/month:**
- Revenue: $900/month
- Your costs: ~$200-400 (depending on usage)
- **Profit: $500-700/month**

**1,000 paid users @ $9/month:**
- Revenue: $9,000/month
- Your costs: ~$2,000-4,000
- **Profit: $5,000-7,000/month**

---

## Monitoring & Analytics

### Track These Metrics:

```typescript
// /api.hanzo.ai/src/analytics.ts

// Daily usage report
async function getDailyUsageReport() {
  const stats = await db.usageLogs.aggregate({
    where: { 
      timestamp: { gte: startOfDay() } 
    },
    _sum: {
      tokensUsed: true,
      costUsd: true
    },
    _count: {
      id: true
    },
    by: ['userId']
  });
  
  return {
    totalRequests: stats._count.id,
    totalTokens: stats._sum.tokensUsed,
    totalCostUsd: stats._sum.costUsd,
    avgCostPerRequest: stats._sum.costUsd / stats._count.id
  };
}

// Monitor costs by tier
async function getCostsByTier() {
  const costs = await db.usageLogs.aggregate({
    join: { users: true },
    _sum: { costUsd: true },
    by: ['users.tier']
  });
  
  return costs;
}
```

### Set Up Alerts:

```typescript
// Alert if daily cost exceeds budget
const DAILY_BUDGET_USD = 50;

setInterval(async () => {
  const stats = await getDailyUsageReport();
  
  if (stats.totalCostUsd > DAILY_BUDGET_USD) {
    await sendAlert({
      type: 'BUDGET_EXCEEDED',
      message: `Daily cost $${stats.totalCostUsd} exceeds budget $${DAILY_BUDGET_USD}`,
      stats
    });
  }
}, 3600000); // Check every hour
```

---

## User Experience Flow

### 1. New User Signs Up
- Gets 7-day trial with full access
- Can use all models (including 70B)
- Higher rate limits

### 2. Trial Expires
- Automatically downgrades to free tier
- Shows upgrade prompt in UI
- Limited to fast models only

### 3. User Upgrades
- Payment via Stripe
- Immediate access to paid tier
- Higher limits, more models

### 4. Usage Limits Reached
- Show friendly error message
- Offer upgrade option
- Display usage stats

---

## UI Components

### Usage Widget (hanzo-desktop)

```tsx
// Usage display component
function UsageWidget({ userId }: { userId: string }) {
  const { data: status } = useQuery(['user-status'], () =>
    fetch('https://api.hanzo.ai/auth/status', {
      headers: { 'X-User-ID': userId }
    }).then(r => r.json())
  );
  
  const percentUsed = (status.usage.requestsToday / status.limits.requestsPerDay) * 100;
  
  return (
    <div className="usage-widget">
      <h3>{status.tier === 'free' ? 'Free Tier' : 'Trial'}</h3>
      
      <div className="progress-bar">
        <div style={{ width: `${percentUsed}%` }} />
      </div>
      
      <p>
        {status.usage.requestsToday} / {status.limits.requestsPerDay} requests today
      </p>
      
      {status.tier === 'free' && (
        <button onClick={() => navigate('/upgrade')}>
          Upgrade for unlimited access
        </button>
      )}
      
      {status.tier === 'trial' && (
        <p>Trial expires: {formatDate(status.trialExpiresAt)}</p>
      )}
    </div>
  );
}
```

---

## Deployment Checklist

### Infrastructure:
- [ ] Deploy api.hanzo.ai proxy service
- [ ] Set up Redis for rate limiting
- [ ] Set up PostgreSQL for user data
- [ ] Configure Nginx with SSL
- [ ] Set up DNS: api.hanzo.ai

### Code:
- [ ] Implement rate limiting middleware
- [ ] Add user authentication
- [ ] Set up usage tracking
- [ ] Integrate Stripe for payments
- [ ] Add analytics dashboard

### Testing:
- [ ] Test free tier limits
- [ ] Test trial expiration
- [ ] Test upgrade flow
- [ ] Load test rate limiting
- [ ] Verify cost tracking

### Monitoring:
- [ ] Set up cost alerts
- [ ] Monitor daily usage
- [ ] Track conversion rates
- [ ] Monitor API errors

---

## Security Best Practices

1. **API Key Protection:**
   - Never expose DO key to users
   - Use session tokens for users
   - Rotate keys regularly

2. **Rate Limiting:**
   - Implement per-user limits
   - Add IP-based limits too
   - Block abusive users

3. **Usage Tracking:**
   - Log all requests
   - Track costs in real-time
   - Set budget alerts

4. **Payment Security:**
   - Use Stripe for PCI compliance
   - Never store credit cards
   - Verify webhooks

---

## Next Steps

1. **Set up api.hanzo.ai infrastructure**
2. **Deploy proxy service with rate limiting**
3. **Configure user database**
4. **Integrate Stripe for payments**
5. **Update hanzo-desktop to use proxy**
6. **Test free tier flow**
7. **Monitor costs and optimize**

---

## Support Resources

- **DigitalOcean Pricing:** https://docs.digitalocean.com/products/ai-ml/details/pricing/
- **Stripe Integration:** https://stripe.com/docs/api
- **Redis Rate Limiting:** https://redis.io/commands/incr/
- **Cost Optimization:** Contact team@hanzo.ai

---

**Free Tier Status:** 📋 Ready to implement!

This setup gives you full control over your free tier while using DigitalOcean credits efficiently!
