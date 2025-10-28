# Inference Infrastructure Migration

## ⚠️ Important Change

The `cloud-node/` directory has been **moved to a separate repository**.

## New Location

🔗 **https://github.com/hanzoai/inference**

## What Moved

The following components are now in the inference repo:

1. **Cloud Node** - Rust-based Hanzo Node with multi-provider AI
2. **Inference Gateway** - Node.js API proxy with rate limiting
3. **Multi-Provider Support** - DigitalOcean, OpenAI, Claude
4. **Docker Compose** - Production deployment
5. **CI/CD Pipeline** - Automated testing
6. **Documentation** - Setup guides and configuration

## Why the Move

- **Separation of Concerns**: Inference infrastructure is now independent
- **Easier Deployment**: Standalone Docker Compose setup
- **Better CI/CD**: Dedicated testing and deployment pipeline
- **Multi-Provider Focus**: Centralized AI provider management
- **Cost Optimization**: Qwen3-32B on DigitalOcean as default

## Migration Guide

### If you were using cloud-node:

1. **Clone new repo**:
   ```bash
   git clone https://github.com/hanzoai/inference.git
   cd inference
   ```

2. **Copy your configuration**:
   ```bash
   # Your old env.conf from hanzo-node/cloud-node/
   cp /old/path/to/env.conf cloud-node/env.conf
   ```

3. **Update API keys**:
   ```bash
   nano cloud-node/env.conf
   # Add DigitalOcean, OpenAI, Claude keys
   ```

4. **Deploy**:
   ```bash
   # Docker Compose (recommended)
   docker-compose up -d
   
   # Or native systemd
   cd cloud-node
   sudo cp env.conf /opt/hanzo-node/
   sudo systemctl restart hanzo-node
   ```

### If you're integrating with hanzo-desktop:

Update your hanzo-desktop configuration to point to the inference service:

```rust
// In hanzo-desktop config
inference_url: "https://inference.hanzo.ai"  // Production
// or
inference_url: "http://localhost:9550"       // Local development
```

## Default Provider Change

### Old (cloud-node):
- Default: OpenAI GPT-4o-mini
- Cost: ~$11/month (1000 req/day)

### New (inference repo):
- **Default: DigitalOcean Qwen3-32B**
- **Cost: ~$23/month (1000 req/day)**
- **Quality: Excellent (32B multilingual model)**
- Alternative: Llama 3.3 70B, GPT-4o, Claude 3.5

## Features Added

✅ **Multi-Provider Support**
- DigitalOcean Gradient AI (40+ models)
- OpenAI (GPT-4o, GPT-4o-mini, o1)
- Anthropic Claude (3.5 Sonnet, Opus)

✅ **Rate Limiting**
- Per device: 100 req/day (free tier)
- Per IP: 500 req/day
- Per user: 1000 req/day

✅ **Cost Controls**
- Daily budget limits ($50 default)
- Real-time tracking
- Auto-pause on overage

✅ **Security**
- Device authentication (JWT)
- API key protection (never exposed)
- Multi-layer security

✅ **Production Ready**
- Docker Compose deployment
- Nginx reverse proxy
- SSL/TLS support
- Comprehensive monitoring

## Quick Start

```bash
# 1. Clone inference repo
git clone https://github.com/hanzoai/inference.git
cd inference

# 2. Configure API keys
cp gateway/.env.example gateway/.env
nano gateway/.env  # Add your DigitalOcean API key

# 3. Update cloud node config
nano cloud-node/env.conf  # Add all provider keys

# 4. Start services
docker-compose up -d

# 5. Test
curl http://localhost:9550/v2/health_check  # Cloud node
curl http://localhost:3001/health            # Gateway
```

## Documentation

📚 Full documentation in inference repo:

- [README.md](https://github.com/hanzoai/inference/blob/main/README.md) - Overview and quick start
- [Cloud Node Setup](https://github.com/hanzoai/inference/blob/main/cloud-node/README.md) - Deploy Hanzo Node
- [Multi-Provider Config](https://github.com/hanzoai/inference/blob/main/cloud-node/PROVIDERS_SETUP.md) - Configure providers
- [Gateway Setup](https://github.com/hanzoai/inference/blob/main/gateway/SETUP.md) - API proxy setup
- [GitHub CI/CD](https://github.com/hanzoai/inference/blob/main/gateway/GITHUB_SETUP.md) - Automated testing

## Support

- **Repo**: https://github.com/hanzoai/inference
- **Issues**: https://github.com/hanzoai/inference/issues
- **Email**: support@hanzo.ai

## FAQs

**Q: Why was this moved?**
A: To enable better separation of concerns, easier deployment, and dedicated focus on multi-provider inference infrastructure.

**Q: Will hanzo-node still work?**
A: Yes! hanzo-node continues to function. The inference capability is now handled by the separate inference service.

**Q: Do I need to migrate immediately?**
A: If you're using cloud-node, yes. If you're just using hanzo-node for blockchain/other features, no migration needed.

**Q: What about my existing configuration?**
A: Copy your `env.conf` to the new repo and update with the new multi-provider format.

**Q: Will the old cloud-node URLs still work?**
A: No. Update your clients to point to the new inference service.

**Q: What's the cost difference?**
A: With Qwen3-32B as default (~$23/month), it's slightly more than GPT-4o-mini (~$11/month) but offers much better quality. Still 10x cheaper than GPT-4o (~$188/month).

---

**Migration Date**: October 28, 2025
**Version**: 1.0.0
