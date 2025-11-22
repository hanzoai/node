#!/usr/bin/env python3
"""
Automated hanzo crate publishing with intelligent dependency resolution
"""

import subprocess
import os
import time
import sys
import re

# Workspace dependency versions
WORKSPACE_DEPS = {
    'tokio': '1.36',
    'serde': '1.0.219',
    'serde_json': '1.0.117',
    'chrono': '0.4',
    'reqwest': '0.11.27',
    'log': '0.4.20',
    'async-trait': '0.1.81',
    'futures': '0.3.30',
    'anyhow': '1.0.86',
    'dashmap': '6.0',
    'ed25519-dalek': '2.1.1',
    'x25519-dalek': '2.0.1',
    'base64': '0.22.0',
    'rusqlite': '0.31.0',
    'dirs': '5.0',
    'clap': '4.5.4',
    'home': '0.5.5',
    'utoipa': '4.2',
    'tracing': '0.1.40',
    'lazy_static': '1.4.0',
    'once_cell': '1.19',
    'regex': '1',
    'tempfile': '3.8',
}

# Crypto features
CRYPTO_FEATURES = {
    'ed25519-dalek': ['rand_core'],
    'x25519-dalek': ['static_secrets'],
    'chrono': ['serde'],
    'tokio': ['rt', 'rt-multi-thread', 'macros', 'fs', 'io-util', 'net', 'sync', 'time'],
    'serde': ['derive'],
}

# Published hanzo crates (updated as we go)
PUBLISHED_HANZO = {
    'hanzo_message_primitives': '1.1.10',
    'hanzo_non_rust_code': '1.1.10',
    'hanzo_crypto_identities': '1.1.10',
    'hanzo_tools_runner': '1.0.3',  # Already published
}

# Publishing order
HANZO_CRATES = [
    'hanzo-pqc',  # Tier 1
    'hanzo-embedding',
    'hanzo-sqlite',
    'hanzo-db',
    'hanzo-did',
    'hanzo-config',
    'hanzo-fs',
    'hanzo-job-queue-manager',
    'hanzo-libp2p-relayer',
    'hanzo-mcp',
    'hanzo-kbs',
    'hanzo-model-discovery',
    'hanzo-hmm',
    'hanzo-tools-primitives',
    'hanzo-llm',
    'hanzo-sheet',
    'hanzo-wasm-runtime',
    'hanzo-mining',
    'hanzo-http-api',
]

def run_cmd(cmd, cwd=None):
    """Run command and return success, output"""
    try:
        result = subprocess.run(cmd, shell=True, capture_output=True, text=True, cwd=cwd)
        return result.returncode == 0, result.stdout + result.stderr
    except Exception as e:
        return False, str(e)

def read_cargo_toml(path):
    """Read Cargo.toml file"""
    with open(path, 'r') as f:
        return f.read()

def create_fixed_cargo_toml(crate_path, crate_name):
    """Create a fixed Cargo.toml with all workspace deps resolved"""
    cargo_toml = os.path.join(crate_path, 'Cargo.toml')
    content = read_cargo_toml(cargo_toml)

    # Replace workspace metadata
    content = re.sub(r'version\s*=\s*\{\s*workspace\s*=\s*true\s*\}', 'version = "1.1.10"', content)
    content = re.sub(r'edition\s*=\s*\{\s*workspace\s*=\s*true\s*\}', 'edition = "2021"', content)
    content = re.sub(r'authors\s*=\s*\{\s*workspace\s*=\s*true\s*\}', 'authors = ["Hanzo AI Inc"]', content)

    # Add missing metadata if not present
    if 'license =' not in content:
        # Add after authors line
        content = re.sub(
            r'(authors = .*\n)',
            r'\1license = "MIT"\n',
            content
        )
    if 'repository =' not in content:
        content = re.sub(
            r'(authors = .*\n)',
            r'\1repository = "https://github.com/hanzoai/hanzo-node"\n',
            content
        )
    if 'homepage =' not in content:
        content = re.sub(
            r'(authors = .*\n)',
            r'\1homepage = "https://hanzo.ai"\n',
            content
        )
    if 'description =' not in content:
        desc = f"{crate_name.replace('hanzo-', '').replace('-', ' ').title()} for Hanzo AI platform"
        content = re.sub(
            r'(authors = .*\n)',
            f'\\1description = "{desc}"\n',
            content
        )

    # Replace workspace dependencies
    def replace_dep(match):
        dep_name = match.group(1)

        # Check if it's a hanzo crate
        hanzo_key = dep_name.replace('-', '_')
        if hanzo_key in PUBLISHED_HANZO:
            version = PUBLISHED_HANZO[hanzo_key]
            if dep_name in CRYPTO_FEATURES:
                features = CRYPTO_FEATURES[dep_name]
                return f'{dep_name} = {{ version = "{version}", features = {features} }}'
            return f'{dep_name} = "{version}"'

        # Check if it's a workspace dep
        if dep_name in WORKSPACE_DEPS:
            version = WORKSPACE_DEPS[dep_name]
            if dep_name in CRYPTO_FEATURES:
                features = CRYPTO_FEATURES[dep_name]
                return f'{dep_name} = {{ version = "{version}", features = {features} }}'
            return f'{dep_name} = "{version}"'

        # Keep as-is
        return match.group(0)

    # Replace workspace deps
    content = re.sub(
        r'(\w+(?:-\w+)*)\s*=\s*\{\s*workspace\s*=\s*true.*?\}',
        replace_dep,
        content
    )

    return content

def publish_crate(crate_name, base_path):
    """Publish a single crate"""
    crate_path = os.path.join(base_path, crate_name)

    if not os.path.exists(crate_path):
        print(f"  ⚠️  Not found: {crate_path}")
        return False

    print(f"\n📦 Publishing {crate_name}")
    print("=" * 60)

    # Create fixed Cargo.toml
    try:
        fixed_content = create_fixed_cargo_toml(crate_path, crate_name)
        cargo_toml = os.path.join(crate_path, 'Cargo.toml')
        backup = cargo_toml + '.backup'

        # Backup
        subprocess.run(f'cp "{cargo_toml}" "{backup}"', shell=True, check=True)

        # Write fixed version
        with open(cargo_toml, 'w') as f:
            f.write(fixed_content)

        print("  ✅ Created fixed Cargo.toml")

    except Exception as e:
        print(f"  ❌ Failed to fix Cargo.toml: {e}")
        return False

    # Publish
    print("  📤 Publishing...")
    success, output = run_cmd('cargo publish --allow-dirty', cwd=crate_path)

    # Restore original
    subprocess.run(f'mv "{backup}" "{cargo_toml}"', shell=True)

    if success or 'already uploaded' in output.lower():
        print(f"  ✅ Published {crate_name}")
        # Add to published list
        PUBLISHED_HANZO[crate_name.replace('-', '_')] = '1.1.10'
        return True
    else:
        print(f"  ❌ Failed to publish")
        # Show errors
        for line in output.split('\n')[-20:]:
            if 'error' in line.lower() or 'failed' in line.lower():
                print(f"    {line}")
        return False

def main():
    print("🚀 Automated Hanzo Crate Publishing")
    print("=" * 60)

    if not os.environ.get('CARGO_REGISTRY_TOKEN'):
        print("❌ CARGO_REGISTRY_TOKEN not set!")
        sys.exit(1)

    base_path = '/Users/z/work/shinkai/hanzo-node/hanzo-libs'

    succeeded = list(PUBLISHED_HANZO.keys())
    failed = []

    for crate_name in HANZO_CRATES:
        if publish_crate(crate_name, base_path):
            # Wait for indexing
            if crate_name != HANZO_CRATES[-1]:
                print("  ⏳ Waiting 10s for crates.io...")
                time.sleep(10)
        else:
            failed.append(crate_name)
            print(f"\n⚠️  {crate_name} failed. Continuing...")

    # Summary
    print("\n" + "=" * 60)
    print("📊 Summary")
    print("=" * 60)
    print(f"✅ Published ({len(succeeded)}):")
    for c in succeeded:
        print(f"  - {c}")

    if failed:
        print(f"\n❌ Failed ({len(failed)}):")
        for c in failed:
            print(f"  - {c}")

    sys.exit(0 if not failed else 1)

if __name__ == "__main__":
    main()
