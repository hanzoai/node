//! Hamiltonian Hidden-Markov LLM (HLLM) Routing System
//! 
//! A physics-inspired regime-based routing system for optimal model selection.
//! Combines Hidden Markov Models for regime detection with Hamiltonian mechanics
//! for price dynamics and BitDelta quantization for efficient adaptation.

pub mod regime;
pub mod hamiltonian;
pub mod bitdelta;
pub mod routing;
pub mod adapter;
pub mod storage;
pub mod free_energy;

use std::sync::Arc;
use async_trait::async_trait;
use serde::{Serialize, Deserialize};
use anyhow::Result;

pub use regime::{Regime, RegimeDetector, MarkovChain};
pub use hamiltonian::{HamiltonianDynamics, PhaseSpace, PriceDynamics};
pub use bitdelta::{BitDeltaAdapter, BitQuantizer, AdapterCache};
pub use routing::{HLLMRouter, RoutingDecision, ModelSelection};
pub use adapter::{UserAdapter, AdapterManager};
pub use storage::{HLLMStorage, VectorIndex};
pub use free_energy::{ExpectedFreeEnergy, BeliefState, Precision};

/// Main HLLM system configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HLLMConfig {
    /// Number of regime states
    pub num_regimes: usize,
    
    /// Transition probability threshold
    pub transition_threshold: f64,
    
    /// Hamiltonian energy scale
    pub energy_scale: f64,
    
    /// BitDelta quantization bits
    pub quantization_bits: usize,
    
    /// Expected free energy precision
    pub efe_precision: f64,
    
    /// Database path for storage
    pub db_path: String,
    
    /// Vector dimension for embeddings
    pub vector_dim: usize,
    
    /// Cache size for adapters
    pub adapter_cache_size: usize,
}

impl Default for HLLMConfig {
    fn default() -> Self {
        Self {
            num_regimes: 4,  // Exploration, Exploitation, Crisis, Transition
            transition_threshold: 0.15,
            energy_scale: 1.0,
            quantization_bits: 1,  // 1-bit for BitDelta
            efe_precision: 0.01,
            db_path: "./hllm_storage.db".to_string(),
            vector_dim: 768,  // Standard embedding dimension
            adapter_cache_size: 100,
        }
    }
}

/// Main HLLM system for routing decisions
pub struct HLLM {
    config: HLLMConfig,
    regime_detector: Arc<RegimeDetector>,
    hamiltonian: Arc<HamiltonianDynamics>,
    router: Arc<HLLMRouter>,
    adapter_manager: Arc<AdapterManager>,
    storage: Arc<HLLMStorage>,
}

impl HLLM {
    /// Create a new HLLM system
    pub async fn new(config: HLLMConfig) -> Result<Self> {
        let storage = Arc::new(HLLMStorage::new(&config.db_path).await?);
        
        let regime_detector = Arc::new(RegimeDetector::new(
            config.num_regimes,
            config.transition_threshold,
        )?);
        
        let hamiltonian = Arc::new(HamiltonianDynamics::new(
            config.energy_scale,
            config.vector_dim,
        ));
        
        let router = Arc::new(HLLMRouter::new(
            regime_detector.clone(),
            hamiltonian.clone(),
        ));
        
        let adapter_manager = Arc::new(AdapterManager::new(
            storage.clone(),
            config.adapter_cache_size,
            config.quantization_bits,
        ));
        
        Ok(Self {
            config,
            regime_detector,
            hamiltonian,
            router,
            adapter_manager,
            storage,
        })
    }
    
    /// Get routing decision for a request
    pub async fn route_request(
        &self,
        user_id: &str,
        request: &RoutingRequest,
    ) -> Result<RoutingDecision> {
        // Get user adapter
        let adapter_lock = self.adapter_manager.get_or_create(user_id).await?;
        
        // Detect current regime
        let regime = self.regime_detector.detect_regime(&request.observations)?;
        
        // Calculate expected free energy
        let efe = {
            let adapter = adapter_lock.read().await;
            self.calculate_efe(&*adapter, &regime, request).await?
        };
        
        // Get routing decision
        let decision = {
            let adapter = adapter_lock.read().await;
            self.router.decide(
                regime,
                efe,
                adapter.get_adapter(),
                request,
            ).await?
        };
        
        // Update adapter based on decision
        self.adapter_manager.update_adapter(
            user_id,
            &decision,
            request,
        ).await?;
        
        Ok(decision)
    }
    
    /// Calculate expected free energy for decision making
    async fn calculate_efe(
        &self,
        adapter: &UserAdapter,
        regime: &Regime,
        request: &RoutingRequest,
    ) -> Result<ExpectedFreeEnergy> {
        let belief_state = BeliefState::from_request(request)?;
        let precision = Precision::from_regime(regime);
        
        ExpectedFreeEnergy::calculate(
            belief_state,
            precision,
            &adapter.get_parameters(),
        )
    }
    
    /// Get current system state
    pub async fn get_state(&self) -> Result<SystemState> {
        let regime_state = self.regime_detector.get_current_state()?;
        let phase_space = self.hamiltonian.get_phase_space()?;
        let cache_stats = self.adapter_manager.get_cache_stats().await?;
        
        Ok(SystemState {
            current_regime: regime_state.current_regime,
            transition_probabilities: regime_state.transition_matrix,
            hamiltonian_energy: phase_space.total_energy(),
            active_adapters: cache_stats.active_count,
            total_requests: cache_stats.total_requests,
        })
    }
}

/// Request for routing decision
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingRequest {
    /// Input text or prompt
    pub input: String,
    
    /// Context history
    pub context: Vec<String>,
    
    /// User preferences
    pub preferences: UserPreferences,
    
    /// Performance requirements
    pub requirements: PerformanceRequirements,
    
    /// Observations for regime detection
    pub observations: Vec<f64>,
}

/// User preferences for model selection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPreferences {
    pub max_latency_ms: Option<u64>,
    pub max_cost_per_token: Option<f64>,
    pub preferred_models: Vec<String>,
    pub quality_threshold: f64,
}

/// Performance requirements
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceRequirements {
    pub min_tokens_per_second: Option<f64>,
    pub max_memory_gb: Option<f64>,
    pub requires_function_calling: bool,
    pub requires_vision: bool,
}

/// System state snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemState {
    pub current_regime: Regime,
    pub transition_probabilities: Vec<Vec<f64>>,
    pub hamiltonian_energy: f64,
    pub active_adapters: usize,
    pub total_requests: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_hllm_creation() {
        let config = HLLMConfig::default();
        let hllm = HLLM::new(config).await.unwrap();
        let state = hllm.get_state().await.unwrap();
        
        assert!(state.hamiltonian_energy >= 0.0);
        assert_eq!(state.current_regime, Regime::Exploration);
    }
}