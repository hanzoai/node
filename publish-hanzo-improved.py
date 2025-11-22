#!/usr/bin/env python3
"""
Improved Hanzo crates publishing script with better workspace inheritance handling.
"""

import subprocess
import os
import time
import json
import sys
import re

# Load the built-in toml library (available in Python 3.11+)
import toml

# Workspace default values from the root Cargo.toml
WORKSPACE_DEFAULTS = {
    'version': '1.1.10',
    'edition': '2021',
    'authors': ['Hanzo AI Inc'],
    'license': 'MIT',
    'repository': 'https://github.com/hanzoai/hanzo-node',
    'homepage': 'https://hanzo.ai',
    'description': 'Hanzo AI component',
}

# Common dependency versions from workspace
DEPENDENCY_VERSIONS = {
    'tokio': '1.36',
    'serde': '1.0.219',
    'serde_json': '1.0.117',
    'chrono': '0.4',  # Needs serde feature
    'reqwest': '0.11.27',  # Needs json feature
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
    'keyphrases': '0.3.2',
    'utoipa': '4.2',
    'nalgebra': '0.32',  # Needs serde-serialize feature
    'tracing': '0.1.40',
    'lazy_static': '1.4.0',
    'once_cell': '1.19',
    'regex': '1',
    'rmcp': '0.6',
    'bincode': '1.3.3',
    'uuid': '1.8.0',
    'csv': '1.3',
}

# List of hanzo crates to publish (in dependency order)
HANZO_CRATES = [
    # Base crates with no hanzo dependencies (first tier)
    "hanzo-message-primitives",
    "hanzo-crypto-identities",
    "hanzo-non-rust-code",
    "hanzo-pqc",

    # Second tier - depend on base crates
    "hanzo-embedding",
    "hanzo-sqlite",
    "hanzo-db",
    "hanzo-did",
    "hanzo-config",

    # Third tier - depend on second tier
    "hanzo-fs",
    "hanzo-job-queue-manager",
    "hanzo-libp2p-relayer",
    "hanzo-mcp",
    "hanzo-kbs",
    "hanzo-model-discovery",
    "hanzo-hmm",

    # Fourth tier - depend on multiple crates
    "hanzo-tools-primitives",
    # hanzo-tools-runner already published at v1.0.3
    "hanzo-llm",
    "hanzo-sheet",
    "hanzo-wasm-runtime",
    "hanzo-mining",
    "hanzo-http-api",

    # Test/sim crates (optional)
    # "hanzo-runtime-tests",
    # "hanzo-simulation",
    # "hanzo-baml",
]

def run_command(cmd):
    """Run a shell command and return success status, output"""
    try:
        result = subprocess.run(cmd, shell=True, capture_output=True, text=True)
        return result.returncode == 0, result.stdout + result.stderr
    except Exception as e:
        return False, str(e)

def fix_dependencies_table_syntax(data):
    """Handle [dependencies.package_name] table syntax."""
    for key in list(data.keys()):
        if key.startswith("dependencies.") or key.startswith("dev-dependencies."):
            parts = key.split(".", 1)
            section = parts[0]
            dep_name = parts[1]

            if section not in data:
                data[section] = {}

            # Move the table content to the main section
            dep_value = data.pop(key)

            # If it's a table with workspace = true, replace with version
            if isinstance(dep_value, dict):
                if dep_value.get('workspace') == True:
                    # Look up the version from our known dependencies
                    if dep_name in DEPENDENCY_VERSIONS:
                        dep_value = {'version': DEPENDENCY_VERSIONS[dep_name]}
                    else:
                        # Try to get from hanzo crates (use workspace version)
                        if dep_name.startswith('hanzo_'):
                            dep_value = {'version': WORKSPACE_DEFAULTS['version']}
                        else:
                            # Default to a reasonable version
                            dep_value = {'version': '1.0'}

                    # Add features if needed
                    if dep_name == 'chrono':
                        dep_value['features'] = ['serde']
                    elif dep_name == 'reqwest':
                        dep_value['features'] = ['json']
                    elif dep_name == 'nalgebra':
                        dep_value['features'] = ['serde-serialize']

            data[section][dep_name] = dep_value

def fix_inline_tables(data):
    """Fix inline tables with workspace inheritance."""
    for section in ['dependencies', 'dev-dependencies', 'build-dependencies']:
        if section in data:
            for dep_name, dep_value in data[section].items():
                if isinstance(dep_value, dict):
                    # Check for workspace inheritance
                    if dep_value.get('workspace') == True:
                        # Look up the version
                        if dep_name in DEPENDENCY_VERSIONS:
                            version = DEPENDENCY_VERSIONS[dep_name]
                        elif dep_name.startswith('hanzo_'):
                            version = WORKSPACE_DEFAULTS['version']
                        else:
                            version = '1.0'

                        # Preserve features if they exist
                        features = dep_value.get('features', [])

                        # Add required features for specific crates
                        if dep_name == 'chrono' and 'serde' not in features:
                            features.append('serde')
                        elif dep_name == 'reqwest' and 'json' not in features:
                            features.append('json')
                        elif dep_name == 'nalgebra' and 'serde-serialize' not in features:
                            features.append('serde-serialize')

                        # Replace with explicit version
                        if features:
                            data[section][dep_name] = {
                                'version': version,
                                'features': features
                            }
                        else:
                            data[section][dep_name] = {'version': version}

                # If it's just a string without version, add one
                elif isinstance(dep_value, str) and dep_value == "":
                    if dep_name in DEPENDENCY_VERSIONS:
                        data[section][dep_name] = DEPENDENCY_VERSIONS[dep_name]
                    else:
                        data[section][dep_name] = "1.0"

def fix_workspace_inheritance(cargo_toml_path):
    """Fix workspace inheritance in a Cargo.toml file."""
    with open(cargo_toml_path, 'r') as f:
        content = f.read()

    # Parse the TOML
    data = toml.loads(content)

    # Fix package metadata
    if 'package' in data:
        package = data['package']
        for field in ['version', 'edition', 'authors', 'license', 'repository', 'homepage']:
            if isinstance(package.get(field), dict) and package[field].get('workspace') == True:
                package[field] = WORKSPACE_DEFAULTS[field]

        # Ensure description exists
        if 'description' not in package or not package['description']:
            package['description'] = WORKSPACE_DEFAULTS['description']

    # Add empty [workspace] table to indicate this is a standalone crate
    if 'workspace' not in data:
        data['workspace'] = {}

    # Fix [dependencies.package_name] table syntax
    fix_dependencies_table_syntax(data)

    # Fix inline tables
    fix_inline_tables(data)

    # Fix reqwest to have json feature
    for section in ['dependencies', 'dev-dependencies', 'build-dependencies']:
        if section in data and 'reqwest' in data[section]:
            req = data[section]['reqwest']
            if isinstance(req, str):
                data[section]['reqwest'] = {
                    'version': req if req else '0.11.27',
                    'features': ['json']
                }
            elif isinstance(req, dict) and 'features' in req and 'json' not in req['features']:
                req['features'].append('json')

    # Fix chrono to have serde feature
    for section in ['dependencies', 'dev-dependencies']:
        if section in data and 'chrono' in data[section]:
            chr = data[section]['chrono']
            if isinstance(chr, str):
                data[section]['chrono'] = {
                    'version': chr if chr else '0.4',
                    'features': ['serde']
                }
            elif isinstance(chr, dict):
                if 'features' not in chr:
                    chr['features'] = ['serde']
                elif 'serde' not in chr['features']:
                    chr['features'].append('serde')

    # Fix nalgebra to have serde-serialize feature
    for section in ['dependencies', 'dev-dependencies']:
        if section in data and 'nalgebra' in data[section]:
            nal = data[section]['nalgebra']
            if isinstance(nal, str):
                data[section]['nalgebra'] = {
                    'version': nal if nal else '0.32',
                    'features': ['serde-serialize']
                }
            elif isinstance(nal, dict):
                if 'features' not in nal:
                    nal['features'] = ['serde-serialize']
                elif 'serde-serialize' not in nal['features']:
                    nal['features'].append('serde-serialize')

    # Write back the fixed TOML
    with open(cargo_toml_path, 'w') as f:
        toml.dump(data, f)

def publish_crate(crate_name, crate_path):
    """Publish a single crate."""
    cargo_toml_path = os.path.join(crate_path, 'Cargo.toml')
    cargo_toml_backup = cargo_toml_path + '.bak'

    print(f"  Fixing workspace inheritance in {cargo_toml_path}")

    # Backup the original
    success, output = run_command(f"cp '{cargo_toml_path}' '{cargo_toml_backup}'")
    if not success:
        print(f"  ❌ Failed to backup Cargo.toml: {output}")
        return False

    try:
        # Fix workspace inheritance
        fix_workspace_inheritance(cargo_toml_path)
        print("  ✅ Fixed workspace inheritance")

        # Try to publish
        print("  📤 Publishing to crates.io...")
        success, output = run_command(f"cd '{crate_path}' && cargo publish --allow-dirty 2>&1")

        if success:
            print(f"  ✅ Successfully published {crate_name}")
            return True
        else:
            print(f"  ❌ Failed to publish {crate_name}")
            print(f"  Error: {output}")
            return False

    finally:
        # Always restore the original
        run_command(f"mv '{cargo_toml_backup}' '{cargo_toml_path}'")
        print("  ✅ Restored original Cargo.toml")

def main():
    print("🚀 Publishing Hanzo Crates to crates.io (Improved)")
    print("=" * 60)

    # Check for CARGO_REGISTRY_TOKEN
    if not os.environ.get('CARGO_REGISTRY_TOKEN'):
        print("❌ CARGO_REGISTRY_TOKEN environment variable not set!")
        print("Please set it with: export CARGO_REGISTRY_TOKEN='your_token'")
        sys.exit(1)

    base_path = "/Users/z/work/shinkai/hanzo-node/hanzo-libs"

    failed_crates = []
    succeeded_crates = []

    for crate_name in HANZO_CRATES:
        print("\n" + "=" * 60)
        print(f"📦 Publishing {crate_name}")
        print("=" * 60)

        crate_path = os.path.join(base_path, crate_name)

        if not os.path.exists(crate_path):
            print(f"  ⚠️ Crate directory not found: {crate_path}")
            failed_crates.append(crate_name)
            continue

        if publish_crate(crate_name, crate_path):
            succeeded_crates.append(crate_name)
            # Wait for crates.io to index
            if crate_name != HANZO_CRATES[-1]:  # Don't wait after the last crate
                print("  ⏳ Waiting 10 seconds for crates.io to index...")
                time.sleep(10)
        else:
            failed_crates.append(crate_name)
            print("  ⚠️ Continuing with next crate...")

    # Summary
    print("\n" + "=" * 60)
    print("📊 Publishing Summary")
    print("=" * 60)

    if succeeded_crates:
        print(f"✅ Successfully published ({len(succeeded_crates)}):")
        for crate in succeeded_crates:
            print(f"  - {crate}")

    if failed_crates:
        print(f"\n❌ Failed to publish ({len(failed_crates)}):")
        for crate in failed_crates:
            print(f"  - {crate}")

    print("\n" + "=" * 60)
    print("✅ Script complete!")

    # Return exit code based on failures
    sys.exit(0 if not failed_crates else 1)

if __name__ == "__main__":
    main()