# Migration Guide: Hanzo Crates v1.1.10 → v1.1.11

## 🚨 Breaking Change: Crate Renaming

**Date**: November 2025
**Affects**: All Hanzo crates (24 total)
**Breaking**: Yes - requires Cargo.toml updates

---

## What Changed?

All Hanzo crates have been renamed from **snake_case** (`hanzo_*`) to **kebab-case** (`hanzo-*`) to follow idiomatic Rust naming conventions.

### Before (v1.1.10 - DEPRECATED)
```toml
[dependencies]
hanzo_message_primitives = "1.1.10"
hanzo_crypto_identities = "1.1.10"
hanzo_tools_primitives = "1.1.10"
```

### After (v1.1.11 - NEW)
```toml
[dependencies]
hanzo-message-primitives = "1.1.11"
hanzo-crypto-identities = "1.1.11"
hanzo-tools-primitives = "1.1.11"
```

---

## Why This Change?

1. **Idiomatic Rust**: Rust package names conventionally use kebab-case (hyphens)
2. **Consistency**: Aligns with Rust ecosystem standards
3. **Clarity**: Library name (snake_case) vs package name (kebab-case) distinction

**Note**: The actual library names in code remain `snake_case`:
```rust
use hanzo_message_primitives::*; // Still snake_case in code!
```

---

## Complete Renaming List

| Old Name (v1.1.10 - yanked) | New Name (v1.1.11+) |
|-----------------------------|---------------------|
| `hanzo_message_primitives` | `hanzo-message-primitives` |
| `hanzo_crypto_identities` | `hanzo-crypto-identities` |
| `hanzo_libp2p_relayer` | `hanzo-libp2p-relayer` |
| `hanzo_job_queue_manager` | `hanzo-job-queue-manager` |
| `hanzo_fs` | `hanzo-fs` |
| `hanzo_embedding` | `hanzo-embedding` |
| `hanzo_http_api` | `hanzo-http-api` |
| `hanzo_tools_primitives` | `hanzo-tools-primitives` |
| `hanzo_tools_runner` | `hanzo-tools-runner` |
| `hanzo_sqlite` | `hanzo-sqlite` |
| `hanzo_db` | `hanzo-db` |
| `hanzo_hmm` | `hanzo-hmm` |
| `hanzo_non_rust_code` | `hanzo-non-rust-code` |
| `hanzo_mcp` | `hanzo-mcp` |
| `hanzo_pqc` | `hanzo-pqc` |
| `hanzo_kbs` | `hanzo-kbs` |
| `hanzo_did` | `hanzo-did` |
| `hanzo_model_discovery` | `hanzo-model-discovery` |
| `hanzo_config` | `hanzo-config` |
| `hanzo_mining` | `hanzo-mining` |
| `hanzo_wasm_runtime` | `hanzo-wasm-runtime` |
| `hanzo_llm` | `hanzo-llm` |
| `hanzo_sheet` | `hanzo-sheet` |
| `hanzo_runtime_tests` | `hanzo-runtime-tests` |

---

## Migration Steps

### Step 1: Update Cargo.toml

Find and replace all `hanzo_` references with `hanzo-` in your `Cargo.toml`:

```bash
# In your project directory
sed -i '' 's/hanzo_/hanzo-/g' Cargo.toml
```

Or manually update each dependency:

```toml
# Before
[dependencies]
hanzo_message_primitives = "1.1.10"

# After
[dependencies]
hanzo-message-primitives = "1.1.11"
```

### Step 2: Update Version Numbers

Change all Hanzo crate versions from `1.1.10` to `1.1.11`:

```bash
# Automated version update
sed -i '' 's/hanzo-\([a-z_-]*\) = "1.1.10"/hanzo-\1 = "1.1.11"/g' Cargo.toml
```

### Step 3: Clean and Rebuild

```bash
# Remove old dependencies
cargo clean
rm -f Cargo.lock

# Fetch new versions
cargo update

# Build with new dependencies
cargo build
```

### Step 4: Verify Migration

Run your tests to ensure everything works:

```bash
cargo test
```

---

## Important Notes

### 1. Old Versions Are Yanked

All v1.1.10 underscore versions have been **yanked** from crates.io:
- ✅ Existing projects with `Cargo.lock` continue working
- ❌ New projects cannot use v1.1.10
- ⚠️ `cargo update` will require migration to v1.1.11

### 2. Code Imports Unchanged

**Your Rust code does NOT need to change**. Library names remain snake_case:

```rust
// Still works exactly the same
use hanzo_message_primitives::HanzoMessage;
use hanzo_crypto_identities::SignatureScheme;
```

Only `Cargo.toml` package names change.

### 3. These Are NEW Crates

The kebab-case versions are **entirely new crates** on crates.io:
- Different crate IDs
- Fresh version history starting at v1.1.11
- No dependency conflicts with old underscore versions

### 4. Workspace Dependencies

If using workspace dependencies:

```toml
# workspace Cargo.toml
[workspace.dependencies]
hanzo-message-primitives = { path = "./hanzo-libs/hanzo-message-primitives" }
hanzo-crypto-identities = { path = "./hanzo-libs/hanzo-crypto-identities" }

# member Cargo.toml
[dependencies]
hanzo-message-primitives = { workspace = true }
```

---

## Troubleshooting

### Error: "no matching package found"

**Problem**: Cargo still looking for old underscore names

**Solution**:
```bash
# Clear cargo cache
rm -rf ~/.cargo/registry/index/*
cargo clean
cargo update
```

### Error: "yanked version"

**Problem**: `Cargo.toml` still references v1.1.10

**Solution**: Update to v1.1.11 or later:
```toml
hanzo-message-primitives = "1.1.11"  # Not "1.1.10"
```

### Build Errors After Migration

**Problem**: Cached build artifacts from old versions

**Solution**:
```bash
cargo clean
rm -rf target/
cargo build
```

---

## Timeline

- **v1.1.10**: Last underscore version (YANKED November 2025)
- **v1.1.11**: First kebab-case version (NEW CRATE IDs)
- **Future versions**: All use kebab-case names

---

## Support

If you encounter issues during migration:

1. Check this guide first
2. Ensure all versions are updated to 1.1.11
3. Clear cache and rebuild
4. File an issue at: https://github.com/hanzoai/hanzo-node/issues

---

## FAQ

**Q: Why can't I just update the version number?**
A: The kebab-case crates are entirely new crates with new IDs. You must update both name AND version.

**Q: Will my existing project break?**
A: No, if you have a `Cargo.lock` file. But you won't be able to add new dependencies or update versions until you migrate.

**Q: Do I need to change my Rust code?**
A: No. Only `Cargo.toml` changes. Imports remain `use hanzo_*`.

**Q: What if I don't want to migrate?**
A: Your existing project will continue working with yanked v1.1.10 versions. However, you cannot:
- Add new Hanzo crate dependencies
- Update to newer versions
- Share your project with others (they can't fetch yanked versions)

**Q: Can I use both old and new crates together?**
A: Technically yes (different crate IDs), but **strongly discouraged**. This will cause version conflicts and duplicate types.

---

**Migration Path**: Update all Hanzo crate names to kebab-case and bump versions to 1.1.11+

**Last Updated**: November 2025
**Status**: BREAKING CHANGE - Action Required
