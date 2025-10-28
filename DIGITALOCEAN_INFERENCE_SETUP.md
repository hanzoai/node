# DigitalOcean Serverless Inference Setup for Hanzo Node

## Overview
This guide shows how to configure Hanzo Node to use DigitalOcean's Gradient™ AI Platform serverless inference API with your DigitalOcean credits.

## Architecture Options

### Option 1: Direct DigitalOcean Integration (Recommended for Development)
```
hanzo-desktop → hanzo-node → https://inference.do-ai.run
```

### Option 2: Cloud Node Proxy (Recommended for Production)
```
hanzo-desktop → api.hanzo.ai (your cloud node) → https://inference.do-ai.run
```

## Setup Steps

### 1. Get Your DigitalOcean Model Access Key

1. Go to [DigitalOcean Control Panel - Serverless Inference](https://cloud.digitalocean.com/ai/serverless-inference)
2. Navigate to **Model Access Keys** section
3. Click **Create model access key**
4. Name it (e.g., "hanzo-production" or "hanzo-dev")
5. **IMPORTANT**: Copy and save the secret key immediately (shown only once)
6. Store it securely (use a secrets manager or secure env var)

### 2. Test the API Connection

```bash
# Test endpoint availability
curl -X GET https://inference.do-ai.run/v1/models \
  -H "Authorization: Bearer YOUR_MODEL_ACCESS_KEY" \
  -H "Content-Type: application/json"

# Test a simple inference request
curl https://inference.do-ai.run/v1/chat/completions \
  -H "Authorization: Bearer YOUR_MODEL_ACCESS_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "llama3.3-70b-instruct",
    "messages": [{"role": "user", "content": "Say hello!"}],
    "temperature": 0.7,
    "max_tokens": 50
  }'
```

### 3. Configure Hanzo Node

#### Option A: Using Environment Variables (Local Development)

Edit your environment file or `scripts/run_all_localhost.sh`:

**Example 1: Open Source Models Only (Recommended for Getting Started)**
```bash
# DigitalOcean Serverless Inference - Open Source Models
export INITIAL_AGENT_NAMES="do_llama33_70b,do_llama31_8b,do_deepseek_r1"
export INITIAL_AGENT_URLS="https://inference.do-ai.run,https://inference.do-ai.run,https://inference.do-ai.run"
export INITIAL_AGENT_MODELS="openai:llama3.3-70b-instruct,openai:llama3-8b-instruct,openai:deepseek-r1-distill-llama-70b"
export INITIAL_AGENT_API_KEYS="YOUR_DO_MODEL_ACCESS_KEY,YOUR_DO_MODEL_ACCESS_KEY,YOUR_DO_MODEL_ACCESS_KEY"
```

**Example 2: Mix of Open Source and Commercial Models**
```bash
# Mixed Configuration - Open Source + Commercial Models
export INITIAL_AGENT_NAMES="do_llama33,do_llama8b,do_claude_sonnet,do_gpt4o"
export INITIAL_AGENT_URLS="https://inference.do-ai.run,https://inference.do-ai.run,https://inference.do-ai.run,https://inference.do-ai.run"
export INITIAL_AGENT_MODELS="openai:llama3.3-70b-instruct,openai:llama3-8b-instruct,anthropic:anthropic-claude-4-sonnet,openai:openai-gpt-4o"
export INITIAL_AGENT_API_KEYS="YOUR_DO_KEY,YOUR_DO_KEY,YOUR_ANTHROPIC_KEY,YOUR_OPENAI_KEY"
```

**Example 3: All Model Types (Maximum Flexibility)**
```bash
# Comprehensive Configuration with Multiple Providers
export INITIAL_AGENT_NAMES="llama33,llama8b,mistral_nemo,deepseek_r1,gpt_oss_120b,claude_sonnet4,gpt5,gpt4o_mini"
export INITIAL_AGENT_URLS="https://inference.do-ai.run,https://inference.do-ai.run,https://inference.do-ai.run,https://inference.do-ai.run,https://inference.do-ai.run,https://inference.do-ai.run,https://inference.do-ai.run,https://inference.do-ai.run"
export INITIAL_AGENT_MODELS="openai:llama3.3-70b-instruct,openai:llama3-8b-instruct,openai:mistral-nemo-instruct-2407,openai:deepseek-r1-distill-llama-70b,openai:openai-gpt-oss-120b,anthropic:anthropic-claude-4-sonnet,openai:openai-gpt-5,openai:openai-gpt-4o-mini"
export INITIAL_AGENT_API_KEYS="YOUR_DO_KEY,YOUR_DO_KEY,YOUR_DO_KEY,YOUR_DO_KEY,YOUR_DO_KEY,YOUR_ANTHROPIC_KEY,YOUR_OPENAI_KEY,YOUR_OPENAI_KEY"
```

**Important Notes:**
- **Open Source models**: Use your DigitalOcean Model Access Key
- **Anthropic models**: Require your Anthropic API key (get from https://console.anthropic.com/)
- **OpenAI commercial models**: Require your OpenAI API key (get from https://platform.openai.com/)
- Model format: `{provider}:{model-id}` (e.g., `openai:llama3.3-70b-instruct`, `anthropic:anthropic-claude-4-sonnet`)
- All models use the same DigitalOcean endpoint (`https://inference.do-ai.run`)
- API keys must match the number of models (comma-separated)

**Available Models on DigitalOcean:**

### Open Source Foundation Models (No API Key Required)
- `llama3.3-70b-instruct` - Meta Llama 3.3 70B (70B params, 128K context) ⭐ **Recommended**
- `llama3-8b-instruct` - Meta Llama 3.1 8B (8B params, 128K context) ⚡ **Fast & Cost-Effective**
- `deepseek-r1-distill-llama-70b` - DeepSeek R1 distill Llama 70B (70B params, 32K context)
- `alibaba-qwen3-32b` - Alibaba Qwen3 32B (32B params, 40K context) *Serverless inference only*
- `mistral-nemo-instruct-2407` - Mistral NeMo 12B (12B params, 128K context)
- `openai-gpt-oss-120b` - OpenAI GPT OSS 120B (117B params, 131K context)
- `openai-gpt-oss-20b` - OpenAI GPT OSS 20B (21B params, 131K context)

⚠️ **Note:** DeepSeek models should use all available guardrails for user-facing agents.

### Commercial Foundation Models (Require Partner Provider API Keys)

**Anthropic Claude Models:**
- `anthropic-claude-4-sonnet` - Claude Sonnet 4 (64K context)
- `anthropic-claude-4-opus` - Claude Opus 4 (32K context)  
- `anthropic-claude-3.7-sonnet` - Claude 3.7 Sonnet (128K context)
- `anthropic-claude-3.5-sonnet` - Claude 3.5 Sonnet (8K context)
- `anthropic-claude-3.5-haiku` - Claude 3.5 Haiku (8K context)
- `anthropic-claude-3-opus` - Claude 3 Opus (4K context)

**OpenAI GPT Models:**
- `openai-gpt-5` - GPT-5 (128K context)
- `openai-gpt-5-mini` - GPT-5 mini (128K context)
- `openai-gpt-5-nano` - GPT-5 nano (128K context)
- `openai-gpt-4.1` - GPT-4.1 (32K context)
- `openai-gpt-4o` - GPT-4o (16K context)
- `openai-gpt-4o-mini` - GPT-4o mini (16K context)
- `openai-o1` - o1 (100K context)
- `openai-o3` - o3 (100K context)
- `openai-o3-mini` - o3-mini (100K context)
- `openai-gpt-image-1` - GPT-image-1 (16K context) - Multimodal

### Serverless Inference-Only Models (Not for Agents)

**Image Generation:**
- `fast-sdxl` - Fast SDXL image generation
- `flux-schnell` - Flux Schnell image generation

**Audio Generation:**
- `stable-audio-25-text-to-audio` - Stable Audio 2.5 (Text-to-Audio)
- `tts-multilingual-v2` - Multilingual Text-to-Speech v2

### Embedding Models (For Knowledge Bases)
- `Alibaba-NLP/gte-large-en-v1.5` - GTE Large EN v1.5 (434M params)
- `sentence-transformers/all-MiniLM-L6-v2` - SBERT MiniLM (22.7M params)
- `sentence-transformers/multi-qa-mpnet-base-dot-v1` - SBERT MPNet (109M params)

📚 For latest models and pricing: https://docs.digitalocean.com/products/ai-ml/details/available-models/

#### Option B: Using hanzo-desktop Configuration

The hanzo-desktop app can be configured to connect to your DigitalOcean-powered node.

### 4. Update hanzo-desktop Configuration

Edit: `/Users/z/work/shinkai/hanzo-desktop/apps/hanzo-desktop/src-tauri/src/local_hanzo_node/hanzo_node_options.rs`

**For local development (direct DigitalOcean):**
```rust
initial_agent_urls: Some(
    "https://inference.do-ai.run,https://inference.do-ai.run".to_string(),
),
initial_agent_names: Some("do_llama33_70b,do_llama31_8b".to_string()),
initial_agent_models: Some(
    "openai:llama3.3-70b-instruct,openai:llama3.1-8b-instruct".to_string(),
),
initial_agent_api_keys: Some("YOUR_DO_KEY,YOUR_DO_KEY".to_string()),
```

**For production (via api.hanzo.ai):**
```rust
initial_agent_urls: Some(
    "https://api.hanzo.ai,https://api.hanzo.ai".to_string(),
),
initial_agent_names: Some("hanzo_llama33,hanzo_llama31".to_string()),
initial_agent_models: Some(
    "openai:llama3.3-70b-instruct,openai:llama3.1-8b-instruct".to_string(),
),
initial_agent_api_keys: Some("YOUR_HANZO_API_KEY,YOUR_HANZO_API_KEY".to_string()),
```

### 5. Set Up Cloud Node at api.hanzo.ai (Production Setup)

#### A. Deploy Hanzo Node on DigitalOcean Droplet

```bash
# 1. Create a Droplet (Ubuntu 24.04 recommended)
# 2. Install dependencies
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
sudo apt update && sudo apt install -y build-essential pkg-config libssl-dev

# 3. Clone and build hanzo-node
git clone https://github.com/your-org/hanzo-node.git
cd hanzo-node
cargo build --release --bin hanzo_node

# 4. Configure environment variables
cat > /etc/hanzo-node/.env <<EOF
GLOBAL_IDENTITY_NAME=@@api.hanzo.ai
NODE_IP=0.0.0.0
NODE_API_IP=0.0.0.0
NODE_API_PORT=9550
NODE_PORT=9552

# DigitalOcean Inference Configuration
INITIAL_AGENT_NAMES=hanzo_llama33,hanzo_llama31
INITIAL_AGENT_URLS=https://inference.do-ai.run,https://inference.do-ai.run
INITIAL_AGENT_MODELS=openai:llama3.3-70b-instruct,openai:llama3.1-8b-instruct
INITIAL_AGENT_API_KEYS=${DO_MODEL_ACCESS_KEY},${DO_MODEL_ACCESS_KEY}

# Other required settings
EMBEDDINGS_SERVER_URL=http://localhost:11435
FIRST_DEVICE_NEEDS_REGISTRATION_CODE=false
LOG_SIMPLE=true
EOF

# 5. Set up systemd service
sudo nano /etc/systemd/system/hanzo-node.service
```

#### B. Create systemd Service

```ini
[Unit]
Description=Hanzo Node - AI Inference Gateway
After=network.target

[Service]
Type=simple
User=hanzo
WorkingDirectory=/opt/hanzo-node
EnvironmentFile=/etc/hanzo-node/.env
ExecStart=/opt/hanzo-node/target/release/hanzo_node
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable hanzo-node
sudo systemctl start hanzo-node
```

#### C. Configure Nginx Reverse Proxy

```nginx
# /etc/nginx/sites-available/api.hanzo.ai
server {
    listen 80;
    server_name api.hanzo.ai;
    
    location / {
        return 301 https://$server_name$request_uri;
    }
}

server {
    listen 443 ssl http2;
    server_name api.hanzo.ai;
    
    ssl_certificate /etc/letsencrypt/live/api.hanzo.ai/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/api.hanzo.ai/privkey.pem;
    
    # Hanzo Node API
    location / {
        proxy_pass http://localhost:9550;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection 'upgrade';
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_cache_bypass $http_upgrade;
    }
    
    # WebSocket support
    location /ws {
        proxy_pass http://localhost:9550;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "Upgrade";
        proxy_set_header Host $host;
    }
}
```

```bash
# Install and configure SSL
sudo apt install certbot python3-certbot-nginx
sudo certbot --nginx -d api.hanzo.ai
sudo systemctl reload nginx
```

### 6. Environment Variables Reference

#### Required Variables
- `INITIAL_AGENT_NAMES` - Comma-separated agent names
- `INITIAL_AGENT_URLS` - Comma-separated base URLs (DO: `https://inference.do-ai.run`)
- `INITIAL_AGENT_MODELS` - Comma-separated model IDs (format: `openai:model-name`)
- `INITIAL_AGENT_API_KEYS` - Comma-separated API keys (DO model access keys)

#### Optional Variables
- `GLOBAL_IDENTITY_NAME` - Node identity (default: `@@localhost.sep-hanzo`)
- `NODE_API_PORT` - API port (default: 9550)
- `EMBEDDINGS_SERVER_URL` - Embedding service URL

### 7. Testing Your Setup

```bash
# Test local hanzo-node connection
curl http://localhost:9550/health

# Test agent availability
curl http://localhost:9550/v2/available_llm_providers

# Test inference (replace with actual endpoint)
curl -X POST http://localhost:9550/v2/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "agent_id": "do_llama33_70b",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## Cost Management

### DigitalOcean Pricing (as of 2025)
- Billed per input/output token
- No upfront costs, no minimum fees
- Use `max_tokens` to control costs
- Monitor usage in DigitalOcean Control Panel

### Best Practices
1. Use smaller models (8B) for simple tasks
2. Set reasonable `max_tokens` limits
3. Implement request caching where possible
4. Monitor token usage via DigitalOcean dashboard

## Security Best Practices

1. **Never commit API keys to git**
   - Use environment variables
   - Use secrets management (HashiCorp Vault, AWS Secrets Manager, 1Password)

2. **Rotate keys regularly**
   - Use the "Regenerate" feature in DigitalOcean Control Panel
   - Update all services using the old key before destroying it

3. **Use separate keys for dev/staging/production**
   - Create different keys for each environment
   - Easy to revoke if one environment is compromised

4. **Implement rate limiting**
   - Protect against excessive usage
   - Prevent abuse

## Troubleshooting

### "invalid authorization header" Error
- Check that your Model Access Key is correct
- Ensure it's properly set in environment variables
- Verify the key hasn't been regenerated or deleted

### Connection Refused
- Verify DigitalOcean endpoint is accessible: `curl https://inference.do-ai.run/v1/models`
- Check firewall rules if using cloud node
- Ensure hanzo-node is running: `systemctl status hanzo-node`

### Model Not Available
- Get list of available models: `curl https://inference.do-ai.run/v1/models`
- Verify model ID matches exactly (case-sensitive)

## Migration from Shinkai.com to Hanzo.ai

If you're migrating from the old Shinkai endpoints:

**Old configuration:**
```rust
"https://api.shinkai.com/inference"
```

**New configuration (DigitalOcean):**
```rust
"https://inference.do-ai.run"
```

**New configuration (your cloud node):**
```rust
"https://api.hanzo.ai"
```

## Additional Resources

- [DigitalOcean Gradient AI Platform Docs](https://docs.digitalocean.com/products/ai-ml/)
- [DigitalOcean Model Access Keys](https://cloud.digitalocean.com/ai/serverless-inference)
- [Available Models](https://docs.digitalocean.com/products/ai-ml/details/available-models/)
- [API Reference](https://docs.digitalocean.com/products/ai-ml/reference/api/)

## Support

For issues with:
- **Hanzo Node**: [GitHub Issues](https://github.com/your-org/hanzo-node/issues)
- **DigitalOcean API**: [DigitalOcean Support](https://docs.digitalocean.com/support/)
- **Cloud Node Setup**: team@hanzo.ai
