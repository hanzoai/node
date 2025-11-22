# Hanzo Crates Idiomatic Renaming Plan

**Date**: 2025-11-16
**Goal**: Rename all 24 Hanzo crates to idiomatic kebab-case names that crates.io will accept
**Strategy**: Use completely new names (not just case changes) to bypass crates.io's name history

---

## Renaming Map

| Old Name (underscore) | New Name (idiomatic) | Rationale |
|----------------------|---------------------|-----------|
| `hanzo_message_primitives` | `hanzo-messages` | Core concept, drop "primitives" |
| `hanzo_crypto_identities` | `hanzo-identity` | Singular, cleaner |
| `hanzo_libp2p_relayer` | `hanzo-libp2p` | Protocol name, drop "relayer" |
| `hanzo_job_queue_manager` | `hanzo-jobs` | Simpler, what it manages |
| `hanzo_fs` | `hanzo-fs` | Already short, just hyphenate |
| `hanzo_embedding` | `hanzo-embed` | Shorter verb form |
| `hanzo_http_api` | `hanzo-api` | Drop protocol prefix |
| `hanzo_tools_primitives` | `hanzo-tools` | Drop redundant "primitives" |
| `hanzo_tools_runner` | `hanzo-runner` | Context makes it clear |
| `hanzo_sqlite` | `hanzo-db-sqlite` | More specific about being DB |
| `hanzo_db` | `hanzo-database` | Spell out, avoid confusion |
| `hanzo_hmm` | `hanzo-hmm` | Keep abbreviation |
| `hanzo_non_rust_code` | `hanzo-runtime` | What it actually provides |
| `hanzo_mcp` | `hanzo-mcp` | Keep protocol abbreviation |
| `hanzo_pqc` | `hanzo-pqc` | Keep crypto abbreviation |
| `hanzo_kbs` | `hanzo-kbs` | Keep abbreviation |
| `hanzo_did` | `hanzo-did` | Keep DID standard name |
| `hanzo_model_discovery` | `hanzo-models` | What it manages |
| `hanzo_config` | `hanzo-config` | Already clean |
| `hanzo_mining` | `hanzo-mining` | Already clean |
| `hanzo_wasm_runtime` | `hanzo-wasm` | Drop "runtime" |
| `hanzo_llm` | `hanzo-llm` | Keep abbreviation |
| `hanzo_sheet` | `hanzo-sheet` | Already clean |
| `hanzo_runtime_tests` | `hanzo-tests` | Drop "runtime" |

---

## Implementation Strategy

### Phase 1: Rename Package Names (Cargo.toml files)
For each crate's `Cargo.toml`:
```toml
[package]
name = "hanzo-messages"  # NEW idiomatic name
version = "1.1.11"

[lib]
name = "hanzo_message_primitives"  # Keep for backward compat in code
```

**Note**: Library names (`[lib]`) can stay as snake_case for code compatibility.

### Phase 2: Update All Dependencies
Update every `Cargo.toml` that references the old names:
```toml
# Before
hanzo_message_primitives = { workspace = true }

# After
hanzo-messages = { workspace = true }
```

### Phase 3: Update Workspace Members
Root `Cargo.toml`:
```toml
[workspace]
members = [
    "hanzo-libs/hanzo-messages",
    "hanzo-libs/hanzo-identity",
    # ... all 24 crates
]

[workspace.dependencies]
hanzo-messages = { path = "./hanzo-libs/hanzo-messages" }
hanzo-identity = { path = "./hanzo-libs/hanzo-identity" }
# ... all 24 crates
```

### Phase 4: Rename Directory Structures
```bash
mv hanzo-libs/hanzo-message-primitives hanzo-libs/hanzo-messages
mv hanzo-libs/hanzo-crypto-identities hanzo-libs/hanzo-identity
# ... all 24 directories
```

### Phase 5: Update Import Statements (if needed)
Most code should NOT need changes because `[lib]` names stay snake_case:
```rust
// Still works
use hanzo_message_primitives::HanzoMessage;
```

### Phase 6: Publish to crates.io
Publish in dependency order (6 tiers):

**Tier 1** (no dependencies):
- hanzo-messages
- hanzo-identity
- hanzo-pqc

**Tier 2** (depend on Tier 1):
- hanzo-embed
- hanzo-runtime
- hanzo-did

**Tier 3** (depend on Tier 2):
- hanzo-tools
- hanzo-mcp
- hanzo-config
- hanzo-db-sqlite

**Tier 4** (depend on Tier 3):
- hanzo-database
- hanzo-libp2p
- hanzo-jobs
- hanzo-fs

**Tier 5** (depend on Tier 4):
- hanzo-kbs
- hanzo-models
- hanzo-hmm
- hanzo-llm

**Tier 6** (depend on Tier 5):
- hanzo-sheet
- hanzo-wasm
- hanzo-mining
- hanzo-api
- hanzo-runner

---

## Migration Guide for Users

### Updating Cargo.toml
```toml
# Before (v1.1.10 - yanked)
[dependencies]
hanzo_message_primitives = "1.1.10"
hanzo_crypto_identities = "1.1.10"

# After (v1.1.11 - new crates)
[dependencies]
hanzo-messages = "1.1.11"
hanzo-identity = "1.1.11"
```

### Code Changes
**NONE REQUIRED** - library names stay the same:
```rust
// Still works
use hanzo_message_primitives::HanzoMessage;
use hanzo_crypto_identities::SignatureScheme;
```

---

## Deprecation Strategy

### Old Crates (hanzo_*)
1. ✅ Already yanked v1.1.10
2. Add README deprecation notice
3. Point users to new crate names
4. Keep yanked indefinitely

### README Template
```markdown
# ⚠️ DEPRECATED

This crate has been renamed to `hanzo-messages` for idiomatic Rust naming.

Please update your `Cargo.toml`:
```toml
[dependencies]
hanzo-messages = "1.1.11"  # Use this instead
```

Your code does NOT need to change - imports stay the same.

See [MIGRATION.md](../MIGRATION.md) for full details.
```

---

## Timeline

### Week 1 (Current)
- ✅ Document renaming plan
- ⏳ Rename all 24 Cargo.toml files
- ⏳ Update all dependency references
- ⏳ Rename directories
- ⏳ Test compilation

### Week 2
- Publish Tier 1-3 crates
- Monitor for issues
- Update documentation

### Week 3
- Publish Tier 4-6 crates
- Create migration guide
- Announce deprecation

---

## Risks & Mitigation

### Risk 1: Breaking Changes
**Mitigation**: Library names stay the same, so code doesn't break

### Risk 2: User Confusion
**Mitigation**: Clear deprecation notices and migration guide

### Risk 3: Dependency Hell
**Mitigation**: Publish in strict dependency order with testing between tiers

### Risk 4: Name Conflicts
**Mitigation**: Check crates.io availability before renaming

---

## Name Availability Check

Before proceeding, verify all 24 new names are available on crates.io:
```bash
cargo search hanzo-messages  # Should return "no results"
cargo search hanzo-identity  # Should return "no results"
# ... check all 24
```

**Status**: ⏳ Pending verification

---

## Rollback Plan

If critical issues arise:
1. Keep old crate names available (un-yank if needed)
2. Publish patch versions with fixes
3. Give users 6+ months to migrate
4. Only fully deprecate after >90% adoption

---

**Next Steps**:
1. Verify all 24 names are available on crates.io
2. Begin Phase 1: Rename Cargo.toml files
3. Update all dependencies
4. Test compilation of all crates
5. Publish Tier 1 crates

**Status**: READY TO PROCEED ✅
