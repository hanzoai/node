//! Regime detection using Hidden Markov Models
//! 
//! Identifies market/system regimes to optimize routing decisions

use std::collections::VecDeque;
use std::hash::Hash;
use serde::{Serialize, Deserialize};
use anyhow::{Result, anyhow};
use nalgebra::{DMatrix, DVector};
use rand::Rng;
use rand_distr::{Distribution, Normal};

/// System regime states
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Regime {
    /// High uncertainty, exploration mode
    Exploration,
    /// Low uncertainty, exploitation mode
    Exploitation,
    /// Extreme conditions, crisis management
    Crisis,
    /// Between states, adapting
    Transition,
}

impl Regime {
    /// Get numeric index for matrix operations
    pub fn index(&self) -> usize {
        match self {
            Regime::Exploration => 0,
            Regime::Exploitation => 1,
            Regime::Crisis => 2,
            Regime::Transition => 3,
        }
    }
    
    /// Create from index
    pub fn from_index(idx: usize) -> Result<Self> {
        match idx {
            0 => Ok(Regime::Exploration),
            1 => Ok(Regime::Exploitation),
            2 => Ok(Regime::Crisis),
            3 => Ok(Regime::Transition),
            _ => Err(anyhow!("Invalid regime index: {}", idx)),
        }
    }
    
    /// Get characteristics of the regime
    pub fn characteristics(&self) -> RegimeCharacteristics {
        match self {
            Regime::Exploration => RegimeCharacteristics {
                risk_tolerance: 0.8,
                cost_sensitivity: 0.3,
                quality_focus: 0.6,
                speed_priority: 0.4,
                innovation_bias: 0.9,
            },
            Regime::Exploitation => RegimeCharacteristics {
                risk_tolerance: 0.2,
                cost_sensitivity: 0.7,
                quality_focus: 0.9,
                speed_priority: 0.8,
                innovation_bias: 0.2,
            },
            Regime::Crisis => RegimeCharacteristics {
                risk_tolerance: 0.1,
                cost_sensitivity: 0.5,
                quality_focus: 0.5,
                speed_priority: 0.9,
                innovation_bias: 0.1,
            },
            Regime::Transition => RegimeCharacteristics {
                risk_tolerance: 0.5,
                cost_sensitivity: 0.5,
                quality_focus: 0.7,
                speed_priority: 0.6,
                innovation_bias: 0.5,
            },
        }
    }
}

/// Characteristics that define regime behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegimeCharacteristics {
    pub risk_tolerance: f64,
    pub cost_sensitivity: f64,
    pub quality_focus: f64,
    pub speed_priority: f64,
    pub innovation_bias: f64,
}

/// Hidden Markov Model for regime detection
pub struct RegimeDetector {
    /// Transition probability matrix
    transition_matrix: DMatrix<f64>,
    
    /// Emission probability parameters
    emission_params: Vec<EmissionParams>,
    
    /// Current belief state
    belief_state: DVector<f64>,
    
    /// Observation history
    observation_history: VecDeque<Vec<f64>>,
    
    /// Maximum history length
    max_history: usize,
    
    /// Transition threshold
    transition_threshold: f64,
}

/// Parameters for emission distributions
#[derive(Debug, Clone)]
struct EmissionParams {
    mean: DVector<f64>,
    covariance: DMatrix<f64>,
}

impl RegimeDetector {
    /// Create a new regime detector
    pub fn new(num_regimes: usize, transition_threshold: f64) -> Result<Self> {
        // Initialize transition matrix with regime-specific probabilities
        let transition_matrix = Self::init_transition_matrix(num_regimes);
        
        // Initialize emission parameters for each regime
        let emission_params = Self::init_emission_params(num_regimes);
        
        // Start with uniform belief
        let mut belief_state = DVector::from_element(num_regimes, 1.0 / num_regimes as f64);
        
        // But bias towards Exploration initially
        belief_state[0] = 0.7;
        belief_state[1] = 0.2;
        belief_state[2] = 0.05;
        belief_state[3] = 0.05;
        
        Ok(Self {
            transition_matrix,
            emission_params,
            belief_state,
            observation_history: VecDeque::with_capacity(100),
            max_history: 100,
            transition_threshold,
        })
    }
    
    /// Initialize transition matrix with realistic probabilities
    fn init_transition_matrix(n: usize) -> DMatrix<f64> {
        let mut matrix = DMatrix::zeros(n, n);
        
        // Exploration regime transitions
        matrix[(0, 0)] = 0.7;  // Stay in exploration
        matrix[(0, 1)] = 0.2;  // Move to exploitation
        matrix[(0, 2)] = 0.05; // Move to crisis
        matrix[(0, 3)] = 0.05; // Move to transition
        
        // Exploitation regime transitions
        matrix[(1, 0)] = 0.1;  // Move to exploration
        matrix[(1, 1)] = 0.8;  // Stay in exploitation
        matrix[(1, 2)] = 0.05; // Move to crisis
        matrix[(1, 3)] = 0.05; // Move to transition
        
        // Crisis regime transitions
        matrix[(2, 0)] = 0.05; // Move to exploration
        matrix[(2, 1)] = 0.05; // Move to exploitation
        matrix[(2, 2)] = 0.7;  // Stay in crisis
        matrix[(2, 3)] = 0.2;  // Move to transition
        
        // Transition regime transitions
        matrix[(3, 0)] = 0.25; // Move to exploration
        matrix[(3, 1)] = 0.25; // Move to exploitation
        matrix[(3, 2)] = 0.1;  // Move to crisis
        matrix[(3, 3)] = 0.4;  // Stay in transition
        
        matrix
    }
    
    /// Initialize emission parameters for each regime
    fn init_emission_params(n: usize) -> Vec<EmissionParams> {
        let mut params = Vec::new();
        
        for i in 0..n {
            let regime = Regime::from_index(i).unwrap();
            let characteristics = regime.characteristics();
            
            // Create mean vector based on regime characteristics
            let mean = DVector::from_vec(vec![
                characteristics.risk_tolerance,
                characteristics.cost_sensitivity,
                characteristics.quality_focus,
                characteristics.speed_priority,
                characteristics.innovation_bias,
            ]);
            
            // Create diagonal covariance matrix
            let variance = match regime {
                Regime::Exploration => 0.1,   // High variance
                Regime::Exploitation => 0.02, // Low variance
                Regime::Crisis => 0.05,       // Medium variance
                Regime::Transition => 0.08,   // Medium-high variance
            };
            
            let covariance = DMatrix::from_diagonal(&DVector::from_element(5, variance));
            
            params.push(EmissionParams { mean, covariance });
        }
        
        params
    }
    
    /// Detect current regime from observations
    pub fn detect_regime(&self, observations: &[f64]) -> Result<Regime> {
        if observations.len() < 5 {
            return Err(anyhow!("Need at least 5 observations"));
        }
        
        let obs_vector = DVector::from_vec(observations.to_vec());
        
        // Calculate likelihood for each regime
        let mut likelihoods = Vec::new();
        for (i, params) in self.emission_params.iter().enumerate() {
            let likelihood = self.calculate_likelihood(&obs_vector, params)?;
            likelihoods.push(likelihood * self.belief_state[i]);
        }
        
        // Find most likely regime
        let (max_idx, _) = likelihoods
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
            .unwrap();
        
        Regime::from_index(max_idx)
    }
    
    /// Calculate likelihood of observations given emission parameters
    fn calculate_likelihood(&self, obs: &DVector<f64>, params: &EmissionParams) -> Result<f64> {
        let diff = obs - &params.mean;
        let inv_cov = params.covariance.clone().try_inverse()
            .ok_or_else(|| anyhow!("Covariance matrix not invertible"))?;
        
        let exponent = -0.5 * diff.transpose() * &inv_cov * &diff;
        let det = params.covariance.determinant();
        let norm = (2.0 * std::f64::consts::PI).powi(obs.len() as i32) * det.abs();
        
        Ok((exponent[(0, 0)] / norm.sqrt()).exp())
    }
    
    /// Update belief state with new observations
    pub fn update(&mut self, observations: Vec<f64>) -> Result<()> {
        // Add to history
        self.observation_history.push_back(observations.clone());
        if self.observation_history.len() > self.max_history {
            self.observation_history.pop_front();
        }
        
        let obs_vector = DVector::from_vec(observations);
        
        // Forward algorithm step
        let mut new_belief = DVector::zeros(self.belief_state.len());
        
        for i in 0..self.belief_state.len() {
            // Prediction step
            let mut prediction = 0.0;
            for j in 0..self.belief_state.len() {
                prediction += self.transition_matrix[(i, j)] * self.belief_state[j];
            }
            
            // Update step
            let likelihood = self.calculate_likelihood(&obs_vector, &self.emission_params[i])?;
            new_belief[i] = likelihood * prediction;
        }
        
        // Normalize
        let sum: f64 = new_belief.iter().sum();
        if sum > 0.0 {
            self.belief_state = new_belief / sum;
        }
        
        Ok(())
    }
    
    /// Get current state information
    pub fn get_current_state(&self) -> Result<RegimeState> {
        let current_regime = self.detect_regime(&self.get_average_observations())?;
        
        Ok(RegimeState {
            current_regime,
            belief_distribution: self.belief_state.as_slice().to_vec(),
            transition_matrix: self.transition_matrix.clone().data.as_vec()
                .chunks(self.transition_matrix.ncols())
                .map(|row| row.to_vec())
                .collect(),
            observation_count: self.observation_history.len(),
        })
    }
    
    /// Get average of recent observations
    fn get_average_observations(&self) -> Vec<f64> {
        if self.observation_history.is_empty() {
            return vec![0.5; 5]; // Neutral defaults
        }
        
        let n = self.observation_history.len() as f64;
        let mut avg = vec![0.0; 5];
        
        for obs in &self.observation_history {
            for (i, &val) in obs.iter().enumerate() {
                if i < 5 {
                    avg[i] += val / n;
                }
            }
        }
        
        avg
    }
    
    /// Check if regime transition is likely
    pub fn is_transition_likely(&self) -> bool {
        let max_belief = self.belief_state.iter().max_by(|a, b| a.partial_cmp(b).unwrap()).unwrap();
        max_belief < &(1.0 - self.transition_threshold)
    }
}

/// Markov chain for regime transitions
pub struct MarkovChain {
    states: Vec<Regime>,
    transition_matrix: DMatrix<f64>,
    current_state: usize,
}

impl MarkovChain {
    /// Create a new Markov chain
    pub fn new(states: Vec<Regime>, transition_matrix: DMatrix<f64>) -> Result<Self> {
        if states.len() != transition_matrix.nrows() || states.len() != transition_matrix.ncols() {
            return Err(anyhow!("States and transition matrix dimensions must match"));
        }
        
        Ok(Self {
            states,
            transition_matrix,
            current_state: 0,
        })
    }
    
    /// Sample next state
    pub fn next_state(&mut self) -> Regime {
        let mut rng = rand::thread_rng();
        let row = self.transition_matrix.row(self.current_state);
        
        let r: f64 = rng.gen();
        let mut cumsum = 0.0;
        
        for (i, &prob) in row.iter().enumerate() {
            cumsum += prob;
            if r < cumsum {
                self.current_state = i;
                return self.states[i];
            }
        }
        
        self.states[self.current_state]
    }
    
    /// Get stationary distribution
    pub fn stationary_distribution(&self) -> Result<DVector<f64>> {
        // Solve (P^T - I)π = 0 with constraint sum(π) = 1
        let n = self.transition_matrix.nrows();
        let mut a = self.transition_matrix.transpose() - DMatrix::identity(n, n);
        
        // Replace last row with ones for normalization constraint
        for j in 0..n {
            a[(n-1, j)] = 1.0;
        }
        
        let mut b = DVector::zeros(n);
        b[n-1] = 1.0;
        
        // Solve the system
        let decomp = a.lu();
        decomp.solve(&b).ok_or_else(|| anyhow!("Cannot solve for stationary distribution"))
    }
}

/// Current regime state information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegimeState {
    pub current_regime: Regime,
    pub belief_distribution: Vec<f64>,
    pub transition_matrix: Vec<Vec<f64>>,
    pub observation_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_regime_detection() {
        let detector = RegimeDetector::new(4, 0.15).unwrap();
        
        // Exploration-like observations
        let obs = vec![0.8, 0.3, 0.6, 0.4, 0.9];
        let regime = detector.detect_regime(&obs).unwrap();
        assert_eq!(regime, Regime::Exploration);
    }
    
    #[test]
    fn test_markov_chain() {
        let states = vec![
            Regime::Exploration,
            Regime::Exploitation,
            Regime::Crisis,
            Regime::Transition,
        ];
        
        let matrix = RegimeDetector::init_transition_matrix(4);
        let mut chain = MarkovChain::new(states, matrix).unwrap();
        
        // Sample should return a valid regime
        let next = chain.next_state();
        assert!(matches!(next, Regime::Exploration | Regime::Exploitation | Regime::Crisis | Regime::Transition));
    }
}