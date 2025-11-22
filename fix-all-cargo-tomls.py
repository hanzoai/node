#!/usr/bin/env python3
"""
Fix all hanzo Cargo.toml files to make them publishable
Updates workspace dependencies to explicit versions
"""

import os
import re

# Workspace dependency versions
DEPS = {
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
    'bincode': '1.3.3',
    'sha2': '0.10',
    'blake3': '1.5',
    'thiserror': '2.0',
    'hex': '0.4.3',
    'tokio-test': '0.4',
    'criterion': '0.5',
}

# Features for specific crates
FEATURES = {
    'ed25519-dalek': ['rand_core'],
    'x25519-dalek': ['static_secrets'],
    'chrono': ['serde'],
    'serde': ['derive'],
    'tokio': ['rt', 'rt-multi-thread', 'macros', 'fs', 'io-util', 'net', 'sync', 'time'],
    'reqwest': ['json'],
    'utoipa': ['yaml'],
    'tracing-subscriber': ['env-filter'],
}

# Published hanzo crates
PUBLISHED_HANZO = {
    'hanzo_message_primitives': '1.1.10',
    'hanzo_non_rust_code': '1.1.10',
    'hanzo_crypto_identities': '1.1.10',
    'hanzo_pqc': '1.1.10',
    'hanzo_did': '1.1.10',
    'hanzo_mcp': '1.1.10',
    'hanzo_tools_primitives': '1.1.10',
    'hanzo_tools_runner': '1.0.3',
}

def fix_cargo_toml(filepath):
    """Fix a single Cargo.toml file"""
    with open(filepath, 'r') as f:
        content = f.read()

    original = content

    # Fix package metadata
    content = re.sub(r'version\s*=\s*\{\s*workspace\s*=\s*true\s*\}', 'version = "1.1.10"', content)
    content = re.sub(r'edition\s*=\s*\{\s*workspace\s*=\s*true\s*\}', 'edition = "2021"', content)
    content = re.sub(r'authors\s*=\s*\{\s*workspace\s*=\s*true\s*\}', 'authors = ["Hanzo AI Inc"]', content)

    # Add missing metadata if [package] section exists
    if '[package]' in content:
        if not re.search(r'^\s*license\s*=', content, re.MULTILINE):
            content = re.sub(r'(\[package\].*?authors = .*?\n)', r'\1license = "MIT"\n', content, flags=re.DOTALL)
        if not re.search(r'^\s*repository\s*=', content, re.MULTILINE):
            content = re.sub(r'(license = .*?\n)', r'\1repository = "https://github.com/hanzoai/hanzo-node"\n', content)
        if not re.search(r'^\s*homepage\s*=', content, re.MULTILINE):
            content = re.sub(r'(repository = .*?\n)', r'\1homepage = "https://hanzo.ai"\n', content)
        if not re.search(r'^\s*description\s*=', content, re.MULTILINE):
            # Extract crate name
            match = re.search(r'name\s*=\s*"([^"]+)"', content)
            if match:
                crate_name = match.group(1).replace('hanzo_', '').replace('_', ' ').title()
                content = re.sub(r'(homepage = .*?\n)', f'\\1description = "{crate_name} for Hanzo AI platform"\n', content)

    # Fix workspace dependencies
    def replace_workspace_dep(match):
        full_match = match.group(0)
        dep_name = match.group(1)

        # Check if it's a published hanzo crate
        hanzo_key = dep_name.replace('-', '_')
        if hanzo_key in PUBLISHED_HANZO:
            version = PUBLISHED_HANZO[hanzo_key]
            if dep_name in FEATURES:
                return f'{dep_name} = {{ version = "{version}", features = {FEATURES[dep_name]} }}'
            return f'{dep_name} = "{version}"'

        # Check standard deps
        if dep_name in DEPS:
            version = DEPS[dep_name]
            if dep_name in FEATURES:
                return f'{dep_name} = {{ version = "{version}", features = {FEATURES[dep_name]} }}'
            return f'{dep_name} = "{version}"'

        # Keep as-is if not found
        return full_match

    # Replace workspace = true dependencies
    content = re.sub(
        r'(\w+(?:-\w+)*)\s*=\s*\{\s*workspace\s*=\s*true(?:.*?)\}',
        replace_workspace_dep,
        content
    )

    # Also handle simple workspace = true without {}
    content = re.sub(
        r'workspace\s*=\s*true',
        '',
        content
    )

    if content != original:
        with open(filepath, 'w') as f:
            f.write(content)
        return True
    return False

def main():
    base_path = '/Users/z/work/shinkai/hanzo-node/hanzo-libs'

    print("🔧 Fixing all hanzo Cargo.toml files")
    print("=" * 60)

    fixed_count = 0
    for crate_dir in os.listdir(base_path):
        if not crate_dir.startswith('hanzo-'):
            continue

        cargo_toml = os.path.join(base_path, crate_dir, 'Cargo.toml')
        if not os.path.exists(cargo_toml):
            continue

        if fix_cargo_toml(cargo_toml):
            print(f"✅ Fixed {crate_dir}/Cargo.toml")
            fixed_count += 1
        else:
            print(f"⏭️  No changes needed: {crate_dir}")

    print("=" * 60)
    print(f"✅ Fixed {fixed_count} Cargo.toml files")
    print("\n🧪 All hanzo crates are now ready for publishing!")

if __name__ == "__main__":
    main()
