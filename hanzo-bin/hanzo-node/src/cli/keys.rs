use super::{KeyCommands, KeyType, OutputFormat};
use anyhow::{Context, Result};
use chrono::Utc;
use rand::{thread_rng, Rng};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const HANZOD_DIR: &str = ".hanzod";
const KEYS_SUBDIR: &str = "keys";

#[derive(Debug, Serialize, Deserialize)]
pub struct KeySet {
    #[serde(rename = "type")]
    pub key_type: String,
    pub network: Option<String>,
    pub generated_at: String,
    pub keys: std::collections::HashMap<String, String>,
}

impl KeySet {
    pub fn new(key_type: String, network: Option<String>) -> Self {
        Self {
            key_type,
            network,
            generated_at: Utc::now().to_rfc3339(),
            keys: std::collections::HashMap::new(),
        }
    }

    pub fn add_key(&mut self, name: String, value: String) {
        self.keys.insert(name, value);
    }
}

pub struct KeyManager;

impl KeyManager {
    /// Get the ~/.hanzod/keys directory, creating it if needed
    pub fn get_keys_dir() -> Result<PathBuf> {
        let home = dirs::home_dir().context("Could not find home directory")?;
        let keys_dir = home.join(HANZOD_DIR).join(KEYS_SUBDIR);

        if !keys_dir.exists() {
            fs::create_dir_all(&keys_dir)
                .with_context(|| format!("Failed to create directory: {:?}", keys_dir))?;

            // Set restrictive permissions (700)
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = fs::Permissions::from_mode(0o700);
                fs::set_permissions(&keys_dir, perms)?;
            }
        }

        Ok(keys_dir)
    }

    /// Generate a random private key (32 bytes hex)
    pub fn generate_private_key() -> String {
        let mut rng = thread_rng();
        let mut bytes = [0u8; 32];
        rng.fill(&mut bytes);
        format!("0x{}", hex::encode(bytes))
    }

    /// Generate a random address (20 bytes hex)
    pub fn generate_address() -> String {
        let mut rng = thread_rng();
        let mut bytes = [0u8; 20];
        rng.fill(&mut bytes);
        format!("0x{}", hex::encode(bytes))
    }

    /// Generate test mnemonic (for testing only)
    pub fn generate_test_mnemonic() -> String {
        // Using first 12 words from BIP39 wordlist for testing
        // In production, use proper BIP39 generation
        "abandon ability able about above absent absorb abstract absurd abuse access accident".to_string()
    }

    /// Execute key generation based on type
    pub fn generate_keys(key_type: &KeyType) -> Result<KeySet> {
        match key_type {
            KeyType::Wallet { network } => {
                let mut keyset = KeySet::new("wallet".to_string(), Some(network.clone()));
                keyset.add_key("PRIVATE_KEY".to_string(), Self::generate_private_key());
                keyset.add_key("ADDRESS".to_string(), Self::generate_address());
                Ok(keyset)
            }

            KeyType::Identity => {
                let mut keyset = KeySet::new("identity".to_string(), None);
                keyset.add_key("NODE_IDENTITY_KEY".to_string(), Self::generate_private_key());
                keyset.add_key("NODE_ENCRYPTION_KEY".to_string(), Self::generate_private_key());
                Ok(keyset)
            }

            KeyType::X402 => {
                let mut keyset = KeySet::new("x402".to_string(), None);
                keyset.add_key("X402_PRIVATE_KEY".to_string(), Self::generate_private_key());
                keyset.add_key("X402_PAY_TO".to_string(), Self::generate_address());
                Ok(keyset)
            }

            KeyType::Test => {
                let mut keyset = KeySet::new("test".to_string(), None);
                keyset.add_key("FROM_WALLET_PRIVATE_KEY".to_string(), Self::generate_private_key());
                keyset.add_key("X402_PRIVATE_KEY".to_string(), Self::generate_private_key());
                keyset.add_key("X402_PAY_TO".to_string(), Self::generate_address());
                keyset.add_key("RESTORE_WALLET_MNEMONICS_NODE2".to_string(), Self::generate_test_mnemonic());
                Ok(keyset)
            }

            KeyType::Staking { amount } => {
                let mut keyset = KeySet::new("staking".to_string(), None);
                keyset.add_key("VALIDATOR_PRIVATE_KEY".to_string(), Self::generate_private_key());
                keyset.add_key("WITHDRAWAL_ADDRESS".to_string(), Self::generate_address());
                if let Some(amt) = amount {
                    keyset.add_key("STAKE_AMOUNT".to_string(), amt.clone());
                }
                Ok(keyset)
            }
        }
    }

    /// Format keyset for output
    pub fn format_output(keyset: &KeySet, format: &OutputFormat) -> Result<String> {
        match format {
            OutputFormat::Env => {
                let mut output = format!("# Hanzo Node Keys\n");
                output.push_str(&format!("# Type: {}\n", keyset.key_type));
                if let Some(network) = &keyset.network {
                    output.push_str(&format!("# Network: {}\n", network));
                }
                output.push_str(&format!("# Generated: {}\n\n", keyset.generated_at));

                for (key, value) in &keyset.keys {
                    output.push_str(&format!("export {}=\"{}\"\n", key, value));
                }
                Ok(output)
            }

            OutputFormat::Json => {
                serde_json::to_string_pretty(&keyset)
                    .context("Failed to serialize to JSON")
            }

            OutputFormat::Yaml => {
                serde_yaml::to_string(&keyset)
                    .context("Failed to serialize to YAML")
            }

            OutputFormat::Toml => {
                toml::to_string_pretty(&keyset)
                    .context("Failed to serialize to TOML")
            }
        }
    }

    /// Save keyset to file
    pub fn save_keyset(keyset: &KeySet, filename: Option<String>) -> Result<PathBuf> {
        let keys_dir = Self::get_keys_dir()?;

        let filename = filename.unwrap_or_else(|| {
            format!("{}-{}.json", keyset.key_type,
                    Utc::now().format("%Y%m%d-%H%M%S"))
        });

        let file_path = keys_dir.join(filename);

        let json = serde_json::to_string_pretty(&keyset)?;
        fs::write(&file_path, json)?;

        // Set restrictive permissions (600)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o600);
            fs::set_permissions(&file_path, perms)?;
        }

        Ok(file_path)
    }

    /// List all keysets in ~/.hanzod/keys/
    pub fn list_keysets() -> Result<Vec<PathBuf>> {
        let keys_dir = Self::get_keys_dir()?;

        let mut keysets = Vec::new();
        for entry in fs::read_dir(keys_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                keysets.push(path);
            }
        }

        keysets.sort();
        Ok(keysets)
    }

    /// Load a keyset from file
    pub fn load_keyset(path: &Path) -> Result<KeySet> {
        let content = fs::read_to_string(path)?;
        let keyset: KeySet = serde_json::from_str(&content)?;
        Ok(keyset)
    }
}

/// Handle the keys subcommand
pub async fn handle_keys_command(cmd: &KeyCommands) -> Result<()> {
    match cmd {
        KeyCommands::Generate { key_type, format, save } => {
            let keyset = KeyManager::generate_keys(key_type)?;
            let output = KeyManager::format_output(&keyset, format)?;

            println!("{}", output);

            if *save {
                let path = KeyManager::save_keyset(&keyset, None)?;
                eprintln!("\n✓ Keys saved to: {:?}", path);
            }

            eprintln!("\n⚠️  WARNING: These are test keys - never use with real funds!");
        }

        KeyCommands::List { show_private } => {
            let keysets = KeyManager::list_keysets()?;

            if keysets.is_empty() {
                println!("No keysets found in ~/.hanzod/keys/");
                return Ok(());
            }

            for path in keysets {
                let keyset = KeyManager::load_keyset(&path)?;
                println!("\n📁 {}", path.file_name().unwrap().to_string_lossy());
                println!("  Type: {}", keyset.key_type);
                if let Some(network) = keyset.network {
                    println!("  Network: {}", network);
                }
                println!("  Generated: {}", keyset.generated_at);

                if *show_private {
                    println!("  Keys:");
                    for (key, value) in &keyset.keys {
                        println!("    {}: {}", key, value);
                    }
                } else {
                    println!("  Keys: {} keys stored", keyset.keys.len());
                }
            }
        }

        KeyCommands::Import { file: _ } => {
            println!("Key import not yet implemented");
        }

        KeyCommands::Export { file: _, include_private: _ } => {
            println!("Key export not yet implemented");
        }
    }

    Ok(())
}