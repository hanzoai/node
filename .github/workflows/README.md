# GitHub Actions Workflows for Hanzo Node

## Crate Publishing Workflows

### 1. `publish-crates.yml` - Automatic Version-Based Publishing

**Trigger**: Automatically runs when Cargo.toml versions change on main branch

This workflow:
- Detects which crates have version bumps
- Publishes only the changed crates
- Runs tests before publishing
- Publishes one crate at a time

**Usage**:
1. Bump version in `hanzo-libs/<crate>/Cargo.toml`
2. Commit and push to main
3. Workflow automatically publishes to crates.io

### 2. `publish-ordered.yml` - Manual Dependency-Ordered Publishing

**Trigger**: Manual workflow dispatch

This workflow:
- Publishes all crates in correct dependency order
- Foundation crates first, then core, services, integration
- Supports dry-run mode for testing

**Usage**:
1. Go to Actions → Publish Crates (Dependency Order)
2. Click "Run workflow"
3. Choose dry-run: true (for testing) or false (for real publish)

## Setup Requirements

### 1. Configure Secrets

Add your crates.io API token to GitHub repository secrets:

1. Get token from https://crates.io/me
2. Go to GitHub repo → Settings → Secrets and variables → Actions
3. Add new secret:
   - Name: `CARGO_REGISTRY_TOKEN`
   - Value: Your crates.io API token

### 2. Grant Workflow Permissions

Ensure GitHub Actions has proper permissions:
1. Go to Settings → Actions → General
2. Under "Workflow permissions"
3. Select "Read and write permissions"

## Publishing Process

### Option A: Automatic (Recommended)

```bash
# Bump version in a crate
cd hanzo-libs/hanzo-messages
# Edit Cargo.toml: version = "1.2.0"

# Commit and push
git add Cargo.toml
git commit -m "chore: bump hanzo-messages to 1.2.0"
git push

# Workflow automatically publishes to crates.io
```

### Option B: Manual Bulk Publishing

Use when publishing multiple crates for the first time:

1. Go to GitHub → Actions → "Publish Crates (Dependency Order)"
2. Click "Run workflow"
3. Select branch: main
4. Set dry-run: false
5. Click "Run workflow"

### Option C: Manual Individual Publishing

```bash
# From local machine
cd hanzo-libs/hanzo-messages
cargo publish --token YOUR_TOKEN
```

## Dependency Order

The workflows publish crates in this order:

1. **Foundation** (no internal deps):
   - hanzo-messages
   - hanzo-embed
   - hanzo-runner
   - hanzo-tools-runner
   - hanzo-models
   - hanzo-model-discovery

2. **Core** (depend on foundation):
   - hanzo-identity
   - hanzo-pqc
   - hanzo-did
   - hanzo-db-sqlite
   - hanzo-runtime
   - hanzo-tools

3. **Services** (depend on core):
   - hanzo-mcp
   - hanzo-fs
   - hanzo-kbs
   - hanzo-llm
   - hanzo-jobs
   - hanzo-database
   - hanzo-wasm
   - hanzo-wasm-runtime

4. **Integration** (depend on services):
   - hanzo-libp2p
   - hanzo-libp2p-relayer
   - hanzo-http-api
   - hanzo-api
   - hanzo-job-queue-manager

## Troubleshooting

### "Already published" errors
Normal - the workflow continues if a version is already on crates.io

### "Missing secret" errors
Configure `CARGO_REGISTRY_TOKEN` in repository secrets

### Publishing failures
- Check the Actions log for specific errors
- Verify crate builds locally: `cargo build --release`
- Test publish locally: `cargo publish --dry-run`

### Dependency errors
Ensure all dependencies are published before dependents
Use the ordered workflow for first-time bulk publishing

## Version Bumping Guidelines

Follow Semantic Versioning (SemVer):
- **Patch** (1.0.0 → 1.0.1): Bug fixes, no API changes
- **Minor** (1.0.0 → 1.1.0): New features, backward compatible
- **Major** (1.0.0 → 2.0.0): Breaking API changes

For the naming migration to v1.2.0:
- This is a **minor** version bump (new naming, backward compatible at code level)
- OR a **major** version bump if you want to emphasize the rebrand
