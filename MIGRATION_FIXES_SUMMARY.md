# Shinkai → Hanzo Migration Fixes - Summary

## Date: 2025-10-26

## Changes Applied

### 1. hanzo-desktop Configuration Updates

**File:** `/hanzo-desktop/apps/hanzo-desktop/src-tauri/src/local_hanzo_node/hanzo_node_options.rs`

#### Changes Made:
1. **Updated inference URLs** (lines 221-223):
   - ❌ Old: `https://api.shinkai.com/inference`
   - ✅ New: `https://inference.do-ai.run` (DigitalOcean Gradient AI Platform)

2. **Updated agent names** (line 224):
   - ❌ Old: `hanzo_free_trial,hanzo_code_gen`
   - ✅ New: `do_llama33_70b,do_llama31_8b`

3. **Updated agent models** (lines 225-227):
   - ❌ Old: `hanzo-backend:FREE_TEXT_INFERENCE,hanzo-backend:CODE_GENERATOR`
   - ✅ New: `openai:llama3.3-70b-instruct,openai:llama3.1-8b-instruct`

4. **Updated API keys placeholder** (line 228):
   - ❌ Old: `'',''` (empty strings)
   - ✅ New: `YOUR_DO_MODEL_ACCESS_KEY,YOUR_DO_MODEL_ACCESS_KEY` (clear placeholder)

5. **Updated store URL** (line 237):
   - ❌ Old: `https://store-api.shinkai.com`
   - ✅ New: `https://store-api.hanzo.ai`

### 2. hanzo-node Cargo.toml Fixes

#### Workspace Cargo.toml Updates:
**File:** `/hanzo-node/Cargo.toml`

- ✅ Line 82: Removed redundant `shinkai_tools_runner = "1.0.0"` entry
- ✅ Line 45 already correctly defines: `hanzo_tools_runner = { path = "./hanzo-libs/hanzo-tools-runner" }`

#### Binary Cargo.toml Updates:
**File:** `/hanzo-node/hanzo-bin/hanzo-node/Cargo.toml`

- ✅ Line 11: Changed `shinkai_tools_runner` → `hanzo_tools_runner` (build-dependencies)
- ✅ Line 83: Changed `shinkai_tools_runner` → `hanzo_tools_runner` (dependencies)

#### Library Cargo.toml Files:
These were already correctly updated in a previous migration:
- ✅ `/hanzo-node/hanzo-libs/hanzo-tools-primitives/Cargo.toml` - already uses `hanzo_tools_runner`
- ✅ `/hanzo-node/hanzo-libs/hanzo-non-rust-code/Cargo.toml` - already uses `hanzo_tools_runner`

### 3. Environment Variable Updates

**File:** `/hanzo-node/scripts/run_all_localhost.sh`

Updated environment variable names (lines 21-22):
- ❌ Old: `SHINKAI_TOOLS_RUNNER_DENO_BINARY_PATH`
- ✅ New: `HANZO_TOOLS_RUNNER_DENO_BINARY_PATH`

- ❌ Old: `SHINKAI_TOOLS_RUNNER_UV_BINARY_PATH`  
- ✅ New: `HANZO_TOOLS_RUNNER_UV_BINARY_PATH`

### 4. Cleanup Actions

- ✅ Deleted `/hanzo-node/hanzo-libs/hanzo-tools-runner/Cargo.toml.orig` (old backup file)
- ✅ Ran `cargo clean` to remove old build artifacts

## Remaining Build Artifacts (Auto-Generated - Will Update on Next Build)

These files contain old references but are auto-generated and will be updated on next build:
- `/hanzo-node/Cargo.lock` - Will regenerate with correct `hanzo_tools_runner` references
- `/hanzo-node/hanzo-libs/hanzo-tools-runner/Cargo.lock` - Will regenerate
- `/hanzo-node/target/*` - Build artifacts (cleaned)

## DigitalOcean Inference Setup

A comprehensive setup guide has been created:
- **File:** `/hanzo-node/DIGITALOCEAN_INFERENCE_SETUP.md`
- **Contents:**
  - How to get DigitalOcean Model Access Keys
  - Configuration for both direct integration and cloud node proxy
  - Environment variable setup
  - Production deployment guide (Droplet + Nginx)
  - Cost management and security best practices
  - Troubleshooting guide

## Next Steps

### 1. Set Up DigitalOcean Model Access Key

```bash
# Go to DigitalOcean Control Panel
https://cloud.digitalocean.com/ai/serverless-inference

# Create a Model Access Key
# Copy the secret key (shown only once!)
```

### 2. Configure hanzo-desktop for Development

Edit the API key placeholder in `/hanzo-desktop/apps/hanzo-desktop/src-tauri/src/local_hanzo_node/hanzo_node_options.rs`:

```rust
initial_agent_api_keys: Some("your_actual_do_key_here,your_actual_do_key_here".to_string()),
```

**IMPORTANT:** Do NOT commit the actual API key! Use environment variables in production.

### 3. Test the Setup

```bash
# Terminal 1: Start hanzo-node
cd /Users/z/work/shinkai/hanzo-node
cargo run --bin hanzo_node

# Terminal 2: Start hanzo-desktop  
cd /Users/z/work/shinkai/hanzo-desktop
npm run dev  # or appropriate command

# Test inference
curl https://inference.do-ai.run/v1/chat/completions \
  -H "Authorization: Bearer YOUR_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "llama3.3-70b-instruct",
    "messages": [{"role": "user", "content": "Hello!"}],
    "max_tokens": 50
  }'
```

### 4. Production Deployment (Optional)

For production `api.hanzo.ai` setup, follow the detailed guide in:
`/hanzo-node/DIGITALOCEAN_INFERENCE_SETUP.md` (Section 5: "Set Up Cloud Node at api.hanzo.ai")

## Verification Checklist

- ✅ All `shinkai_tools_runner` references changed to `hanzo_tools_runner`
- ✅ All `api.shinkai.com` URLs changed to `inference.do-ai.run`
- ✅ Store URL changed from `store-api.shinkai.com` to `store-api.hanzo.ai`
- ✅ Environment variables renamed from `SHINKAI_TOOLS_RUNNER_*` to `HANZO_TOOLS_RUNNER_*`
- ✅ Backup files removed
- ✅ Build artifacts cleaned
- ⏳ **TODO:** Rebuild to verify no compilation errors
- ⏳ **TODO:** Test hanzo-desktop integration with DigitalOcean inference API
- ⏳ **TODO:** Set up actual DigitalOcean Model Access Key

## Known Issues

### API Key Security

The current configuration in `hanzo_node_options.rs` has a hardcoded placeholder:
```rust
initial_agent_api_keys: Some("YOUR_DO_MODEL_ACCESS_KEY,YOUR_DO_MODEL_ACCESS_KEY".to_string()),
```

**Recommendation:** Update hanzo-desktop to read API keys from:
1. Environment variables (for development)
2. Secure keychain/secrets manager (for production)
3. User configuration file (encrypted)

### Model Compatibility

The default models are set to:
- `llama3.3-70b-instruct` (70B parameter model - higher quality, slower)
- `llama3.1-8b-instruct` (8B parameter model - faster, lower cost)

**Note:** Verify these models are available in your DigitalOcean region by calling:
```bash
curl https://inference.do-ai.run/v1/models \
  -H "Authorization: Bearer YOUR_KEY"
```

## Files Modified

### hanzo-node:
1. `/Cargo.toml` - Removed redundant workspace dependency
2. `/hanzo-bin/hanzo-node/Cargo.toml` - Updated tool runner references (2 locations)
3. `/scripts/run_all_localhost.sh` - Updated environment variable names
4. `/DIGITALOCEAN_INFERENCE_SETUP.md` - ✨ NEW comprehensive setup guide

### hanzo-desktop:
1. `/apps/hanzo-desktop/src-tauri/src/local_hanzo_node/hanzo_node_options.rs` - Updated all URLs, models, and API keys

## Testing Required

1. **Build Test:**
   ```bash
   cd /Users/z/work/shinkai/hanzo-node
   cargo build --release
   ```

2. **Run Test:**
   ```bash
   cargo run --bin hanzo_node
   ```

3. **Desktop Integration Test:**
   ```bash
   cd /Users/z/work/shinkai/hanzo-desktop
   # Build and run hanzo-desktop
   # Verify it connects to DigitalOcean API
   ```

4. **Inference Test:**
   ```bash
   # Test direct API call
   curl https://inference.do-ai.run/v1/chat/completions \
     -H "Authorization: Bearer YOUR_KEY" \
     -H "Content-Type: application/json" \
     -d '{"model": "llama3.3-70b-instruct", "messages": [{"role": "user", "content": "test"}]}'
   ```

## Documentation Created

1. **DIGITALOCEAN_INFERENCE_SETUP.md** - Complete guide including:
   - DigitalOcean API setup
   - Local development configuration
   - Production cloud node deployment
   - Security best practices
   - Cost management
   - Troubleshooting

2. **MIGRATION_FIXES_SUMMARY.md** (this file) - Summary of all changes

---

**Migration Status:** ✅ Code Changes Complete | ⏳ Testing Required

**Next Action:** Set up DigitalOcean Model Access Key and test the integration
