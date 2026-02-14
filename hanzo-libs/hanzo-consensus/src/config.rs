// Copyright (C) 2024-2025, Hanzo AI Inc. All rights reserved.
// Consensus configuration mapping Hanzo network presets to QuasarConfig.

use lux_consensus::QuasarConfig;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Hanzo consensus configuration.
///
/// Provides a simplified interface over the full `QuasarConfig`, exposing
/// only the parameters that Hanzo node operators need to reason about.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HanzoConsensusConfig {
    /// Committee / sample size (maps to QuasarConfig.k).
    pub committee_size: usize,
    /// Quorum threshold ratio 0.5..0.8 (maps to QuasarConfig.alpha).
    pub threshold: f64,
    /// Consecutive rounds for finality (maps to QuasarConfig.beta).
    pub finality_rounds: u32,
    /// Per-round timeout in milliseconds.
    pub round_timeout_ms: u64,
    /// Enable post-quantum (Ringtail) dual signatures.
    pub pq_enabled: bool,
    /// Enable FPC adaptive thresholds.
    pub fpc_enabled: bool,
    /// Network name for logging / metrics.
    pub network: String,
}

impl HanzoConsensusConfig {
    /// Devnet preset: small committee, fast finality, no PQ overhead.
    pub fn devnet() -> Self {
        HanzoConsensusConfig {
            committee_size: 5,
            threshold: 0.60,
            finality_rounds: 3,
            round_timeout_ms: 30,
            pq_enabled: false,
            fpc_enabled: false,
            network: "devnet".to_string(),
        }
    }

    /// Testnet preset: medium committee, moderate finality.
    pub fn testnet() -> Self {
        HanzoConsensusConfig {
            committee_size: 11,
            threshold: 0.65,
            finality_rounds: 8,
            round_timeout_ms: 50,
            pq_enabled: false,
            fpc_enabled: true,
            network: "testnet".to_string(),
        }
    }

    /// Mainnet preset: production committee, PQ enabled, strict finality.
    pub fn mainnet() -> Self {
        HanzoConsensusConfig {
            committee_size: 21,
            threshold: 0.69,
            finality_rounds: 20,
            round_timeout_ms: 100,
            pq_enabled: true,
            fpc_enabled: true,
            network: "mainnet".to_string(),
        }
    }
}

/// Convert HanzoConsensusConfig into the full QuasarConfig expected by lux-consensus.
impl From<&HanzoConsensusConfig> for QuasarConfig {
    fn from(hc: &HanzoConsensusConfig) -> Self {
        let base = match hc.network.as_str() {
            "mainnet" => QuasarConfig::mainnet(),
            "testnet" => QuasarConfig::testnet(),
            _ => QuasarConfig::testnet(), // devnet uses testnet base
        };

        QuasarConfig {
            k: hc.committee_size,
            alpha: hc.threshold,
            beta: hc.finality_rounds,
            round_timeout: Duration::from_millis(hc.round_timeout_ms),
            enable_fpc: hc.fpc_enabled,
            quantum_resistant: hc.pq_enabled,
            // Inherit remaining parameters from the base preset
            theta_min: base.theta_min,
            theta_max: base.theta_max,
            fpc_seed: base.fpc_seed,
            base_luminance: base.base_luminance,
            max_luminance: base.max_luminance,
            min_luminance: base.min_luminance,
            success_multiplier: base.success_multiplier,
            failure_multiplier: base.failure_multiplier,
            network_timeout: base.network_timeout,
            max_message_size: base.max_message_size,
            max_outstanding: base.max_outstanding,
            security_level: base.security_level,
            gpu_acceleration: base.gpu_acceleration,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devnet_maps_correctly() {
        let hc = HanzoConsensusConfig::devnet();
        let qc: QuasarConfig = (&hc).into();
        assert_eq!(qc.k, 5);
        assert!((qc.alpha - 0.60).abs() < f64::EPSILON);
        assert_eq!(qc.beta, 3);
        assert!(!qc.quantum_resistant);
        assert!(!qc.enable_fpc);
    }

    #[test]
    fn testnet_maps_correctly() {
        let hc = HanzoConsensusConfig::testnet();
        let qc: QuasarConfig = (&hc).into();
        assert_eq!(qc.k, 11);
        assert!((qc.alpha - 0.65).abs() < f64::EPSILON);
        assert_eq!(qc.beta, 8);
        assert!(!qc.quantum_resistant);
        assert!(qc.enable_fpc);
    }

    #[test]
    fn mainnet_maps_correctly() {
        let hc = HanzoConsensusConfig::mainnet();
        let qc: QuasarConfig = (&hc).into();
        assert_eq!(qc.k, 21);
        assert!((qc.alpha - 0.69).abs() < f64::EPSILON);
        assert_eq!(qc.beta, 20);
        assert!(qc.quantum_resistant);
        assert!(qc.enable_fpc);
    }

    #[test]
    fn round_timeout_propagates() {
        let hc = HanzoConsensusConfig {
            round_timeout_ms: 250,
            ..HanzoConsensusConfig::devnet()
        };
        let qc: QuasarConfig = (&hc).into();
        assert_eq!(qc.round_timeout, Duration::from_millis(250));
    }
}
