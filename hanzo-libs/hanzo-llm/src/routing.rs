//! HLLM routing logic for regime-based model selection
//! 
//! Routes requests to optimal models based on regime state and dynamics

use std::sync::Arc;
use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use anyhow::{Result, anyhow};
use async_trait::async_trait;

use crate::{
    Regime, RegimeDetector,
    HamiltonianDynamics, PriceDynamics,
    BitDeltaAdapter, ExpectedFreeEnergy,
};

/// HLLM router for intelligent model selection
pub struct HLLMRouter {
    /// Regime detector
    regime_detector: Arc<RegimeDetector>,
    
    /// Hamiltonian dynamics system
    hamiltonian: Arc<HamiltonianDynamics>,
    
    /// Model registry
    model_registry: ModelRegistry,
    
    /// Routing strategy per regime
    routing_strategies: HashMap<Regime, RoutingStrategy>,
    
    /// Price dynamics tracker
    price_dynamics: HashMap<String, PriceDynamics>,
}

impl HLLMRouter {
    /// Create new router
    pub fn new(
        regime_detector: Arc<RegimeDetector>,
        hamiltonian: Arc<HamiltonianDynamics>,
    ) -> Self {
        let model_registry = ModelRegistry::default();
        let routing_strategies = Self::init_routing_strategies();
        let price_dynamics = Self::init_price_dynamics();
        
        Self {
            regime_detector,
            hamiltonian,
            model_registry,
            routing_strategies,
            price_dynamics,
        }
    }
    
    /// Initialize routing strategies for each regime
    fn init_routing_strategies() -> HashMap<Regime, RoutingStrategy> {
        let mut strategies = HashMap::new();
        
        // Exploration: Try new models, higher risk tolerance
        strategies.insert(Regime::Exploration, RoutingStrategy {
            model_preferences: vec![
                ModelPreference { model: "gpt-4-turbo".into(), weight: 0.3 },
                ModelPreference { model: "claude-3-opus".into(), weight: 0.3 },
                ModelPreference { model: "gemini-pro".into(), weight: 0.2 },
                ModelPreference { model: "mixtral-8x7b".into(), weight: 0.2 },
            ],
            cost_weight: 0.2,
            quality_weight: 0.5,
            latency_weight: 0.3,
            innovation_bonus: 0.2,
        });
        
        // Exploitation: Use proven models, optimize cost/performance
        strategies.insert(Regime::Exploitation, RoutingStrategy {
            model_preferences: vec![
                ModelPreference { model: "gpt-3.5-turbo".into(), weight: 0.4 },
                ModelPreference { model: "claude-3-haiku".into(), weight: 0.4 },
                ModelPreference { model: "llama-3-70b".into(), weight: 0.2 },
            ],
            cost_weight: 0.5,
            quality_weight: 0.3,
            latency_weight: 0.2,
            innovation_bonus: 0.0,
        });
        
        // Crisis: Fast, reliable models only
        strategies.insert(Regime::Crisis, RoutingStrategy {
            model_preferences: vec![
                ModelPreference { model: "gpt-3.5-turbo".into(), weight: 0.5 },
                ModelPreference { model: "claude-instant".into(), weight: 0.5 },
            ],
            cost_weight: 0.3,
            quality_weight: 0.2,
            latency_weight: 0.5,
            innovation_bonus: -0.1, // Penalize experimentation
        });
        
        // Transition: Balanced approach
        strategies.insert(Regime::Transition, RoutingStrategy {
            model_preferences: vec![
                ModelPreference { model: "gpt-4".into(), weight: 0.25 },
                ModelPreference { model: "claude-3-sonnet".into(), weight: 0.25 },
                ModelPreference { model: "gpt-3.5-turbo".into(), weight: 0.25 },
                ModelPreference { model: "llama-3-70b".into(), weight: 0.25 },
            ],
            cost_weight: 0.33,
            quality_weight: 0.34,
            latency_weight: 0.33,
            innovation_bonus: 0.05,
        });
        
        strategies
    }
    
    /// Initialize price dynamics for models
    fn init_price_dynamics() -> HashMap<String, PriceDynamics> {
        let mut dynamics = HashMap::new();
        
        // Premium models
        dynamics.insert("gpt-4-turbo".into(), 
            PriceDynamics::new(10.0, 0.2, 0.1));
        dynamics.insert("claude-3-opus".into(), 
            PriceDynamics::new(15.0, 0.25, 0.15));
        
        // Mid-tier models
        dynamics.insert("gpt-4".into(), 
            PriceDynamics::new(30.0, 0.15, 0.1));
        dynamics.insert("claude-3-sonnet".into(), 
            PriceDynamics::new(3.0, 0.1, 0.1));
        dynamics.insert("gemini-pro".into(), 
            PriceDynamics::new(1.0, 0.1, 0.05));
        
        // Budget models
        dynamics.insert("gpt-3.5-turbo".into(), 
            PriceDynamics::new(0.5, 0.05, 0.2));
        dynamics.insert("claude-3-haiku".into(), 
            PriceDynamics::new(0.25, 0.05, 0.2));
        dynamics.insert("claude-instant".into(), 
            PriceDynamics::new(0.8, 0.05, 0.15));
        
        // Open models
        dynamics.insert("llama-3-70b".into(), 
            PriceDynamics::new(0.9, 0.1, 0.1));
        dynamics.insert("mixtral-8x7b".into(), 
            PriceDynamics::new(0.7, 0.1, 0.1));
        
        dynamics
    }
    
    /// Make routing decision
    pub async fn decide(
        &self,
        regime: Regime,
        efe: ExpectedFreeEnergy,
        adapter: &BitDeltaAdapter,
        request: &crate::RoutingRequest,
    ) -> Result<RoutingDecision> {
        let strategy = self.routing_strategies
            .get(&regime)
            .ok_or_else(|| anyhow!("No strategy for regime {:?}", regime))?;
        
        // Score each available model
        let mut model_scores = Vec::new();
        
        for pref in &strategy.model_preferences {
            if let Some(model) = self.model_registry.get(&pref.model) {
                let score = self.score_model(
                    model,
                    strategy,
                    &efe,
                    adapter,
                    request,
                ).await?;
                
                model_scores.push((model.clone(), score));
            }
        }
        
        // Sort by score descending
        model_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        
        if model_scores.is_empty() {
            return Err(anyhow!("No models available for routing"));
        }
        
        // Select best model
        let (selected_model, score) = &model_scores[0];
        
        // Calculate price
        let price = self.calculate_price(
            &selected_model.name,
            request,
        ).await?;
        
        Ok(RoutingDecision {
            model: selected_model.name.clone(),
            provider: selected_model.provider.clone(),
            regime: regime.clone(),
            confidence: score / 100.0, // Normalize to 0-1
            expected_latency_ms: selected_model.avg_latency_ms,
            expected_cost: price,
            reasoning: format!(
                "Selected {} in {:?} regime (score: {:.2}, EFE: {:.4})",
                selected_model.name, regime, score, efe.value()
            ),
            fallback_models: model_scores.iter()
                .skip(1)
                .take(2)
                .map(|(m, _)| m.name.clone())
                .collect(),
        })
    }
    
    /// Score a model based on current conditions
    async fn score_model(
        &self,
        model: &ModelInfo,
        strategy: &RoutingStrategy,
        efe: &ExpectedFreeEnergy,
        adapter: &BitDeltaAdapter,
        request: &crate::RoutingRequest,
    ) -> Result<f64> {
        let mut score = 0.0;
        
        // Base score from strategy preference
        if let Some(pref) = strategy.model_preferences
            .iter()
            .find(|p| p.model == model.name)
        {
            score += pref.weight * 100.0;
        }
        
        // Cost score (inverse - lower is better)
        let cost_score = 100.0 / (1.0 + model.cost_per_1k_tokens);
        score += cost_score * strategy.cost_weight;
        
        // Quality score
        let quality_score = model.quality_score * 100.0;
        score += quality_score * strategy.quality_weight;
        
        // Latency score (inverse - lower is better)
        let latency_score = 10000.0 / (100.0 + model.avg_latency_ms as f64);
        score += latency_score * strategy.latency_weight;
        
        // Innovation bonus/penalty
        if model.capabilities.contains(&"experimental".to_string()) {
            score *= 1.0 + strategy.innovation_bonus;
        }
        
        // EFE adjustment - lower EFE means more certainty, prefer exploitation
        let efe_factor = 1.0 - efe.value().tanh();
        score *= 0.8 + 0.4 * efe_factor;
        
        // User adapter affinity
        if adapter.metadata.regime_history.contains(&model.name) {
            score *= 1.1; // 10% bonus for familiar models
        }
        
        // Requirements check
        if request.requirements.requires_function_calling 
            && !model.capabilities.contains(&"function_calling".to_string()) {
            score *= 0.1; // Heavy penalty
        }
        
        if request.requirements.requires_vision 
            && !model.capabilities.contains(&"vision".to_string()) {
            score *= 0.1; // Heavy penalty
        }
        
        Ok(score)
    }
    
    /// Calculate dynamic price for model
    async fn calculate_price(
        &self,
        model_name: &str,
        request: &crate::RoutingRequest,
    ) -> Result<f64> {
        let base_model = self.model_registry.get(model_name)
            .ok_or_else(|| anyhow!("Model {} not found", model_name))?;
        
        // Get price dynamics if available
        if let Some(dynamics) = self.price_dynamics.get(model_name) {
            let price_per_token = dynamics.cost_per_token();
            
            // Estimate tokens (rough approximation)
            let estimated_tokens = request.input.len() / 4 + 500; // Output estimate
            
            Ok(price_per_token * estimated_tokens as f64)
        } else {
            // Fallback to static pricing
            let estimated_tokens = request.input.len() / 4 + 500;
            Ok(base_model.cost_per_1k_tokens * estimated_tokens as f64 / 1000.0)
        }
    }
}

/// Routing strategy for a regime
#[derive(Debug, Clone)]
struct RoutingStrategy {
    model_preferences: Vec<ModelPreference>,
    cost_weight: f64,
    quality_weight: f64,
    latency_weight: f64,
    innovation_bonus: f64,
}

/// Model preference in strategy
#[derive(Debug, Clone)]
struct ModelPreference {
    model: String,
    weight: f64,
}

/// Routing decision result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingDecision {
    /// Selected model name
    pub model: String,
    
    /// Provider (OpenAI, Anthropic, etc.)
    pub provider: String,
    
    /// Current regime
    pub regime: Regime,
    
    /// Confidence in decision (0-1)
    pub confidence: f64,
    
    /// Expected latency in milliseconds
    pub expected_latency_ms: u64,
    
    /// Expected cost in USD
    pub expected_cost: f64,
    
    /// Reasoning for decision
    pub reasoning: String,
    
    /// Fallback models if primary fails
    pub fallback_models: Vec<String>,
}

/// Model selection criteria
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSelection {
    pub primary: String,
    pub fallbacks: Vec<String>,
    pub criteria: SelectionCriteria,
}

/// Selection criteria for models
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionCriteria {
    pub max_cost: Option<f64>,
    pub max_latency_ms: Option<u64>,
    pub min_quality: Option<f64>,
    pub required_capabilities: Vec<String>,
}

/// Model registry
struct ModelRegistry {
    models: HashMap<String, ModelInfo>,
}

impl ModelRegistry {
    fn get(&self, name: &str) -> Option<&ModelInfo> {
        self.models.get(name)
    }
}

impl Default for ModelRegistry {
    fn default() -> Self {
        let mut models = HashMap::new();
        
        // OpenAI models
        models.insert("gpt-4-turbo".into(), ModelInfo {
            name: "gpt-4-turbo".into(),
            provider: "openai".into(),
            cost_per_1k_tokens: 0.01,
            avg_latency_ms: 2000,
            quality_score: 0.95,
            capabilities: vec!["function_calling".into(), "vision".into()],
        });
        
        models.insert("gpt-4".into(), ModelInfo {
            name: "gpt-4".into(),
            provider: "openai".into(),
            cost_per_1k_tokens: 0.03,
            avg_latency_ms: 3000,
            quality_score: 0.93,
            capabilities: vec!["function_calling".into()],
        });
        
        models.insert("gpt-3.5-turbo".into(), ModelInfo {
            name: "gpt-3.5-turbo".into(),
            provider: "openai".into(),
            cost_per_1k_tokens: 0.0005,
            avg_latency_ms: 500,
            quality_score: 0.75,
            capabilities: vec!["function_calling".into()],
        });
        
        // Anthropic models
        models.insert("claude-3-opus".into(), ModelInfo {
            name: "claude-3-opus".into(),
            provider: "anthropic".into(),
            cost_per_1k_tokens: 0.015,
            avg_latency_ms: 2500,
            quality_score: 0.96,
            capabilities: vec!["vision".into()],
        });
        
        models.insert("claude-3-sonnet".into(), ModelInfo {
            name: "claude-3-sonnet".into(),
            provider: "anthropic".into(),
            cost_per_1k_tokens: 0.003,
            avg_latency_ms: 1500,
            quality_score: 0.88,
            capabilities: vec!["vision".into()],
        });
        
        models.insert("claude-3-haiku".into(), ModelInfo {
            name: "claude-3-haiku".into(),
            provider: "anthropic".into(),
            cost_per_1k_tokens: 0.00025,
            avg_latency_ms: 400,
            quality_score: 0.72,
            capabilities: vec!["vision".into()],
        });
        
        models.insert("claude-instant".into(), ModelInfo {
            name: "claude-instant".into(),
            provider: "anthropic".into(),
            cost_per_1k_tokens: 0.0008,
            avg_latency_ms: 300,
            quality_score: 0.70,
            capabilities: vec![],
        });
        
        // Google models
        models.insert("gemini-pro".into(), ModelInfo {
            name: "gemini-pro".into(),
            provider: "google".into(),
            cost_per_1k_tokens: 0.001,
            avg_latency_ms: 1000,
            quality_score: 0.85,
            capabilities: vec!["vision".into()],
        });
        
        // Open models
        models.insert("llama-3-70b".into(), ModelInfo {
            name: "llama-3-70b".into(),
            provider: "together".into(),
            cost_per_1k_tokens: 0.0009,
            avg_latency_ms: 800,
            quality_score: 0.82,
            capabilities: vec![],
        });
        
        models.insert("mixtral-8x7b".into(), ModelInfo {
            name: "mixtral-8x7b".into(),
            provider: "together".into(),
            cost_per_1k_tokens: 0.0007,
            avg_latency_ms: 600,
            quality_score: 0.78,
            capabilities: vec!["experimental".into()],
        });
        
        Self { models }
    }
}

/// Model information
#[derive(Debug, Clone)]
struct ModelInfo {
    name: String,
    provider: String,
    cost_per_1k_tokens: f64,
    avg_latency_ms: u64,
    quality_score: f64,
    capabilities: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{UserPreferences, PerformanceRequirements};
    
    #[tokio::test]
    async fn test_routing_decision() {
        let regime_detector = Arc::new(RegimeDetector::new(4, 0.15).unwrap());
        let hamiltonian = Arc::new(HamiltonianDynamics::new(1.0, 768));
        let router = HLLMRouter::new(regime_detector, hamiltonian);
        
        let adapter = BitDeltaAdapter::new("test_user".into(), 768, 4);
        let efe = ExpectedFreeEnergy::zero();
        
        let request = crate::RoutingRequest {
            input: "Test input".into(),
            context: vec![],
            preferences: UserPreferences {
                max_latency_ms: Some(2000),
                max_cost_per_token: Some(0.01),
                preferred_models: vec![],
                quality_threshold: 0.7,
            },
            requirements: PerformanceRequirements {
                min_tokens_per_second: Some(50.0),
                max_memory_gb: None,
                requires_function_calling: false,
                requires_vision: false,
            },
            observations: vec![0.5; 5],
        };
        
        let decision = router.decide(
            Regime::Exploration,
            efe,
            &adapter,
            &request,
        ).await.unwrap();
        
        assert!(!decision.model.is_empty());
        assert!(decision.confidence > 0.0);
    }
}