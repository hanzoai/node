//! Zoo-1 Multi-tier Simulation System
//!
//! Provides three tiers of simulation:
//! - Tier A: V-JEPA2 latent world model (cheapest)
//! - Tier B: WebXR/WebGPU lightweight spatial (browser)
//! - Tier C: High-fidelity physics (MuJoCo/Brax)

pub mod tiers;
pub mod usd_layers;
pub mod sql_brain;
pub mod orchestrator;
pub mod metaverse;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Simulation tier selection
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SimulationTier {
    /// Latent mental simulation using V-JEPA2
    Latent,
    /// Lightweight 3D with WebGPU/WebXR
    Lightweight,
    /// High-fidelity physics simulation
    HighFidelity,
}

/// Expected Free Energy for tier selection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectedFreeEnergy {
    pub epistemic_value: f64,  // Information gain
    pub pragmatic_value: f64,   // Goal achievement
    pub cost: f64,              // Computational cost
}

impl ExpectedFreeEnergy {
    /// Select optimal simulation tier based on EFE
    pub fn select_tier(&self) -> SimulationTier {
        let value_of_information = self.epistemic_value + self.pragmatic_value;

        if value_of_information < 0.3 {
            SimulationTier::Latent
        } else if value_of_information < 0.7 {
            SimulationTier::Lightweight
        } else {
            SimulationTier::HighFidelity
        }
    }
}

/// Simulation request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationRequest {
    pub question: String,
    pub scene_ref: String,  // USD stage reference
    pub user_id: String,
    pub budget: SimulationBudget,
    pub policy: SimulationPolicy,
}

/// Resource budget for simulation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationBudget {
    pub max_seconds: u64,
    pub max_gpu_seconds: u64,
    pub max_memory_gb: f32,
}

/// Simulation execution policy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationPolicy {
    pub tier: TierSelection,
    pub risk_tolerance: RiskTolerance,
    pub parallelism: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TierSelection {
    Auto,
    Fixed(SimulationTier),
    Progressive, // Start with Latent, escalate if needed
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RiskTolerance {
    Low,
    Medium,
    High,
}

/// Simulation result with artifacts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationResult {
    pub answer: String,
    pub confidence: f64,
    pub tier_used: SimulationTier,
    pub artifacts: Vec<SimulationArtifact>,
    pub metrics: SimulationMetrics,
    pub cost: SimulationCost,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationArtifact {
    pub artifact_type: ArtifactType,
    pub uri: String,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArtifactType {
    Video,
    Image,
    UsdLayer,
    GltfScene,
    Trajectory,
    Metrics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationMetrics {
    pub episodes_run: u32,
    pub success_rate: f64,
    pub avg_reward: f64,
    pub collision_count: u32,
    pub physics_violations: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationCost {
    pub compute_seconds: f64,
    pub gpu_seconds: f64,
    pub memory_gb_hours: f64,
    pub estimated_usd: f64,
}

/// Main simulation orchestrator
pub struct SimulationOrchestrator {
    latent_tier: Arc<tiers::latent::LatentSimulator>,
    lightweight_tier: Arc<tiers::lightweight::LightweightSimulator>,
    highfidelity_tier: Arc<tiers::highfidelity::HighFidelitySimulator>,
    usd_manager: Arc<usd_layers::UsdLayerManager>,
    sql_brain: Arc<sql_brain::SqlBrain>,
}

impl SimulationOrchestrator {
    pub async fn new(config: OrchestratorConfig) -> Result<Self> {
        Ok(Self {
            latent_tier: Arc::new(tiers::latent::LatentSimulator::new(config.latent_config).await?),
            lightweight_tier: Arc::new(tiers::lightweight::LightweightSimulator::new(config.lightweight_config).await?),
            highfidelity_tier: Arc::new(tiers::highfidelity::HighFidelitySimulator::new(config.hf_config).await?),
            usd_manager: Arc::new(usd_layers::UsdLayerManager::new(config.usd_config)?),
            sql_brain: Arc::new(sql_brain::SqlBrain::new(&config.sql_path).await?),
        })
    }

    /// Process a simulation request
    pub async fn simulate(&self, request: SimulationRequest) -> Result<SimulationResult> {
        // 1. Load user context from SQL brain
        let user_context = self.sql_brain.load_user_context(&request.user_id).await?;

        // 2. Create USD session layer for cloning
        let session_layer = self.usd_manager.create_session_layer(&request.scene_ref).await?;

        // 3. Calculate Expected Free Energy
        let efe = self.calculate_efe(&request, &user_context).await?;

        // 4. Select tier based on EFE and policy
        let tier = match request.policy.tier {
            TierSelection::Auto => efe.select_tier(),
            TierSelection::Fixed(tier) => tier,
            TierSelection::Progressive => SimulationTier::Latent, // Start with cheapest
        };

        // 5. Execute simulation on selected tier
        let result = match tier {
            SimulationTier::Latent => {
                self.latent_tier.simulate(&request, &session_layer).await?
            }
            SimulationTier::Lightweight => {
                self.lightweight_tier.simulate(&request, &session_layer).await?
            }
            SimulationTier::HighFidelity => {
                self.highfidelity_tier.simulate(&request, &session_layer).await?
            }
        };

        // 6. Store results in SQL brain
        self.sql_brain.store_episode(&request.user_id, &result).await?;

        // 7. Optionally escalate if confidence too low
        if request.policy.tier == TierSelection::Progressive && result.confidence < 0.6 {
            // Escalate to next tier
            let next_tier = match tier {
                SimulationTier::Latent => SimulationTier::Lightweight,
                SimulationTier::Lightweight => SimulationTier::HighFidelity,
                SimulationTier::HighFidelity => SimulationTier::HighFidelity,
            };

            if next_tier != tier {
                // Run again on higher tier
                return self.simulate_with_tier(request, next_tier).await;
            }
        }

        Ok(result)
    }

    async fn calculate_efe(&self, request: &SimulationRequest, context: &sql_brain::UserContext) -> Result<ExpectedFreeEnergy> {
        // Calculate value of information based on question complexity and user history
        let epistemic_value = self.estimate_information_gain(&request.question, context).await?;
        let pragmatic_value = self.estimate_goal_value(&request.question, context).await?;
        let cost = self.estimate_compute_cost(SimulationTier::Latent);

        Ok(ExpectedFreeEnergy {
            epistemic_value,
            pragmatic_value,
            cost,
        })
    }

    async fn estimate_information_gain(&self, question: &str, context: &sql_brain::UserContext) -> Result<f64> {
        // Estimate based on question novelty and complexity
        // This would use embeddings to check similarity with past questions
        Ok(0.5) // Placeholder
    }

    async fn estimate_goal_value(&self, question: &str, context: &sql_brain::UserContext) -> Result<f64> {
        // Estimate value for achieving user's goals
        Ok(0.3) // Placeholder
    }

    fn estimate_compute_cost(&self, tier: SimulationTier) -> f64 {
        match tier {
            SimulationTier::Latent => 0.001,       // $0.001 per simulation
            SimulationTier::Lightweight => 0.01,   // $0.01 per simulation
            SimulationTier::HighFidelity => 0.10,  // $0.10 per simulation
        }
    }

    async fn simulate_with_tier(&self, request: SimulationRequest, tier: SimulationTier) -> Result<SimulationResult> {
        let mut req = request;
        req.policy.tier = TierSelection::Fixed(tier);
        self.simulate(req).await
    }
}

/// Orchestrator configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorConfig {
    pub latent_config: tiers::latent::LatentConfig,
    pub lightweight_config: tiers::lightweight::LightweightConfig,
    pub hf_config: tiers::highfidelity::HighFidelityConfig,
    pub usd_config: usd_layers::UsdConfig,
    pub sql_path: String,
}