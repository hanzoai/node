#!/usr/bin/env python3
"""
Publish all hanzo crates to crates.io with proper dependency resolution
"""

import subprocess
import os
import time
import sys

# Hanzo crates in dependency order
HANZO_CRATES = [
    # Tier 1: No hanzo dependencies
    "hanzo-message-primitives",      # Already published!
    "hanzo-crypto-identities",
    "hanzo-non-rust-code",
    "hanzo-pqc",

    # Tier 2: Depend on tier 1
    "hanzo-embedding",
    "hanzo-sqlite",
    "hanzo-db",
    "hanzo-did",
    "hanzo-config",

    # Tier 3: Depend on tier 1+2
    "hanzo-fs",
    "hanzo-job-queue-manager",
    "hanzo-libp2p-relayer",
    "hanzo-mcp",
    "hanzo-kbs",
    "hanzo-model-discovery",
    "hanzo-hmm",

    # Tier 4: Depend on multiple lower tiers
    "hanzo-tools-primitives",
    "hanzo-llm",
    "hanzo-sheet",
    "hanzo-wasm-runtime",
    "hanzo-mining",
    "hanzo-http-api",
]

# Fixes needed based on hanzo-message-primitives experience
CARGO_TOML_FIXES = {
    "thiserror": "2.0",  # Not 1.0.86
    "blake3": "1.5",     # Not 1.8.2
}

FEATURE_ADDITIONS = {
    "ed25519-dalek": ["rand_core"],
    "x25519-dalek": ["static_secrets"],
}

def run_command(cmd, cwd=None):
    """Run command and return (success, output)"""
    try:
        result = subprocess.run(
            cmd,
            shell=True,
            capture_output=True,
            text=True,
            cwd=cwd
        )
        return result.returncode == 0, result.stdout + result.stderr
    except Exception as e:
        return False, str(e)

def fix_cargo_toml(crate_path):
    """Apply fixes to Cargo.toml"""
    cargo_toml = os.path.join(crate_path, "Cargo.toml")

    if not os.path.exists(cargo_toml):
        return False

    with open(cargo_toml, 'r') as f:
        content = f.read()

    original = content

    # Fix thiserror version
    content = content.replace('thiserror = "1.0.86"', 'thiserror = "2.0"')

    # Fix blake3 version
    content = content.replace('blake3 = "1.8.2"', 'blake3 = "1.5"')

    # Add features to ed25519-dalek if needed
    if 'ed25519-dalek = "2.1.1"' in content:
        content = content.replace(
            'ed25519-dalek = "2.1.1"',
            'ed25519-dalek = { version = "2.1.1", features = ["rand_core"] }'
        )

    # Add features to x25519-dalek if needed
    if 'x25519-dalek = "2.0.1"' in content:
        content = content.replace(
            'x25519-dalek = "2.0.1"',
            'x25519-dalek = { version = "2.0.1", features = ["static_secrets"] }'
        )

    if content != original:
        with open(cargo_toml, 'w') as f:
            f.write(content)
        print("  ✅ Applied Cargo.toml fixes")
        return True
    else:
        print("  ⏭️  No Cargo.toml fixes needed")
        return False

def publish_crate(crate_name, base_path):
    """Publish a single crate"""
    crate_path = os.path.join(base_path, crate_name)

    if not os.path.exists(crate_path):
        print(f"  ⚠️  Crate directory not found: {crate_path}")
        return False

    print(f"\n📦 Publishing {crate_name}")
    print("=" * 60)

    # Fix Cargo.toml
    fix_cargo_toml(crate_path)

    # Check if already published
    success, output = run_command(f'cargo search {crate_name.replace("-", "_")} --limit 1')
    if crate_name.replace("-", "_") in output:
        print(f"  ℹ️  {crate_name} may already be published")
        print(f"  Attempting publish anyway...")

    # Publish
    print(f"  📤 Publishing to crates.io...")
    success, output = run_command("cargo publish --allow-dirty", cwd=crate_path)

    if success or "already uploaded" in output.lower():
        print(f"  ✅ {crate_name} published successfully")
        return True
    else:
        print(f"  ❌ Failed to publish {crate_name}")
        # Show last 30 lines of error
        lines = output.split('\n')
        print("  Error output:")
        for line in lines[-30:]:
            print(f"    {line}")
        return False

def main():
    print("🚀 Publishing All Hanzo Crates")
    print("=" * 60)

    # Check CARGO_REGISTRY_TOKEN
    if not os.environ.get('CARGO_REGISTRY_TOKEN'):
        print("❌ CARGO_REGISTRY_TOKEN not set!")
        sys.exit(1)

    base_path = "/Users/z/work/shinkai/hanzo-node/hanzo-libs"

    succeeded = []
    failed = []
    skipped = ["hanzo-message-primitives"]  # Already published

    for crate_name in HANZO_CRATES:
        if crate_name == "hanzo-message-primitives":
            print(f"\n✅ {crate_name} - Already published, skipping")
            continue

        if publish_crate(crate_name, base_path):
            succeeded.append(crate_name)
            # Wait for crates.io to index
            if crate_name != HANZO_CRATES[-1]:
                print("  ⏳ Waiting 10 seconds for crates.io...")
                time.sleep(10)
        else:
            failed.append(crate_name)
            # Ask user if they want to continue
            response = input(f"\n⚠️  {crate_name} failed. Continue? (y/n): ")
            if response.lower() != 'y':
                print("Stopping...")
                break

    # Summary
    print("\n" + "=" * 60)
    print("📊 Publishing Summary")
    print("=" * 60)

    if skipped:
        print(f"⏭️  Skipped (already published): {', '.join(skipped)}")

    if succeeded:
        print(f"\n✅ Successfully published ({len(succeeded)}):")
        for crate in succeeded:
            print(f"  - {crate}")

    if failed:
        print(f"\n❌ Failed ({len(failed)}):")
        for crate in failed:
            print(f"  - {crate}")

    print("\n" + "=" * 60)

    sys.exit(0 if not failed else 1)

if __name__ == "__main__":
    main()
