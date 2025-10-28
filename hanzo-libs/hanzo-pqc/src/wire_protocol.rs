//! Wire protocol for PQC-enabled P2P handshake and key exchange

use serde::{Deserialize, Serialize};
use crate::{
    hybrid::HybridCiphertext,
    signature::DigitalSignature,
    privacy_tiers::PrivacyTier,
};

/// P2P handshake message with PQC support
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeMessage {
    /// Protocol version
    pub version: u8,
    /// Sender identity
    pub sender_id: Vec<u8>,
    /// ML-KEM public key
    pub mlkem_pubkey: Vec<u8>,
    /// X25519 public key (for hybrid mode)
    pub x25519_pubkey: Option<Vec<u8>>,
    /// Supported cipher suites
    pub suites: Vec<CipherSuite>,
    /// Client nonce
    pub nonce: [u8; 32],
    /// Encapsulated ciphertext (server response)
    pub encap_ct: Option<HybridCiphertext>,
    /// ML-DSA signature over transcript
    pub signature: Option<DigitalSignature>,
    /// Privacy tier capabilities
    pub privacy_tier: Option<PrivacyTier>,
}

/// Cipher suite definition
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CipherSuite {
    pub kem: String,        // e.g., "ML-KEM-768", "ML-KEM-768+X25519"
    pub sig: String,        // e.g., "ML-DSA-65", "Ed25519"
    pub aead: String,       // e.g., "ChaCha20-Poly1305", "AES-256-GCM"
    pub kdf: String,        // e.g., "HKDF-SHA384"
}

impl CipherSuite {
    /// Default PQC suite (ML-KEM-768 + ML-DSA-65)
    pub fn default_pqc() -> Self {
        Self {
            kem: "ML-KEM-768".to_string(),
            sig: "ML-DSA-65".to_string(),
            aead: "ChaCha20-Poly1305".to_string(),
            kdf: "HKDF-SHA384".to_string(),
        }
    }
    
    /// Hybrid suite (ML-KEM-768+X25519)
    pub fn default_hybrid() -> Self {
        Self {
            kem: "ML-KEM-768+X25519".to_string(),
            sig: "ML-DSA-65".to_string(),
            aead: "ChaCha20-Poly1305".to_string(),
            kdf: "HKDF-SHA384".to_string(),
        }
    }
    
    /// Legacy suite for backward compatibility
    pub fn legacy() -> Self {
        Self {
            kem: "X25519".to_string(),
            sig: "Ed25519".to_string(),
            aead: "ChaCha20-Poly1305".to_string(),
            kdf: "HKDF-SHA256".to_string(),
        }
    }
}

/// KBS key release request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyReleaseRequest {
    pub lease_id: String,
    pub node_mode: NodeMode,
    pub node_id: String,
    pub eid: Option<String>,  // SIM EID if available
    pub attestation: Option<AttestationBundle>,
    pub mlkem_encap_key: Vec<u8>,
    pub privacy_tier: PrivacyTier,
}

/// Node operation mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeMode {
    SoftwareOnly,
    SimOnly,
    SimTee,
}

/// Attestation bundle for TEE evidence
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttestationBundle {
    pub cpu_quote: Option<CpuQuote>,
    pub gpu_quote: Option<GpuQuote>,
    pub measurement: Vec<u8>,
    pub policy_hash: [u8; 32],
    pub timestamp: u64,
}

/// CPU TEE quote (SEV-SNP/TDX)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuQuote {
    pub quote_type: CpuTeeType,
    pub quote_data: Vec<u8>,
    pub report_data: Vec<u8>,
    pub measurement: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CpuTeeType {
    SevSnp,
    Tdx,
    Sgx,
    ArmCca,
}

/// GPU attestation quote (NVIDIA NRAS)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuQuote {
    pub gpu_type: GpuType,
    pub nras_token: Vec<u8>,
    pub device_id: Vec<u8>,
    pub cc_enabled: bool,
    pub tee_io_enabled: bool,
    pub mig_uuid: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GpuType {
    H100,
    Blackwell,
    Other,
}

/// KBS key release response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyReleaseResponse {
    pub enc_dek: Vec<u8>,  // ML-KEM encrypted DEK
    pub aead_alg: String,
    pub key_release_cert: KeyReleaseCertificate,
    pub capability_token: Vec<u8>,
}

/// Key release certificate (signed by KBS)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyReleaseCertificate {
    pub lease_id: String,
    pub node_id: String,
    pub measurement: Option<Vec<u8>>,
    pub timestamp: u64,
    pub orr_digest: Vec<u8>,  // Operational Reference Record
    pub privacy_tier: PrivacyTier,
    pub signature: DigitalSignature,  // ML-DSA-65 signature
}

/// Transcript for signature verification
pub struct HandshakeTranscript {
    messages: Vec<Vec<u8>>,
}

impl Default for HandshakeTranscript {
    fn default() -> Self {
        Self::new()
    }
}

impl HandshakeTranscript {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }
    
    pub fn add_message(&mut self, msg: &[u8]) {
        self.messages.push(msg.to_vec());
    }
    
    pub fn get_hash(&self) -> [u8; 32] {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        for msg in &self.messages {
            hasher.update((msg.len() as u32).to_be_bytes());
            hasher.update(msg);
        }
        hasher.finalize().into()
    }
}

/// Protocol constants
pub mod constants {
    /// Current protocol version
    pub const PROTOCOL_VERSION: u8 = 1;
    
    /// Maximum handshake message size
    pub const MAX_HANDSHAKE_SIZE: usize = 16384;
    
    /// Session timeout (seconds)
    pub const SESSION_TIMEOUT: u64 = 3600;
    
    /// Domain separation labels
    pub const LABEL_HANDSHAKE: &str = "hanzo-pqc-handshake-v1";
    pub const LABEL_KEY_RELEASE: &str = "hanzo-pqc-key-release-v1";
    pub const LABEL_HYBRID_KEX: &str = "hanzo-hybrid-v1";
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_cipher_suites() {
        let pqc = CipherSuite::default_pqc();
        assert_eq!(pqc.kem, "ML-KEM-768");
        assert_eq!(pqc.sig, "ML-DSA-65");
        
        let hybrid = CipherSuite::default_hybrid();
        assert!(hybrid.kem.contains("X25519"));
    }
    
    #[test]
    fn test_transcript_hash() {
        let mut transcript = HandshakeTranscript::new();
        transcript.add_message(b"message1");
        transcript.add_message(b"message2");
        
        let hash1 = transcript.get_hash();
        
        // Verify deterministic
        let mut transcript2 = HandshakeTranscript::new();
        transcript2.add_message(b"message1");
        transcript2.add_message(b"message2");
        
        assert_eq!(hash1, transcript2.get_hash());
    }
}