//! Hanzo KBS (Key Broker Service) Library
//! 
//! Provides Key Management Service (KMS) and Key Broker Service (KBS) functionality
//! for confidential computing and privacy-preserving agent execution in Hanzo nodes.
//! 
//! This crate implements the KMS/KBS split architecture where:
//! - KMS handles key lifecycle management and storage
//! - KBS handles attestation verification and policy-based key release

pub mod error;
pub mod types;
pub mod kms;
pub mod kbs;
pub mod attestation;
pub mod vault;

#[cfg(feature = "pqc")]
pub mod pqc_integration;

#[cfg(feature = "pqc")]
pub mod pqc_vault;

pub use error::{SecurityError, Result};
pub use types::*;
pub use kms::KeyManagementService;
pub use kbs::KeyBrokerService;

// Re-export submodules from kms
pub use kms::{memory_kms, api};