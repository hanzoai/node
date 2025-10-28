# Hanzo AI Gateway - Standalone Service
## Secure LLM API Gateway with Multi-Layer Protection

**Version:** 1.0.0  
**Last Updated:** October 26, 2025

---

## Architecture Overview

```
┌──────────────────┐
│  User Devices    │
│  (hanzo-desktop) │
│  (hanzo-mobile)  │
│  (web app)       │
└────────┬─────────┘
         │
         │ Device ID + Session Token
         ↓
┌─────────────────────────────────────────┐
│     Hanzo AI Gateway (Standalone)       │
│     Running on: gateway.hanzo.ai        │
│                                         │
│  ┌───────────────────────────────────┐ │
│  │  1. Authentication Layer          │ │
│  │     - Device Registration         │ │
│  │     - Session Management          │ │
│  │     - IP Verification             │ │
│  └───────────────────────────────────┘ │
│                                         │
│  ┌───────────────────────────────────┐ │
│  │  2. Rate Limiting Layer           │ │
│  │     - Per Device (100/day)        │ │
│  │     - Per IP (500/day)            │ │
│  │     - Per User (1000/day)         │ │
│  │     - Global Limits               │ │
│  └───────────────────────────────────┘ │
│                                         │
│  ┌───────────────────────────────────┐ │
│  │  3. Cost Control Layer            │ │
│  │     - Daily Budget Limits         │ │
│  │     - Per-Request Cost Tracking   │ │
│  │     - Auto-shutdown on Exceed     │ │
│  └───────────────────────────────────┘ │
│                                         │
│  ┌───────────────────────────────────┐ │
│  │  4. API Key Management            │ │
│  │     - DigitalOcean Keys (yours)   │ │
│  │     - Anthropic Keys (yours)      │ │
│  │     - OpenAI Keys (yours)         │ │
│  │     - Key Rotation                │ │
│  │     - Never Exposed to Users!     │ │
│  └───────────────────────────────────┘ │
└────────┬────────────────────────────────┘
         │
         │ Your API Keys (secured)
         ↓
┌─────────────────────────────────────────┐
│       Inference Providers               │
│                                         │
│  - DigitalOcean (inference.do-ai.run)  │
│  - Anthropic (api.anthropic.com)       │
│  - OpenAI (api.openai.com)             │
└─────────────────────────────────────────┘
```

---

## Key Features

### 🔒 Security
- Device registration required
- IP-based rate limiting
- Session token authentication
- API keys never exposed to clients
- Automatic threat detection

### 📊 Rate Limiting (Multi-Layer)
- **Per Device ID:** 100 requests/day
- **Per IP Address:** 500 requests/day  
- **Per User Account:** 1,000 requests/day
- **Global:** 10,000 requests/day (cost protection)

### 💰 Cost Control
- Real-time cost tracking
- Daily budget limits ($50/day default)
- Auto-shutdown if exceeded
- Cost per model tracking
- Monthly spending reports

### 📱 Device Management
- Device registration flow
- Device approval/revocation
- Per-device quotas
- Suspicious device detection

---

## Project Structure

```
hanzo-gateway/
├── src/
│   ├── server.ts                 # Main server
│   ├── middleware/
│   │   ├── auth.ts               # Device authentication
│   │   ├── ratelimit.ts          # Multi-layer rate limiting
│   │   ├── cost-control.ts       # Budget management
│   │   └── ip-guard.ts           # IP-based protection
│   ├── services/
│   │   ├── device-manager.ts     # Device registration
│   │   ├── api-key-manager.ts    # Your API keys (secure)
│   │   ├── usage-tracker.ts      # Usage analytics
│   │   └── inference-proxy.ts    # Proxy to providers
│   ├── models/
│   │   ├── device.ts             # Device schema
│   │   ├── usage-log.ts          # Usage logs
│   │   └── rate-limit.ts         # Rate limit counters
│   └── config/
│       ├── providers.ts          # Provider configs
│       ├── limits.ts             # Rate limit configs
│       └── costs.ts              # Cost configs
├── docker-compose.yml
├── Dockerfile
├── package.json
└── README.md
```

---

## Implementation

### 1. Main Server (`src/server.ts`)

```typescript
import { Hono } from 'hono';
import { cors } from 'hono/cors';
import { logger } from 'hono/logger';
import { authMiddleware } from './middleware/auth';
import { rateLimitMiddleware } from './middleware/ratelimit';
import { costControlMiddleware } from './middleware/cost-control';
import { ipGuardMiddleware } from './middleware/ip-guard';
import { InferenceProxy } from './services/inference-proxy';

const app = new Hono();

// Middleware stack
app.use('*', logger());
app.use('*', cors({
  origin: ['https://app.hanzo.ai', 'tauri://localhost'],
  credentials: true
}));

// Health check (no auth required)
app.get('/health', (c) => c.json({ status: 'healthy', version: '1.0.0' }));

// Protected routes require authentication
app.use('/v1/*', ipGuardMiddleware);      // 1. Check IP
app.use('/v1/*', authMiddleware);          // 2. Authenticate device
app.use('/v1/*', rateLimitMiddleware);     // 3. Check rate limits
app.use('/v1/*', costControlMiddleware);   // 4. Check budget

// Inference endpoints
app.post('/v1/chat/completions', async (c) => {
  const deviceId = c.get('deviceId');
  const userId = c.get('userId');
  const body = await c.req.json();
  
  const proxy = new InferenceProxy();
  return await proxy.handleRequest({
    deviceId,
    userId,
    model: body.model,
    messages: body.messages,
    maxTokens: body.max_tokens,
    temperature: body.temperature
  });
});

// Device management endpoints
app.post('/v1/devices/register', async (c) => {
  const { deviceName, platform, version } = await c.req.json();
  const ip = c.req.header('x-real-ip') || c.req.header('x-forwarded-for');
  
  const deviceManager = new DeviceManager();
  const device = await deviceManager.registerDevice({
    deviceName,
    platform,
    version,
    ipAddress: ip
  });
  
  return c.json({
    deviceId: device.id,
    sessionToken: device.sessionToken,
    status: 'pending_approval', // Manual approval required
    message: 'Device registered. Awaiting approval from admin.'
  });
});

// Usage stats (for user dashboard)
app.get('/v1/usage/stats', authMiddleware, async (c) => {
  const deviceId = c.get('deviceId');
  const userId = c.get('userId');
  
  const usageTracker = new UsageTracker();
  const stats = await usageTracker.getStats({ deviceId, userId });
  
  return c.json(stats);
});

export default app;
```

### 2. Device Authentication (`src/middleware/auth.ts`)

```typescript
import { createMiddleware } from 'hono/factory';
import { DeviceManager } from '../services/device-manager';

export const authMiddleware = createMiddleware(async (c, next) => {
  const deviceId = c.req.header('X-Device-ID');
  const sessionToken = c.req.header('X-Session-Token');
  
  if (!deviceId || !sessionToken) {
    return c.json({ 
      error: 'Missing authentication headers',
      required: ['X-Device-ID', 'X-Session-Token']
    }, 401);
  }
  
  const deviceManager = new DeviceManager();
  const device = await deviceManager.validateDevice(deviceId, sessionToken);
  
  if (!device) {
    return c.json({ error: 'Invalid or revoked device' }, 401);
  }
  
  if (device.status !== 'approved') {
    return c.json({ 
      error: 'Device not approved',
      status: device.status,
      message: 'Contact support for device approval'
    }, 403);
  }
  
  // Check if device is suspended
  if (device.suspendedUntil && new Date() < device.suspendedUntil) {
    return c.json({
      error: 'Device temporarily suspended',
      reason: device.suspensionReason,
      suspendedUntil: device.suspendedUntil
    }, 403);
  }
  
  // Store for downstream middleware
  c.set('deviceId', device.id);
  c.set('userId', device.userId);
  c.set('deviceTier', device.tier);
  
  await next();
});
```

### 3. Multi-Layer Rate Limiting (`src/middleware/ratelimit.ts`)

```typescript
import { createMiddleware } from 'hono/factory';
import { Redis } from 'ioredis';

const redis = new Redis(process.env.REDIS_URL);

// Rate limit configurations
const RATE_LIMITS = {
  free: {
    perDevice: { requests: 100, window: 86400 },      // 100/day
    perIP: { requests: 500, window: 86400 },          // 500/day
    perUser: { requests: 1000, window: 86400 },       // 1000/day
    perMinute: { requests: 5, window: 60 }            // 5/min burst protection
  },
  paid: {
    perDevice: { requests: 10000, window: 86400 },
    perIP: { requests: 50000, window: 86400 },
    perUser: { requests: 100000, window: 86400 },
    perMinute: { requests: 100, window: 60 }
  },
  global: {
    requests: 10000,
    window: 86400,
    costLimit: 50.00  // Max $50/day across all users
  }
};

async function checkRateLimit(
  key: string, 
  limit: number, 
  window: number
): Promise<{ allowed: boolean; remaining: number; resetAt: number }> {
  const now = Date.now();
  const windowStart = now - (window * 1000);
  
  // Remove old entries
  await redis.zremrangebyscore(key, 0, windowStart);
  
  // Count current requests
  const count = await redis.zcard(key);
  
  if (count >= limit) {
    const oldestEntry = await redis.zrange(key, 0, 0, 'WITHSCORES');
    const resetAt = parseInt(oldestEntry[1]) + (window * 1000);
    
    return { 
      allowed: false, 
      remaining: 0, 
      resetAt 
    };
  }
  
  // Add new request
  await redis.zadd(key, now, `${now}-${Math.random()}`);
  await redis.expire(key, window);
  
  return { 
    allowed: true, 
    remaining: limit - count - 1,
    resetAt: now + (window * 1000)
  };
}

export const rateLimitMiddleware = createMiddleware(async (c, next) => {
  const deviceId = c.get('deviceId');
  const userId = c.get('userId');
  const tier = c.get('deviceTier') || 'free';
  const ip = c.req.header('x-real-ip') || c.req.header('x-forwarded-for');
  
  const limits = RATE_LIMITS[tier];
  
  // Check 1: Per-Device Limit
  const deviceLimit = await checkRateLimit(
    `ratelimit:device:${deviceId}`,
    limits.perDevice.requests,
    limits.perDevice.window
  );
  
  if (!deviceLimit.allowed) {
    return c.json({
      error: 'Device rate limit exceeded',
      limit: limits.perDevice.requests,
      window: '24 hours',
      resetAt: deviceLimit.resetAt,
      upgradeUrl: 'https://hanzo.ai/pricing'
    }, 429);
  }
  
  // Check 2: Per-IP Limit
  const ipLimit = await checkRateLimit(
    `ratelimit:ip:${ip}`,
    limits.perIP.requests,
    limits.perIP.window
  );
  
  if (!ipLimit.allowed) {
    return c.json({
      error: 'IP rate limit exceeded. Multiple devices detected.',
      limit: limits.perIP.requests,
      resetAt: ipLimit.resetAt
    }, 429);
  }
  
  // Check 3: Per-User Limit
  const userLimit = await checkRateLimit(
    `ratelimit:user:${userId}`,
    limits.perUser.requests,
    limits.perUser.window
  );
  
  if (!userLimit.allowed) {
    return c.json({
      error: 'User rate limit exceeded',
      limit: limits.perUser.requests,
      resetAt: userLimit.resetAt
    }, 429);
  }
  
  // Check 4: Burst Protection (per minute)
  const burstLimit = await checkRateLimit(
    `ratelimit:burst:${deviceId}`,
    limits.perMinute.requests,
    limits.perMinute.window
  );
  
  if (!burstLimit.allowed) {
    return c.json({
      error: 'Too many requests. Please slow down.',
      limit: limits.perMinute.requests,
      window: '1 minute',
      resetAt: burstLimit.resetAt
    }, 429);
  }
  
  // Check 5: Global Cost Limit
  const globalCost = await redis.get('global:cost:today') || '0';
  if (parseFloat(globalCost) >= RATE_LIMITS.global.costLimit) {
    return c.json({
      error: 'Service temporarily unavailable. Daily budget exceeded.',
      message: 'Please try again tomorrow or contact support.'
    }, 503);
  }
  
  // Add rate limit headers
  c.header('X-RateLimit-Limit-Device', limits.perDevice.requests.toString());
  c.header('X-RateLimit-Remaining-Device', deviceLimit.remaining.toString());
  c.header('X-RateLimit-Reset-Device', new Date(deviceLimit.resetAt).toISOString());
  
  c.header('X-RateLimit-Limit-IP', limits.perIP.requests.toString());
  c.header('X-RateLimit-Remaining-IP', ipLimit.remaining.toString());
  
  await next();
});
```

### 4. Cost Control (`src/middleware/cost-control.ts`)

```typescript
import { createMiddleware } from 'hono/factory';
import { Redis } from 'ioredis';

const redis = new Redis(process.env.REDIS_URL);

// Model pricing (approximate, in USD per 1M tokens)
const MODEL_COSTS = {
  // DigitalOcean open source
  'llama3.3-70b-instruct': { input: 0.50, output: 0.50 },
  'llama3-8b-instruct': { input: 0.10, output: 0.10 },
  'deepseek-r1-distill-llama-70b': { input: 0.50, output: 0.50 },
  
  // Commercial (approximate)
  'anthropic-claude-4-sonnet': { input: 3.00, output: 15.00 },
  'openai-gpt-5': { input: 5.00, output: 15.00 },
  'openai-gpt-4o': { input: 2.50, output: 10.00 }
};

export const costControlMiddleware = createMiddleware(async (c, next) => {
  const body = await c.req.json();
  const model = body.model;
  
  // Get model cost
  const costs = MODEL_COSTS[model] || { input: 1.00, output: 1.00 };
  
  // Estimate cost (conservative)
  const estimatedInputTokens = JSON.stringify(body.messages).length / 4;
  const estimatedOutputTokens = body.max_tokens || 1000;
  
  const estimatedCost = 
    (estimatedInputTokens / 1000000 * costs.input) +
    (estimatedOutputTokens / 1000000 * costs.output);
  
  // Check daily budget
  const dailyBudget = parseFloat(process.env.DAILY_BUDGET_USD || '50');
  const currentSpend = parseFloat(await redis.get('global:cost:today') || '0');
  
  if (currentSpend + estimatedCost > dailyBudget) {
    // Send alert
    await sendAlert({
      type: 'BUDGET_EXCEEDED',
      currentSpend,
      dailyBudget,
      message: 'Daily budget exceeded. Service auto-paused.'
    });
    
    return c.json({
      error: 'Daily budget limit reached',
      message: 'Service temporarily paused. Will resume tomorrow.',
      currentSpend: currentSpend.toFixed(2),
      dailyBudget: dailyBudget.toFixed(2)
    }, 503);
  }
  
  // Store estimated cost for tracking
  c.set('estimatedCost', estimatedCost);
  c.set('modelCosts', costs);
  
  await next();
});
```

### 5. Device Manager (`src/services/device-manager.ts`)

```typescript
import { db } from '../db';
import { generateSessionToken, hashToken } from '../utils/crypto';

export class DeviceManager {
  async registerDevice(data: {
    deviceName: string;
    platform: string;
    version: string;
    ipAddress: string;
  }) {
    // Check if device already exists
    const existing = await db.devices.findOne({
      where: {
        deviceName: data.deviceName,
        ipAddress: data.ipAddress
      }
    });
    
    if (existing) {
      return existing;
    }
    
    // Generate session token
    const sessionToken = generateSessionToken();
    const hashedToken = hashToken(sessionToken);
    
    // Create new device (pending approval)
    const device = await db.devices.create({
      deviceName: data.deviceName,
      platform: data.platform,
      version: data.version,
      ipAddress: data.ipAddress,
      sessionTokenHash: hashedToken,
      status: 'pending_approval',
      tier: 'free',
      createdAt: new Date()
    });
    
    // Send notification to admin
    await sendAdminNotification({
      type: 'NEW_DEVICE_REGISTRATION',
      device: {
        id: device.id,
        name: data.deviceName,
        platform: data.platform,
        ip: data.ipAddress
      },
      approvalUrl: `https://admin.hanzo.ai/devices/${device.id}/approve`
    });
    
    // Return with plain session token (only time it's visible)
    return {
      ...device,
      sessionToken  // Only sent once!
    };
  }
  
  async validateDevice(deviceId: string, sessionToken: string) {
    const hashedToken = hashToken(sessionToken);
    
    const device = await db.devices.findOne({
      where: {
        id: deviceId,
        sessionTokenHash: hashedToken
      }
    });
    
    if (!device) {
      return null;
    }
    
    // Update last seen
    await db.devices.update({
      where: { id: deviceId },
      data: { lastSeenAt: new Date() }
    });
    
    return device;
  }
  
  async approveDevice(deviceId: string, adminId: string) {
    return await db.devices.update({
      where: { id: deviceId },
      data: {
        status: 'approved',
        approvedBy: adminId,
        approvedAt: new Date()
      }
    });
  }
  
  async suspendDevice(deviceId: string, reason: string, durationHours: number = 24) {
    const suspendedUntil = new Date();
    suspendedUntil.setHours(suspendedUntil.getHours() + durationHours);
    
    return await db.devices.update({
      where: { id: deviceId },
      data: {
        status: 'suspended',
        suspendedUntil,
        suspensionReason: reason
      }
    });
  }
}
```

### 6. API Key Manager (`src/services/api-key-manager.ts`)

```typescript
import { SecretsManager } from 'aws-sdk';

// NEVER store API keys in code or env files in production!
// Use AWS Secrets Manager, HashiCorp Vault, or similar

export class APIKeyManager {
  private secretsManager: SecretsManager;
  private cache: Map<string, { key: string; expiresAt: number }>;
  
  constructor() {
    this.secretsManager = new SecretsManager({
      region: process.env.AWS_REGION || 'us-east-1'
    });
    this.cache = new Map();
  }
  
  async getKey(provider: 'digitalocean' | 'anthropic' | 'openai'): Promise<string> {
    // Check cache first (5 min TTL)
    const cached = this.cache.get(provider);
    if (cached && cached.expiresAt > Date.now()) {
      return cached.key;
    }
    
    // Fetch from secrets manager
    const secretName = `hanzo-gateway/${provider}/api-key`;
    
    const secret = await this.secretsManager.getSecretValue({
      SecretId: secretName
    }).promise();
    
    const key = secret.SecretString;
    
    // Cache for 5 minutes
    this.cache.set(provider, {
      key,
      expiresAt: Date.now() + (5 * 60 * 1000)
    });
    
    return key;
  }
  
  async rotateKey(provider: string, newKey: string) {
    const secretName = `hanzo-gateway/${provider}/api-key`;
    
    await this.secretsManager.updateSecret({
      SecretId: secretName,
      SecretString: newKey
    }).promise();
    
    // Invalidate cache
    this.cache.delete(provider);
    
    console.log(`[API Key Manager] Rotated key for ${provider}`);
  }
}
```

---

## Database Schema

```sql
-- Device registry
CREATE TABLE devices (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  device_name VARCHAR(255) NOT NULL,
  platform VARCHAR(50) NOT NULL,
  version VARCHAR(50) NOT NULL,
  ip_address VARCHAR(45) NOT NULL,
  session_token_hash VARCHAR(255) NOT NULL,
  status VARCHAR(20) DEFAULT 'pending_approval', -- pending_approval, approved, suspended, revoked
  tier VARCHAR(20) DEFAULT 'free', -- free, paid
  created_at TIMESTAMP DEFAULT NOW(),
  approved_at TIMESTAMP,
  approved_by UUID,
  last_seen_at TIMESTAMP,
  suspended_until TIMESTAMP,
  suspension_reason TEXT,
  user_id UUID,
  UNIQUE(device_name, ip_address)
);

-- Usage logs
CREATE TABLE usage_logs (
  id SERIAL PRIMARY KEY,
  device_id UUID REFERENCES devices(id),
  user_id UUID,
  model VARCHAR(100),
  tokens_input INTEGER,
  tokens_output INTEGER,
  tokens_total INTEGER,
  cost_usd DECIMAL(10, 6),
  duration_ms INTEGER,
  ip_address VARCHAR(45),
  created_at TIMESTAMP DEFAULT NOW()
);

-- Daily cost tracking
CREATE TABLE daily_costs (
  date DATE PRIMARY KEY,
  total_requests INTEGER DEFAULT 0,
  total_tokens INTEGER DEFAULT 0,
  total_cost_usd DECIMAL(10, 2) DEFAULT 0,
  by_model JSONB DEFAULT '{}',
  by_tier JSONB DEFAULT '{}'
);

CREATE INDEX idx_devices_status ON devices(status);
CREATE INDEX idx_devices_user ON devices(user_id);
CREATE INDEX idx_usage_device ON usage_logs(device_id, created_at);
CREATE INDEX idx_usage_date ON usage_logs(created_at);
```

---

## Deployment

### Docker Compose (`docker-compose.yml`)

```yaml
version: '3.8'

services:
  gateway:
    build: .
    ports:
      - "3000:3000"
    environment:
      - NODE_ENV=production
      - PORT=3000
      - REDIS_URL=redis://redis:6379
      - DATABASE_URL=postgresql://postgres:password@db:5432/hanzo_gateway
      - AWS_REGION=us-east-1
      - DAILY_BUDGET_USD=50
    depends_on:
      - redis
      - db
    restart: unless-stopped
  
  redis:
    image: redis:7-alpine
    volumes:
      - redis-data:/data
    restart: unless-stopped
  
  db:
    image: postgres:16-alpine
    environment:
      - POSTGRES_DB=hanzo_gateway
      - POSTGRES_PASSWORD=password
    volumes:
      - postgres-data:/var/lib/postgresql/data
    restart: unless-stopped
  
  nginx:
    image: nginx:alpine
    ports:
      - "80:80"
      - "443:443"
    volumes:
      - ./nginx.conf:/etc/nginx/nginx.conf
      - /etc/letsencrypt:/etc/letsencrypt
    depends_on:
      - gateway
    restart: unless-stopped

volumes:
  redis-data:
  postgres-data:
```

### Nginx Configuration (`nginx.conf`)

```nginx
http {
  upstream gateway {
    server gateway:3000;
  }
  
  # Rate limiting zones
  limit_req_zone $binary_remote_addr zone=global:10m rate=100r/s;
  limit_req_zone $http_x_device_id zone=device:10m rate=10r/s;
  
  server {
    listen 443 ssl http2;
    server_name gateway.hanzo.ai;
    
    ssl_certificate /etc/letsencrypt/live/gateway.hanzo.ai/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/gateway.hanzo.ai/privkey.pem;
    
    # Apply rate limits
    limit_req zone=global burst=200 nodelay;
    limit_req zone=device burst=20 nodelay;
    
    location / {
      proxy_pass http://gateway;
      proxy_http_version 1.1;
      
      # Pass real IP
      proxy_set_header X-Real-IP $remote_addr;
      proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
      proxy_set_header X-Forwarded-Proto $scheme;
      
      # Timeout settings
      proxy_connect_timeout 30s;
      proxy_send_timeout 30s;
      proxy_read_timeout 90s;
    }
  }
}
```

---

## Client Configuration (hanzo-desktop)

```rust
// /hanzo-desktop/src/config/gateway.rs

pub struct GatewayConfig {
    pub endpoint: String,
    pub device_id: String,
    pub session_token: String,
}

impl GatewayConfig {
    pub fn new() -> Self {
        // Load from secure storage
        let device_id = keyring::get("hanzo.device_id")
            .unwrap_or_else(|| register_device());
        
        let session_token = keyring::get("hanzo.session_token")
            .expect("Session token not found. Please register device.");
        
        Self {
            endpoint: "https://gateway.hanzo.ai".to_string(),
            device_id,
            session_token,
        }
    }
    
    pub async fn make_request(&self, body: serde_json::Value) -> Result<Response> {
        let client = reqwest::Client::new();
        
        let response = client
            .post(format!("{}/v1/chat/completions", self.endpoint))
            .header("X-Device-ID", &self.device_id)
            .header("X-Session-Token", &self.session_token)
            .json(&body)
            .send()
            .await?;
        
        // Check rate limit headers
        if let Some(remaining) = response.headers().get("X-RateLimit-Remaining-Device") {
            let count: u32 = remaining.to_str()?.parse()?;
            if count < 10 {
                show_low_quota_warning(count);
            }
        }
        
        Ok(response)
    }
}

// Device registration flow
async fn register_device() -> String {
    let client = reqwest::Client::new();
    
    let response = client
        .post("https://gateway.hanzo.ai/v1/devices/register")
        .json(&json!({
            "deviceName": get_device_name(),
            "platform": std::env::consts::OS,
            "version": env!("CARGO_PKG_VERSION")
        }))
        .send()
        .await
        .expect("Failed to register device");
    
    let data: serde_json::Value = response.json().await?;
    
    // Save credentials
    keyring::set("hanzo.device_id", &data["deviceId"]);
    keyring::set("hanzo.session_token", &data["sessionToken"]);
    
    // Show approval message
    show_message(&format!(
        "Device registered! Status: {}. {}",
        data["status"],
        data["message"]
    ));
    
    data["deviceId"].as_str().unwrap().to_string()
}
```

---

## Admin Dashboard

### Device Approval UI

```tsx
// Admin dashboard for approving devices
function DeviceApprovalQueue() {
  const { data: pendingDevices } = useQuery('pending-devices', () =>
    fetch('https://gateway.hanzo.ai/admin/devices/pending').then(r => r.json())
  );
  
  const approveMutation = useMutation(
    (deviceId: string) =>
      fetch(`https://gateway.hanzo.ai/admin/devices/${deviceId}/approve`, {
        method: 'POST'
      })
  );
  
  return (
    <div>
      <h2>Pending Device Approvals ({pendingDevices?.length || 0})</h2>
      
      {pendingDevices?.map(device => (
        <div key={device.id} className="device-card">
          <h3>{device.deviceName}</h3>
          <p>Platform: {device.platform}</p>
          <p>IP: {device.ipAddress}</p>
          <p>Registered: {formatDate(device.createdAt)}</p>
          
          <button onClick={() => approveMutation.mutate(device.id)}>
            Approve
          </button>
          <button onClick={() => rejectDevice(device.id)}>
            Reject
          </button>
        </div>
      ))}
    </div>
  );
}
```

---

## Monitoring & Alerts

```typescript
// /src/services/monitoring.ts

import { Slack } from '@slack/web-api';

const slack = new Slack(process.env.SLACK_BOT_TOKEN);

export async function sendAlert(alert: {
  type: string;
  message: string;
  data?: any;
}) {
  // Log to console
  console.error(`[ALERT] ${alert.type}: ${alert.message}`, alert.data);
  
  // Send to Slack
  await slack.chat.postMessage({
    channel: '#hanzo-alerts',
    text: `🚨 *${alert.type}*\n${alert.message}`,
    blocks: [
      {
        type: 'section',
        text: {
          type: 'mrkdwn',
          text: `🚨 *${alert.type}*\n${alert.message}`
        }
      },
      alert.data && {
        type: 'section',
        fields: Object.entries(alert.data).map(([key, value]) => ({
          type: 'mrkdwn',
          text: `*${key}:*\n${value}`
        }))
      }
    ].filter(Boolean)
  });
  
  // If budget exceeded, pause service
  if (alert.type === 'BUDGET_EXCEEDED') {
    await redis.set('service:paused', 'true', 'EX', 86400);
  }
}

// Cost monitoring cron job
setInterval(async () => {
  const cost = await redis.get('global:cost:today') || '0';
  const budget = parseFloat(process.env.DAILY_BUDGET_USD || '50');
  const percentage = (parseFloat(cost) / budget) * 100;
  
  if (percentage >= 80) {
    await sendAlert({
      type: 'BUDGET_WARNING',
      message: `Daily spending at ${percentage.toFixed(1)}% of budget`,
      data: {
        current: `$${cost}`,
        budget: `$${budget}`,
        remaining: `$${(budget - parseFloat(cost)).toFixed(2)}`
      }
    });
  }
}, 3600000); // Check every hour
```

---

## Security Checklist

- [ ] API keys stored in AWS Secrets Manager (not env vars)
- [ ] Device registration requires approval
- [ ] Session tokens are hashed in database
- [ ] IP-based rate limiting enabled
- [ ] Per-device quotas enforced
- [ ] Global cost limits configured
- [ ] SSL/TLS certificates installed
- [ ] Rate limit headers in responses
- [ ] Anomaly detection enabled
- [ ] Admin dashboard requires 2FA
- [ ] Audit logs for all device actions
- [ ] Automatic key rotation scheduled

---

## Cost Optimization

### Tips:
1. **Use free tier models** (8B) for most users
2. **Set aggressive max_tokens** limits
3. **Cache common responses**
4. **Monitor per-model costs**
5. **Auto-downgrade abusive users**
6. **Batch requests when possible**
7. **Use smaller context windows**

### Expected Costs:
- **100 free users:** $15-50/month
- **1,000 free users:** $150-500/month  
- **10,000 free users:** $1,500-5,000/month

With DigitalOcean credits and paid tier revenue, this is sustainable!

---

## Next Steps

1. **Deploy gateway service** on DigitalOcean Droplet
2. **Configure AWS Secrets Manager** for API keys
3. **Set up database and Redis**
4. **Deploy with Docker Compose**
5. **Configure DNS:** gateway.hanzo.ai
6. **Install SSL certificate**
7. **Create admin dashboard**
8. **Test device registration flow**
9. **Monitor costs and optimize**

---

**Gateway Status:** 📋 Ready to deploy!

This standalone service gives you FULL control over your API keys and costs while providing a great free tier to users!
