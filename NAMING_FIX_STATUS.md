# Hanzo Crates Naming Convention Fixes - Status Report

**Date**: 2025-11-15
**Task**: Fix all Hanzo Rust crate names to use idiomatic kebab-case (hyphens) instead of snake_case (underscores)
**Target Version**: 1.1.11

## Summary

### ✅ Completed Naming Convention Fixes

All 24 Hanzo crates have been renamed from `hanzo_*` to `hanzo-*` in accordance with Rust/Cargo naming best practices:

- **[package] name fields**: Changed from snake_case to kebab-case (e.g., `hanzo_db` → `hanzo-db`)
- **[lib] name fields**: Kept as snake_case (e.g., `hanzo_db`) - required by Rust
- **Dependency references**: Updated from `hanzo_*` to `hanzo-*` in all Cargo.toml files
- **Feature references**: Updated from `hanzo_*/feature` to `hanzo-*/feature` format
- **Version bump**: All crates bumped from 1.1.10 to 1.1.11

### ❌ Publication Status

**0 out of 24 crates successfully published at v1.1.11**

All publication attempts failed due to compilation errors (unrelated to naming fixes).

## Naming Convention Rules Applied

### Correct Usage:
```toml
[package]
name = "hanzo-db"  # ✅ Kebab-case with hyphens (crates.io name)
version = "1.1.11"

[lib]
name = "hanzo_db"  # ✅ Snake_case with underscores (Rust import name)
path = "src/lib.rs"

[dependencies]
hanzo-sqlite = { workspace = true }  # ✅ Match package name with hyphens

[features]
default = ["hanzo-tools-primitives/default"]  # ✅ Feature refs use hyphens
```

### Incorrect Usage (Fixed):
```toml
# BEFORE (incorrect):
[package]
name = "hanzo_db"  # ❌ Underscores in package name

[dependencies]
hanzo_sqlite = { workspace = true }  # ❌ Underscores in dependency key

[features]
default = ["hanzo_tools_primitives/default"]  # ❌ Underscores in feature ref
```

## Files Modified

### Core Cargo.toml Files:
- `/Users/z/work/shinkai/hanzo-node/Cargo.toml` (root workspace)
- `hanzo-libs/*/Cargo.toml` (23 library crates)
- `hanzo-bin/hanzo-node/Cargo.toml` (main binary)
- `hanzo-test-framework/Cargo.toml` (test framework)
- `hanzo-test-macro/Cargo.toml` (test macros)

### Scripts Created:
1. **fix-naming-and-republish.sh** - Initial bulk renaming script
2. **fix-dependency-refs.sh** - Fix dependency references
3. **fix-and-publish-failed.sh** - Attempt to publish failed crates
4. **fix-all-remaining.sh** - Final cleanup script

## Verification

### Remaining Underscore References:
```bash
grep -r 'hanzo_[a-z_]*' --include='Cargo.toml' . | grep -v target | grep -v '.bak' | grep -v '^\s*#' | grep -v '\[lib\]'
```

**Result**: Only commented-out lines remain (e.g., `# hanzo_baml`, `# hanzo_engine`)
All active dependency and feature references have been fixed to use hyphens.

### Verification Commands:
```bash
# Check package names
grep '^name = "hanzo' hanzo-libs/*/Cargo.toml

# Check lib names (should use underscores)
grep -A1 '^\[lib\]' hanzo-libs/*/Cargo.toml

# Check dependency refs (should use hyphens)
grep 'hanzo-[a-z-]* = {' Cargo.toml
```

## Crate List

All 24 crates with naming fixes applied (version 1.1.11):

1. hanzo-baml
2. hanzo-config
3. hanzo-crypto-identities
4. hanzo-db
5. hanzo-did
6. hanzo-embedding
7. hanzo-fs
8. hanzo-hmm
9. hanzo-http-api
10. hanzo-job-queue-manager
11. hanzo-kbs
12. hanzo-libp2p-relayer
13. hanzo-llm
14. hanzo-mcp
15. hanzo-message-primitives
16. hanzo-mining
17. hanzo-model-discovery
18. hanzo-non-rust-code
19. hanzo-pqc
20. hanzo-sheet
21. hanzo-sqlite
22. hanzo-tools-primitives
23. hanzo-tools-runner
24. hanzo-wasm-runtime

## Compilation Errors Preventing Publication

### Critical Errors Found:

1. **hanzo-db** (109 errors):
   - Missing imports: `HanzoDbError` not declared
   - Type errors: `bool` cannot be dereferenced
   - Missing implementations

2. **hanzo-kbs** (27 errors):
   - Missing attestation types
   - Unresolved imports
   - Type mismatches

3. **hanzo-model-discovery** (6 errors):
   - Serde derive issues
   - Missing trait implementations

4. **hanzo-hmm** (16 errors):
   - Type inference failures in Matrix operations
   - Placeholder type issues

5. **hanzo-http-api** (3 errors):
   - Missing struct fields: `icons`, `title`, `website_url`
   - MCP server info incomplete

6. **hanzo-sheet** (3 errors):
   - Missing module: `hanzo_message_primitives::schemas::sheet`
   - Unresolved imports

7. **hanzo-libp2p-relayer** (4 errors):
   - Missing cargo features: `json`, `tokio`
   - libp2p version incompatibility

## Next Steps

### Immediate Actions Needed:

1. **Fix Compilation Errors** (Priority: HIGH)
   - Fix each crate's compilation errors individually
   - Start with dependency-order: primitives → tools → higher-level crates

2. **Add Missing Cargo Features**:
   ```toml
   # hanzo-libp2p-relayer/Cargo.toml
   libp2p = { version = "0.55", features = ["tokio", "tcp", "noise", "yamux"] }
   libp2p-request-response = { version = "0.28", features = ["json"] }
   ```

3. **Fix hanzo-http-api MCP Implementation**:
   ```rust
   server_info: Implementation {
       name: "hanzo".to_string(),
       version: "1.1.11".to_string(),
       icons: vec![],  // Add missing field
       title: "Hanzo MCP Server".to_string(),  // Add missing field
       website_url: "https://hanzo.ai".to_string(),  // Add missing field
   }
   ```

4. **Fix hanzo-db Type Issues**:
   - Declare `HanzoDbError` type or import it
   - Fix bool dereferencing issues

5. **Publish in Dependency Order**:
   ```bash
   # 1. Core primitives (no dependencies)
   cd hanzo-libs/hanzo-message-primitives && cargo publish --allow-dirty

   # 2. Tools (depends on primitives)
   cd hanzo-libs/hanzo-tools-primitives && cargo publish --allow-dirty

   # 3. Higher-level libraries
   # ... continue in dependency order
   ```

### Long-term Improvements:

1. **Add CI/CD Checks**:
   - Automated naming convention linting
   - Compilation checks before allowing merges
   - Automated publishing on version bumps

2. **Documentation**:
   - Add CONTRIBUTING.md with naming conventions
   - Update README with crate dependency graph

3. **Testing**:
   - Add integration tests for each crate
   - Verify cross-crate compatibility

## Lessons Learned

1. **Naming Conventions Are Critical**:
   - Cargo/crates.io expect kebab-case for package names
   - Rust code expects snake_case for module names
   - This is NOT optional - it's the ecosystem standard

2. **Dependency References Must Match**:
   - Dependency keys must use kebab-case matching package names
   - Feature references must also use kebab-case

3. **[lib] Names Are Special**:
   - Must use snake_case (Rust requirement)
   - Cannot contain hyphens
   - Different from [package] name

4. **Compilation Before Publication**:
   - All crates must compile successfully before `cargo publish`
   - `--allow-dirty` only skips git checks, not compilation checks

## Status Check Commands

### Check Published Versions:
```bash
# Check all crates on crates.io
for crate in hanzo-{db,embedding,crypto-identities,...}; do
  curl -s "https://crates.io/api/v1/crates/$crate" | jq -r '.crate.max_stable_version'
done

# Or use the provided script:
./check-published-1.1.11.sh
```

### Verify Local Naming:
```bash
# Should show all package names with hyphens
grep '^name = "hanzo' hanzo-libs/*/Cargo.toml

# Should show all lib names with underscores
grep -A1 '^\[lib\]' hanzo-libs/*/Cargo.toml | grep 'name ='
```

### Test Compilation:
```bash
# Test specific crate
cd hanzo-libs/hanzo-embedding && cargo check

# Test all workspace
cargo check --workspace
```

## Conclusion

✅ **Naming convention fixes: COMPLETE**
❌ **Publication: BLOCKED by compilation errors**

All Cargo.toml files have been correctly updated to use idiomatic Rust naming conventions. The crates are ready for publication once the compilation errors are resolved.

The naming fixes are permanent and correct. The next step is to fix the code bugs that prevent compilation, then proceed with publishing in dependency order.

---

**Last Updated**: 2025-11-15
**Author**: AI Assistant
**Repository**: /Users/z/work/shinkai/hanzo-node
