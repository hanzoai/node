//! Expected Free Energy calculations for active inference
//! 
//! Implements free energy minimization for decision making

use serde::{Serialize, Deserialize};
use anyhow::{Result, anyhow};
use nalgebra::{DVector, DMatrix};
use std::f64::consts::E;

/// Expected Free Energy for decision making
#[derive(Debug, Clone)]
pub struct ExpectedFreeEnergy {
    /// Epistemic value (information gain)
    epistemic_value: f64,
    
    /// Pragmatic value (goal achievement)
    pragmatic_value: f64,
    
    /// Total expected free energy
    total_efe: f64,
}

impl ExpectedFreeEnergy {
    /// Calculate expected free energy
    pub fn calculate(
        belief_state: BeliefState,
        precision: Precision,
        parameters: &[f64],
    ) -> Result<Self> {
        // Calculate epistemic value (negative entropy)
        let epistemic_value = Self::calculate_epistemic_value(&belief_state)?;
        
        // Calculate pragmatic value (expected utility)
        let pragmatic_value = Self::calculate_pragmatic_value(
            &belief_state,
            &precision,
            parameters,
        )?;
        
        // Total EFE = epistemic + pragmatic (weighted by precision)
        let total_efe = epistemic_value + precision.value() * pragmatic_value;
        
        Ok(Self {
            epistemic_value,
            pragmatic_value,
            total_efe,
        })
    }
    
    /// Calculate epistemic value (information gain)
    fn calculate_epistemic_value(belief: &BeliefState) -> Result<f64> {
        // Shannon entropy: -Σ p(x) log p(x)
        let entropy = belief.distribution
            .iter()
            .filter(|&&p| p > 0.0)
            .map(|&p| -p * p.ln())
            .sum::<f64>();
        
        // Epistemic value is negative entropy (we want low uncertainty)
        Ok(-entropy)
    }
    
    /// Calculate pragmatic value (expected utility)
    fn calculate_pragmatic_value(
        belief: &BeliefState,
        precision: &Precision,
        parameters: &[f64],
    ) -> Result<f64> {
        // Calculate expected utility under current belief
        let mut expected_utility = 0.0;
        
        for (i, &prob) in belief.distribution.iter().enumerate() {
            if i < belief.utilities.len() {
                expected_utility += prob * belief.utilities[i];
            }
        }
        
        // Apply precision weighting and parameter modulation
        let param_factor = if !parameters.is_empty() {
            parameters.iter().sum::<f64>() / parameters.len() as f64
        } else {
            1.0
        };
        
        Ok(expected_utility * precision.inverse_temperature() * param_factor)
    }
    
    /// Get the total EFE value
    pub fn value(&self) -> f64 {
        self.total_efe
    }
    
    /// Get epistemic component
    pub fn epistemic(&self) -> f64 {
        self.epistemic_value
    }
    
    /// Get pragmatic component
    pub fn pragmatic(&self) -> f64 {
        self.pragmatic_value
    }
    
    /// Create zero EFE for testing
    pub fn zero() -> Self {
        Self {
            epistemic_value: 0.0,
            pragmatic_value: 0.0,
            total_efe: 0.0,
        }
    }
    
    /// Check if EFE indicates exploration should be preferred
    pub fn should_explore(&self) -> bool {
        // High epistemic value relative to pragmatic suggests exploration
        self.epistemic_value.abs() > self.pragmatic_value.abs() * 1.5
    }
    
    /// Check if EFE indicates exploitation should be preferred
    pub fn should_exploit(&self) -> bool {
        // High pragmatic value with low epistemic suggests exploitation
        self.pragmatic_value > 0.5 && self.epistemic_value.abs() < 0.1
    }
}

/// Belief state representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeliefState {
    /// Probability distribution over states
    pub distribution: Vec<f64>,
    
    /// Utilities for each state
    pub utilities: Vec<f64>,
    
    /// Confidence in beliefs
    pub confidence: f64,
    
    /// Observation count
    pub observation_count: usize,
}

impl BeliefState {
    /// Create from request observations
    pub fn from_request(request: &crate::RoutingRequest) -> Result<Self> {
        if request.observations.is_empty() {
            return Err(anyhow!("No observations in request"));
        }
        
        // Normalize observations to probabilities
        let sum: f64 = request.observations.iter().map(|x| x.abs()).sum();
        let distribution = if sum > 0.0 {
            request.observations.iter()
                .map(|&x| x.abs() / sum)
                .collect()
        } else {
            vec![1.0 / request.observations.len() as f64; request.observations.len()]
        };
        
        // Calculate utilities based on preferences
        let utilities = Self::calculate_utilities(request);
        
        // Confidence based on context history
        let confidence = 1.0 / (1.0 + (-(request.context.len() as f64) / 10.0).exp());
        
        Ok(Self {
            distribution,
            utilities,
            confidence,
            observation_count: request.observations.len(),
        })
    }
    
    /// Calculate utility values from request
    fn calculate_utilities(request: &crate::RoutingRequest) -> Vec<f64> {
        let mut utilities = Vec::new();
        
        // Quality utility
        utilities.push(request.preferences.quality_threshold);
        
        // Cost utility (inverse)
        let cost_util = if let Some(max_cost) = request.preferences.max_cost_per_token {
            1.0 / (1.0 + max_cost)
        } else {
            0.5
        };
        utilities.push(cost_util);
        
        // Latency utility (inverse)
        let latency_util = if let Some(max_latency) = request.preferences.max_latency_ms {
            10000.0 / (100.0 + max_latency as f64)
        } else {
            0.5
        };
        utilities.push(latency_util);
        
        // Capability utility
        let capability_util = if request.requirements.requires_function_calling 
            || request.requirements.requires_vision {
            0.8
        } else {
            0.3
        };
        utilities.push(capability_util);
        
        // Performance utility
        let perf_util = if let Some(min_tps) = request.requirements.min_tokens_per_second {
            min_tps / 100.0 // Normalize to 0-1 range
        } else {
            0.5
        };
        utilities.push(perf_util);
        
        utilities
    }
    
    /// Update belief with new observations
    pub fn update(&mut self, observations: &[f64]) -> Result<()> {
        if observations.len() != self.distribution.len() {
            return Err(anyhow!("Observation dimension mismatch"));
        }
        
        // Bayesian update
        for (i, &obs) in observations.iter().enumerate() {
            self.distribution[i] *= obs;
        }
        
        // Renormalize
        let sum: f64 = self.distribution.iter().sum();
        if sum > 0.0 {
            for p in &mut self.distribution {
                *p /= sum;
            }
        }
        
        self.observation_count += 1;
        self.confidence = (self.confidence + 0.1).min(1.0);
        
        Ok(())
    }
    
    /// Calculate KL divergence from another belief
    pub fn kl_divergence(&self, other: &BeliefState) -> Result<f64> {
        if self.distribution.len() != other.distribution.len() {
            return Err(anyhow!("Distribution dimension mismatch"));
        }
        
        let mut kl = 0.0;
        for (i, &p) in self.distribution.iter().enumerate() {
            if p > 0.0 && other.distribution[i] > 0.0 {
                kl += p * (p / other.distribution[i]).ln();
            }
        }
        
        Ok(kl)
    }
}

/// Precision (inverse temperature) for active inference
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Precision {
    /// Base precision value
    base_value: f64,
    
    /// Adaptive component
    adaptive_component: f64,
    
    /// Decay rate
    decay_rate: f64,
}

impl Precision {
    /// Create from regime
    pub fn from_regime(regime: &crate::Regime) -> Self {
        let (base, adaptive, decay) = match regime {
            crate::Regime::Exploration => (0.5, 0.2, 0.01),
            crate::Regime::Exploitation => (2.0, 0.1, 0.005),
            crate::Regime::Crisis => (5.0, 0.5, 0.02),
            crate::Regime::Transition => (1.0, 0.3, 0.01),
        };
        
        Self {
            base_value: base,
            adaptive_component: adaptive,
            decay_rate: decay,
        }
    }
    
    /// Get current precision value
    pub fn value(&self) -> f64 {
        self.base_value + self.adaptive_component
    }
    
    /// Get inverse temperature (β in physics)
    pub fn inverse_temperature(&self) -> f64 {
        1.0 / self.value()
    }
    
    /// Update precision based on performance
    pub fn update(&mut self, performance_error: f64) {
        // Increase precision if error is high
        self.adaptive_component += performance_error * 0.1;
        self.adaptive_component *= 1.0 - self.decay_rate;
        self.adaptive_component = self.adaptive_component.clamp(-1.0, 2.0);
    }
    
    /// Calculate action probability using softmax with precision
    pub fn action_probability(&self, values: &[f64]) -> Vec<f64> {
        let beta = self.value();
        let max_val = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        
        let exp_values: Vec<f64> = values.iter()
            .map(|&v| ((v - max_val) * beta).exp())
            .collect();
        
        let sum: f64 = exp_values.iter().sum();
        
        exp_values.iter().map(|&v| v / sum).collect()
    }
}

/// Active inference agent for decision making
pub struct ActiveInferenceAgent {
    /// Current belief state
    belief: BeliefState,
    
    /// Precision controller
    precision: Precision,
    
    /// Generative model
    generative_model: GenerativeModel,
}

impl ActiveInferenceAgent {
    /// Create new agent
    pub fn new(initial_belief: BeliefState, precision: Precision) -> Self {
        Self {
            belief: initial_belief,
            precision,
            generative_model: GenerativeModel::default(),
        }
    }
    
    /// Select action to minimize expected free energy
    pub fn select_action(&self, available_actions: &[Action]) -> Result<Action> {
        let mut min_efe = f64::INFINITY;
        let mut best_action = None;
        
        for action in available_actions {
            // Predict future belief given action
            let predicted_belief = self.generative_model.predict(&self.belief, action)?;
            
            // Calculate expected free energy
            let efe = ExpectedFreeEnergy::calculate(
                predicted_belief,
                self.precision.clone(),
                &[],
            )?;
            
            if efe.value() < min_efe {
                min_efe = efe.value();
                best_action = Some(action.clone());
            }
        }
        
        best_action.ok_or_else(|| anyhow!("No action selected"))
    }
    
    /// Update agent with observation
    pub fn observe(&mut self, observation: Observation) -> Result<()> {
        // Update belief
        self.belief.update(&observation.values)?;
        
        // Update precision based on prediction error
        let prediction_error = self.generative_model.prediction_error(&observation)?;
        self.precision.update(prediction_error);
        
        // Update generative model
        self.generative_model.learn(&self.belief, &observation)?;
        
        Ok(())
    }
}

/// Generative model for predictions
#[derive(Debug, Clone)]
struct GenerativeModel {
    /// State transition model
    transition_model: DMatrix<f64>,
    
    /// Observation model
    observation_model: DMatrix<f64>,
}

impl GenerativeModel {
    /// Predict future belief given action
    fn predict(&self, belief: &BeliefState, action: &Action) -> Result<BeliefState> {
        // Simple prediction: apply transition model
        let current = DVector::from_vec(belief.distribution.clone());
        let predicted = &self.transition_model * current;
        
        let mut new_belief = belief.clone();
        new_belief.distribution = predicted.as_slice().to_vec();
        
        // Apply action effect
        if let Some(effect_idx) = action.effect_index {
            if effect_idx < new_belief.distribution.len() {
                new_belief.distribution[effect_idx] *= action.effect_magnitude;
                
                // Renormalize
                let sum: f64 = new_belief.distribution.iter().sum();
                if sum > 0.0 {
                    for p in &mut new_belief.distribution {
                        *p /= sum;
                    }
                }
            }
        }
        
        Ok(new_belief)
    }
    
    /// Calculate prediction error
    fn prediction_error(&self, observation: &Observation) -> Result<f64> {
        // Simple MSE for now
        let error = observation.values.iter()
            .map(|&v| (v - observation.expected).powi(2))
            .sum::<f64>() / observation.values.len() as f64;
        
        Ok(error.sqrt())
    }
    
    /// Learn from observation
    fn learn(&mut self, belief: &BeliefState, observation: &Observation) -> Result<()> {
        // Simple Hebbian-like update
        let learning_rate = 0.01;
        
        for i in 0..self.transition_model.nrows() {
            for j in 0..self.transition_model.ncols() {
                if i < belief.distribution.len() && j < observation.values.len() {
                    self.transition_model[(i, j)] += 
                        learning_rate * belief.distribution[i] * observation.values[j];
                }
            }
        }
        
        // Keep matrix normalized
        for i in 0..self.transition_model.nrows() {
            let row_sum: f64 = self.transition_model.row(i).iter().sum();
            if row_sum > 0.0 {
                for j in 0..self.transition_model.ncols() {
                    self.transition_model[(i, j)] /= row_sum;
                }
            }
        }
        
        Ok(())
    }
}

impl Default for GenerativeModel {
    fn default() -> Self {
        let n = 5;
        
        // Initialize with slightly biased identity matrix
        let mut transition = DMatrix::identity(n, n) * 0.8;
        for i in 0..n {
            for j in 0..n {
                if i != j {
                    transition[(i, j)] = 0.2 / (n - 1) as f64;
                }
            }
        }
        
        Self {
            transition_model: transition.clone(),
            observation_model: transition,
        }
    }
}

/// Action representation
#[derive(Debug, Clone)]
pub struct Action {
    pub name: String,
    pub effect_index: Option<usize>,
    pub effect_magnitude: f64,
}

/// Observation representation
#[derive(Debug, Clone)]
pub struct Observation {
    pub values: Vec<f64>,
    pub expected: f64,
    pub timestamp: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_expected_free_energy() {
        let belief = BeliefState {
            distribution: vec![0.3, 0.5, 0.2],
            utilities: vec![0.8, 0.6, 0.4],
            confidence: 0.7,
            observation_count: 10,
        };
        
        let precision = Precision::from_regime(&crate::Regime::Exploration);
        
        let efe = ExpectedFreeEnergy::calculate(
            belief,
            precision,
            &[0.5, 0.3],
        ).unwrap();
        
        assert!(efe.value().is_finite());
        assert!(efe.epistemic().is_finite());
        assert!(efe.pragmatic().is_finite());
    }
    
    #[test]
    fn test_belief_update() {
        let mut belief = BeliefState {
            distribution: vec![0.25, 0.25, 0.25, 0.25],
            utilities: vec![1.0, 0.5, 0.3, 0.1],
            confidence: 0.5,
            observation_count: 1,
        };
        
        belief.update(&[0.1, 0.5, 0.3, 0.1]).unwrap();
        
        // Distribution should be renormalized
        let sum: f64 = belief.distribution.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6);
        
        // Confidence should increase
        assert!(belief.confidence > 0.5);
    }
}