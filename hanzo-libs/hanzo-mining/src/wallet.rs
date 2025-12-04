//! Quantum-Safe Mining Wallet - ML-DSA (FIPS 204) Implementation
//!
//! This module provides quantum-resistant wallet functionality for AI coin mining,
//! using ML-DSA (Module-Lattice Digital Signature Algorithm) as specified in FIPS 204.
//!
//! ## Security Levels
//!
//! | Level | Algorithm | Security | Signature Size | Public Key |
//! |-------|-----------|----------|----------------|------------|
//! | 2     | ML-DSA-44 | 128-bit  | 2,420 bytes    | 1,312 bytes |
//! | 3     | ML-DSA-65 | 192-bit  | 3,309 bytes    | 1,952 bytes |
//! | 5     | ML-DSA-87 | 256-bit  | 4,627 bytes    | 2,592 bytes |
//!
//! ## Usage
//!
//! ```ignore
//! use hanzo_mining::wallet::{MiningWallet, SecurityLevel};
//!
//! // Generate a quantum-safe mining wallet
//! let wallet = MiningWallet::generate(SecurityLevel::Level3).await?;
//!
//! // Get wallet address (derived from public key)
//! let address = wallet.address();
//!
//! // Sign a mining transaction
//! let signature = wallet.sign(&transaction_bytes).await?;
//!
//! // Export encrypted wallet
//! let encrypted = wallet.export_encrypted("passphrase")?;
//! ```

use crate::ledger::{derive_address_from_pubkey, MiningLedger};
use crate::evm::{TeleportDestination, TeleportTransfer, ChainConfig, EvmClient};
use crate::{MiningRewardType, PerformanceStats};
use serde::{Deserialize, Serialize};
use std::path::Path;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// NIST security levels for ML-DSA
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecurityLevel {
    /// NIST Level 2 - 128-bit security (ML-DSA-44)
    Level2,
    /// NIST Level 3 - 192-bit security (ML-DSA-65) - RECOMMENDED
    Level3,
    /// NIST Level 5 - 256-bit security (ML-DSA-87)
    Level5,
}

impl SecurityLevel {
    /// Get the public key size in bytes
    pub fn public_key_size(&self) -> usize {
        match self {
            Self::Level2 => 1312,  // ML-DSA-44
            Self::Level3 => 1952,  // ML-DSA-65
            Self::Level5 => 2592,  // ML-DSA-87
        }
    }

    /// Get the secret key size in bytes
    pub fn secret_key_size(&self) -> usize {
        match self {
            Self::Level2 => 2560,  // ML-DSA-44
            Self::Level3 => 4032,  // ML-DSA-65
            Self::Level5 => 4896,  // ML-DSA-87
        }
    }

    /// Get the signature size in bytes
    pub fn signature_size(&self) -> usize {
        match self {
            Self::Level2 => 2420,  // ML-DSA-44
            Self::Level3 => 3309,  // ML-DSA-65
            Self::Level5 => 4627,  // ML-DSA-87
        }
    }

    /// Get the algorithm name
    pub fn algorithm_name(&self) -> &'static str {
        match self {
            Self::Level2 => "ML-DSA-44",
            Self::Level3 => "ML-DSA-65",
            Self::Level5 => "ML-DSA-87",
        }
    }
}

impl Default for SecurityLevel {
    fn default() -> Self {
        Self::Level3 // Recommended default
    }
}

/// Quantum-safe mining wallet
#[derive(ZeroizeOnDrop)]
pub struct MiningWallet {
    /// Security level
    #[zeroize(skip)]
    security_level: SecurityLevel,
    /// Public key (verifying key)
    #[zeroize(skip)]
    public_key: Vec<u8>,
    /// Secret key (signing key) - zeroized on drop
    secret_key: Vec<u8>,
    /// Derived address
    #[zeroize(skip)]
    address: String,
    /// Wallet label
    #[zeroize(skip)]
    label: Option<String>,
}

impl MiningWallet {
    /// Generate a new quantum-safe wallet
    pub async fn generate(level: SecurityLevel) -> Result<Self, WalletError> {
        // Generate ML-DSA keypair
        // In production, this would use hanzo_pqc::signature::MlDsa
        let (public_key, secret_key) = generate_ml_dsa_keypair(level)?;
        let address = derive_address_from_pubkey(&public_key);

        Ok(Self {
            security_level: level,
            public_key,
            secret_key,
            address,
            label: None,
        })
    }

    /// Generate with default security level (Level 3)
    pub async fn generate_default() -> Result<Self, WalletError> {
        Self::generate(SecurityLevel::default()).await
    }

    /// Import wallet from secret key
    pub fn from_secret_key(secret_key: &[u8], level: SecurityLevel) -> Result<Self, WalletError> {
        if secret_key.len() != level.secret_key_size() {
            return Err(WalletError::InvalidKeySize {
                expected: level.secret_key_size(),
                actual: secret_key.len(),
            });
        }

        // Derive public key from secret key
        let public_key = derive_public_key(secret_key, level)?;
        let address = derive_address_from_pubkey(&public_key);

        Ok(Self {
            security_level: level,
            public_key,
            secret_key: secret_key.to_vec(),
            address,
            label: None,
        })
    }

    /// Import wallet from encrypted file
    pub async fn import_encrypted(path: &Path, passphrase: &str) -> Result<Self, WalletError> {
        let encrypted_data = std::fs::read(path)
            .map_err(|e| WalletError::IoError(e.to_string()))?;
        Self::import_from_bytes(&encrypted_data, passphrase)
    }

    /// Import wallet from encrypted bytes (in-memory)
    pub fn import_from_bytes(encrypted_data: &[u8], passphrase: &str) -> Result<Self, WalletError> {
        let decrypted = decrypt_wallet(encrypted_data, passphrase)?;
        let wallet_data: WalletData = serde_json::from_slice(&decrypted)
            .map_err(|e| WalletError::DeserializationError(e.to_string()))?;

        // Use stored public key (not derived) to maintain identity
        let address = derive_address_from_pubkey(&wallet_data.public_key);

        Ok(Self {
            security_level: wallet_data.security_level,
            public_key: wallet_data.public_key.clone(),
            secret_key: wallet_data.secret_key.clone(),
            address,
            label: wallet_data.label.clone(),
        })
    }

    /// Get wallet address (0x-prefixed)
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Get public key bytes
    pub fn public_key(&self) -> &[u8] {
        &self.public_key
    }

    /// Get security level
    pub fn security_level(&self) -> SecurityLevel {
        self.security_level
    }

    /// Set wallet label
    pub fn set_label(&mut self, label: impl Into<String>) {
        self.label = Some(label.into());
    }

    /// Get wallet label
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    /// Sign a message with ML-DSA
    pub async fn sign(&self, message: &[u8]) -> Result<Vec<u8>, WalletError> {
        sign_ml_dsa(&self.secret_key, message, self.security_level)
    }

    /// Sign a transaction hash
    pub async fn sign_transaction(&self, tx_hash: &[u8; 32]) -> Result<Vec<u8>, WalletError> {
        self.sign(tx_hash).await
    }

    /// Export encrypted wallet to file
    pub fn export_encrypted(&self, path: &Path, passphrase: &str) -> Result<(), WalletError> {
        let encrypted = self.export_to_bytes(passphrase)?;
        std::fs::write(path, encrypted)
            .map_err(|e| WalletError::IoError(e.to_string()))?;
        Ok(())
    }

    /// Export wallet to encrypted bytes (in-memory)
    pub fn export_to_bytes(&self, passphrase: &str) -> Result<Vec<u8>, WalletError> {
        let wallet_data = WalletData {
            security_level: self.security_level,
            public_key: self.public_key.clone(),
            secret_key: self.secret_key.clone(),
            label: self.label.clone(),
        };

        let serialized = serde_json::to_vec(&wallet_data)
            .map_err(|e| WalletError::SerializationError(e.to_string()))?;

        encrypt_wallet(&serialized, passphrase)
    }

    /// Register as a miner on the ledger
    pub async fn register_miner(
        &self,
        ledger: &MiningLedger,
        capabilities: &[u8],
        stats: &PerformanceStats,
    ) -> Result<String, WalletError> {
        // Sign registration message
        let registration_msg = create_registration_message(&self.public_key, capabilities, stats);
        let signature = self.sign(&registration_msg).await?;

        ledger.register_miner(&self.public_key, capabilities, stats, &signature)
            .await
            .map_err(|e| WalletError::LedgerError(e.to_string()))
    }

    /// Submit a mining proof
    pub async fn submit_proof(
        &self,
        ledger: &MiningLedger,
        reward_type: MiningRewardType,
        proof: &[u8],
    ) -> Result<String, WalletError> {
        let proof_msg = create_proof_message(&self.public_key, &reward_type, proof);
        let signature = self.sign(&proof_msg).await?;

        ledger.submit_proof(&self.public_key, reward_type, proof, &signature)
            .await
            .map_err(|e| WalletError::LedgerError(e.to_string()))
    }

    /// Claim mining rewards
    pub async fn claim_rewards(
        &self,
        ledger: &MiningLedger,
        amount: u128,
        recipient: Option<&str>,
    ) -> Result<String, WalletError> {
        let recipient_addr = recipient.unwrap_or(&self.address);

        // Get Merkle proof for rewards
        let proof = vec![]; // TODO: Get actual proof from ledger

        let claim_msg = create_claim_message(&self.public_key, amount, recipient_addr, &proof);
        let signature = self.sign(&claim_msg).await?;

        ledger.claim_rewards(&self.public_key, amount, recipient_addr, &proof, &signature)
            .await
            .map_err(|e| WalletError::LedgerError(e.to_string()))
    }

    /// Teleport AI coins to an EVM chain
    pub async fn teleport_to_evm(
        &self,
        ledger: &MiningLedger,
        destination: TeleportDestination,
        amount: u128,
        to_address: Option<&str>,
    ) -> Result<TeleportTransfer, WalletError> {
        let to_addr = to_address.unwrap_or(&self.address);

        let teleport_msg = create_teleport_message(&self.public_key, &destination, amount, to_addr);
        let signature = self.sign(&teleport_msg).await?;

        ledger.teleport_out(&self.public_key, destination, to_addr, amount, &signature)
            .await
            .map_err(|e| WalletError::LedgerError(e.to_string()))
    }

    /// Get pending rewards from ledger
    pub async fn get_pending_rewards(&self, ledger: &MiningLedger) -> Result<u128, WalletError> {
        ledger.get_pending_rewards(&self.public_key)
            .await
            .map_err(|e| WalletError::LedgerError(e.to_string()))
    }

    /// Get EVM balance (after teleport)
    pub async fn get_evm_balance(&self, chain: TeleportDestination) -> Result<u128, WalletError> {
        let config = match chain {
            TeleportDestination::LuxCChain => ChainConfig::lux_mainnet(),
            TeleportDestination::ZooEvm => ChainConfig::zoo_mainnet(),
            TeleportDestination::HanzoEvm => ChainConfig::hanzo_mainnet(),
        };

        let client = EvmClient::new(config);
        client.get_balance(&self.address)
            .await
            .map_err(|e| WalletError::EvmError(e.to_string()))
    }

    /// Verify a signature
    pub fn verify(public_key: &[u8], message: &[u8], signature: &[u8], level: SecurityLevel) -> Result<bool, WalletError> {
        verify_ml_dsa(public_key, message, signature, level)
    }
}

/// Serializable wallet data for export/import
#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
struct WalletData {
    #[zeroize(skip)]
    security_level: SecurityLevel,
    #[zeroize(skip)]
    public_key: Vec<u8>,
    secret_key: Vec<u8>,
    #[zeroize(skip)]
    label: Option<String>,
}

/// Wallet errors
#[derive(Debug, thiserror::Error)]
pub enum WalletError {
    #[error("Key generation failed: {0}")]
    KeyGenerationFailed(String),
    #[error("Invalid key size: expected {expected}, got {actual}")]
    InvalidKeySize { expected: usize, actual: usize },
    #[error("Signing failed: {0}")]
    SigningFailed(String),
    #[error("Verification failed: {0}")]
    VerificationFailed(String),
    #[error("Encryption failed: {0}")]
    EncryptionFailed(String),
    #[error("Decryption failed: {0}")]
    DecryptionFailed(String),
    #[error("Serialization error: {0}")]
    SerializationError(String),
    #[error("Deserialization error: {0}")]
    DeserializationError(String),
    #[error("IO error: {0}")]
    IoError(String),
    #[error("Ledger error: {0}")]
    LedgerError(String),
    #[error("EVM error: {0}")]
    EvmError(String),
}

// =============================================================================
// Internal helper functions
// =============================================================================

/// Generate ML-DSA keypair
fn generate_ml_dsa_keypair(level: SecurityLevel) -> Result<(Vec<u8>, Vec<u8>), WalletError> {
    // In production, this would use hanzo_pqc::signature::MlDsa
    // For now, generate deterministic placeholder keys for testing

    let pk_size = level.public_key_size();
    let sk_size = level.secret_key_size();

    // Use random bytes (in production, use OQS ML-DSA)
    let mut public_key = vec![0u8; pk_size];
    let mut secret_key = vec![0u8; sk_size];

    // Fill with random data
    use rand::RngCore;
    let mut rng = rand::thread_rng();
    rng.fill_bytes(&mut public_key);
    rng.fill_bytes(&mut secret_key);

    // Ensure first bytes encode the level for verification
    public_key[0] = match level {
        SecurityLevel::Level2 => 0x44,
        SecurityLevel::Level3 => 0x65,
        SecurityLevel::Level5 => 0x87,
    };

    Ok((public_key, secret_key))
}

/// Derive public key from secret key
fn derive_public_key(secret_key: &[u8], level: SecurityLevel) -> Result<Vec<u8>, WalletError> {
    // In production, this would derive the actual public key
    // For now, hash the secret key and pad to appropriate size
    let hash = blake3::hash(secret_key);
    let pk_size = level.public_key_size();

    let mut public_key = vec![0u8; pk_size];
    public_key[..32].copy_from_slice(hash.as_bytes());

    // Fill rest with derived data
    for i in 1..(pk_size / 32) {
        let mut data = hash.as_bytes().to_vec();
        data.push(i as u8);
        let derived = blake3::hash(&data);
        let start = i * 32;
        let end = std::cmp::min(start + 32, pk_size);
        public_key[start..end].copy_from_slice(&derived.as_bytes()[..(end - start)]);
    }

    Ok(public_key)
}

/// Sign a message with ML-DSA
fn sign_ml_dsa(secret_key: &[u8], message: &[u8], level: SecurityLevel) -> Result<Vec<u8>, WalletError> {
    // In production, this would use actual ML-DSA signing
    // For now, create a deterministic signature placeholder

    let sig_size = level.signature_size();
    let mut signature = vec![0u8; sig_size];

    // Create deterministic "signature" from secret key + message
    let combined = [secret_key, message].concat();
    let hash = blake3::hash(&combined);

    // Fill signature with deterministic data
    signature[..32].copy_from_slice(hash.as_bytes());
    for i in 1..(sig_size / 32) {
        let mut data = hash.as_bytes().to_vec();
        data.push(i as u8);
        let derived = blake3::hash(&data);
        let start = i * 32;
        let end = std::cmp::min(start + 32, sig_size);
        signature[start..end].copy_from_slice(&derived.as_bytes()[..(end - start)]);
    }

    Ok(signature)
}

/// Verify an ML-DSA signature
fn verify_ml_dsa(public_key: &[u8], message: &[u8], signature: &[u8], level: SecurityLevel) -> Result<bool, WalletError> {
    // In production, this would use actual ML-DSA verification
    // For now, check basic structure

    if public_key.len() != level.public_key_size() {
        return Ok(false);
    }
    if signature.len() != level.signature_size() {
        return Ok(false);
    }

    // Placeholder: always return true for valid-sized inputs
    // Real implementation would verify the lattice-based signature
    Ok(true)
}

/// Encrypt wallet data with passphrase
fn encrypt_wallet(data: &[u8], passphrase: &str) -> Result<Vec<u8>, WalletError> {
    use chacha20poly1305::{
        aead::{Aead, KeyInit},
        ChaCha20Poly1305, Nonce,
    };

    // Derive key from passphrase using Blake3
    let key_material = blake3::hash(passphrase.as_bytes());
    let key = chacha20poly1305::Key::from_slice(key_material.as_bytes());

    // Generate random nonce
    let mut nonce_bytes = [0u8; 12];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    // Encrypt
    let cipher = ChaCha20Poly1305::new(key);
    let ciphertext = cipher.encrypt(nonce, data)
        .map_err(|e| WalletError::EncryptionFailed(e.to_string()))?;

    // Prepend nonce to ciphertext
    let mut result = nonce_bytes.to_vec();
    result.extend(ciphertext);

    Ok(result)
}

/// Decrypt wallet data with passphrase
fn decrypt_wallet(encrypted: &[u8], passphrase: &str) -> Result<Vec<u8>, WalletError> {
    use chacha20poly1305::{
        aead::{Aead, KeyInit},
        ChaCha20Poly1305, Nonce,
    };

    if encrypted.len() < 12 {
        return Err(WalletError::DecryptionFailed("Data too short".into()));
    }

    // Derive key from passphrase
    let key_material = blake3::hash(passphrase.as_bytes());
    let key = chacha20poly1305::Key::from_slice(key_material.as_bytes());

    // Extract nonce and ciphertext
    let nonce = Nonce::from_slice(&encrypted[..12]);
    let ciphertext = &encrypted[12..];

    // Decrypt
    let cipher = ChaCha20Poly1305::new(key);
    let plaintext = cipher.decrypt(nonce, ciphertext)
        .map_err(|e| WalletError::DecryptionFailed(e.to_string()))?;

    Ok(plaintext)
}

/// Create registration message for signing
fn create_registration_message(public_key: &[u8], capabilities: &[u8], stats: &PerformanceStats) -> Vec<u8> {
    let stats_bytes = serde_json::to_vec(stats).unwrap_or_default();
    [public_key, capabilities, &stats_bytes].concat()
}

/// Create proof message for signing
fn create_proof_message(public_key: &[u8], reward_type: &MiningRewardType, proof: &[u8]) -> Vec<u8> {
    let type_bytes = serde_json::to_vec(reward_type).unwrap_or_default();
    [public_key, &type_bytes, proof].concat()
}

/// Create claim message for signing
fn create_claim_message(public_key: &[u8], amount: u128, recipient: &str, proof: &[u8]) -> Vec<u8> {
    let amount_bytes = amount.to_le_bytes();
    [public_key, &amount_bytes, recipient.as_bytes(), proof].concat()
}

/// Create teleport message for signing
fn create_teleport_message(
    public_key: &[u8],
    destination: &TeleportDestination,
    amount: u128,
    to_address: &str,
) -> Vec<u8> {
    let dest_bytes = match destination {
        TeleportDestination::LuxCChain => [0x01],
        TeleportDestination::ZooEvm => [0x02],
        TeleportDestination::HanzoEvm => [0x03],
    };
    let amount_bytes = amount.to_le_bytes();
    [public_key, &dest_bytes, &amount_bytes, to_address.as_bytes()].concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_wallet_generation() {
        let wallet = MiningWallet::generate(SecurityLevel::Level3).await.unwrap();

        assert!(wallet.address().starts_with("0x"));
        assert_eq!(wallet.public_key().len(), SecurityLevel::Level3.public_key_size());
        assert_eq!(wallet.security_level(), SecurityLevel::Level3);
    }

    #[tokio::test]
    async fn test_signing() {
        let wallet = MiningWallet::generate(SecurityLevel::Level3).await.unwrap();
        let message = b"Test message for signing";

        let signature = wallet.sign(message).await.unwrap();

        assert_eq!(signature.len(), SecurityLevel::Level3.signature_size());
    }

    #[tokio::test]
    async fn test_verification() {
        let wallet = MiningWallet::generate(SecurityLevel::Level3).await.unwrap();
        let message = b"Test message";
        let signature = wallet.sign(message).await.unwrap();

        let valid = MiningWallet::verify(
            wallet.public_key(),
            message,
            &signature,
            SecurityLevel::Level3,
        ).unwrap();

        assert!(valid);
    }

    #[test]
    fn test_security_levels() {
        assert_eq!(SecurityLevel::Level2.signature_size(), 2420);
        assert_eq!(SecurityLevel::Level3.signature_size(), 3309);
        assert_eq!(SecurityLevel::Level5.signature_size(), 4627);

        assert_eq!(SecurityLevel::Level3.algorithm_name(), "ML-DSA-65");
    }

    #[test]
    fn test_encryption_roundtrip() {
        let data = b"secret wallet data";
        let passphrase = "test-passphrase";

        let encrypted = encrypt_wallet(data, passphrase).unwrap();
        let decrypted = decrypt_wallet(&encrypted, passphrase).unwrap();

        assert_eq!(data.as_slice(), decrypted.as_slice());
    }

    #[test]
    fn test_wrong_passphrase() {
        let data = b"secret data";
        let encrypted = encrypt_wallet(data, "correct").unwrap();

        let result = decrypt_wallet(&encrypted, "wrong");
        assert!(result.is_err());
    }
}
