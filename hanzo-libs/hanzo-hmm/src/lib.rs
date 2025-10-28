//! # Hanzo HMM - Hidden Markov Model Implementation
//!
//! This is a pure Hidden Markov Model (HMM) implementation, separate from HLLM.
//! 
//! ## Difference between HMM and HLLM:
//! 
//! - **HMM (This module)**: Classic Hidden Markov Model for state detection and prediction
//!   - Discrete states and observations
//!   - Transition and emission probabilities
//!   - Viterbi algorithm for state sequence detection
//!   - Forward-backward algorithm for probability calculations
//!   - Baum-Welch for parameter learning
//!
//! - **HLLM (hanzo-llm)**: Hamiltonian Hidden-Markov LLM - A hybrid system that combines:
//!   - HMM for regime detection (using this module internally)
//!   - Hamiltonian mechanics for price dynamics
//!   - LLM routing based on detected regimes
//!   - BitDelta quantization for model adaptation
//!
//! This module provides the foundational HMM that HLLM builds upon.

use anyhow::{Result, Context};
use nalgebra::{DMatrix, DVector};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use log::{debug, trace};

/// A Hidden Markov Model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HiddenMarkovModel<S, O> 
where
    S: Clone + Eq + std::hash::Hash,
    O: Clone + Eq + std::hash::Hash,
{
    /// Set of states
    pub states: Vec<S>,
    /// Set of observations
    pub observations: Vec<O>,
    /// Initial state probabilities π
    pub initial_probs: DVector<f64>,
    /// State transition matrix A[i,j] = P(state_j | state_i)
    pub transition_matrix: DMatrix<f64>,
    /// Emission matrix B[i,j] = P(observation_j | state_i)
    pub emission_matrix: DMatrix<f64>,
    /// State to index mapping
    state_to_idx: HashMap<S, usize>,
    /// Observation to index mapping
    obs_to_idx: HashMap<O, usize>,
}

impl<S, O> HiddenMarkovModel<S, O>
where
    S: Clone + Eq + std::hash::Hash + std::fmt::Debug,
    O: Clone + Eq + std::hash::Hash + std::fmt::Debug,
{
    /// Create a new HMM
    pub fn new(
        states: Vec<S>,
        observations: Vec<O>,
        initial_probs: Vec<f64>,
        transition_probs: Vec<Vec<f64>>,
        emission_probs: Vec<Vec<f64>>,
    ) -> Result<Self> {
        let n_states = states.len();
        let n_obs = observations.len();
        
        // Validate dimensions
        if initial_probs.len() != n_states {
            anyhow::bail!("Initial probabilities must have {} elements", n_states);
        }
        if transition_probs.len() != n_states {
            anyhow::bail!("Transition matrix must have {} rows", n_states);
        }
        if emission_probs.len() != n_states {
            anyhow::bail!("Emission matrix must have {} rows", n_states);
        }
        
        // Create index mappings
        let mut state_to_idx = HashMap::new();
        for (idx, state) in states.iter().enumerate() {
            state_to_idx.insert(state.clone(), idx);
        }
        
        let mut obs_to_idx = HashMap::new();
        for (idx, obs) in observations.iter().enumerate() {
            obs_to_idx.insert(obs.clone(), idx);
        }
        
        // Convert to matrices
        let initial_probs = DVector::from_vec(initial_probs);
        let transition_matrix = DMatrix::from_row_slice(n_states, n_states, 
            &transition_probs.concat());
        let emission_matrix = DMatrix::from_row_slice(n_states, n_obs,
            &emission_probs.concat());
        
        Ok(Self {
            states,
            observations,
            initial_probs,
            transition_matrix,
            emission_matrix,
            state_to_idx,
            obs_to_idx,
        })
    }
    
    /// Viterbi algorithm - find most likely state sequence given observations
    pub fn viterbi(&self, observations: &[O]) -> Result<Vec<S>> {
        let n_states = self.states.len();
        let n_obs = observations.len();
        
        if n_obs == 0 {
            return Ok(Vec::new());
        }
        
        // Convert observations to indices
        let obs_indices: Vec<usize> = observations
            .iter()
            .map(|o| self.obs_to_idx.get(o).copied()
                .context(format!("Unknown observation: {:?}", o)))
            .collect::<Result<Vec<_>>>()?;
        
        // Initialize Viterbi tables
        let mut viterbi = DMatrix::zeros(n_states, n_obs);
        let mut path = vec![vec![0usize; n_obs]; n_states];
        
        // Initialization step
        let first_obs = obs_indices[0];
        for s in 0..n_states {
            viterbi[(s, 0)] = self.initial_probs[s] * self.emission_matrix[(s, first_obs)];
        }
        
        // Recursion step
        for t in 1..n_obs {
            let obs_idx = obs_indices[t];
            for s in 0..n_states {
                let mut max_prob = 0.0;
                let mut max_state = 0;
                
                for prev_s in 0..n_states {
                    let prob = viterbi[(prev_s, t-1)] * 
                               self.transition_matrix[(prev_s, s)] * 
                               self.emission_matrix[(s, obs_idx)];
                    if prob > max_prob {
                        max_prob = prob;
                        max_state = prev_s;
                    }
                }
                
                viterbi[(s, t)] = max_prob;
                path[s][t] = max_state;
            }
        }
        
        // Termination step - find best final state
        let mut best_last_state = 0;
        let mut max_prob = viterbi[(0, n_obs-1)];
        for s in 1..n_states {
            if viterbi[(s, n_obs-1)] > max_prob {
                max_prob = viterbi[(s, n_obs-1)];
                best_last_state = s;
            }
        }
        
        // Backtrack to find path
        let mut state_sequence = vec![best_last_state];
        for t in (1..n_obs).rev() {
            let prev_state = path[state_sequence[0]][t];
            state_sequence.insert(0, prev_state);
        }
        
        // Convert indices back to states
        Ok(state_sequence
            .iter()
            .map(|&idx| self.states[idx].clone())
            .collect())
    }
    
    /// Forward algorithm - compute probability of observation sequence
    pub fn forward(&self, observations: &[O]) -> Result<f64> {
        let n_states = self.states.len();
        let n_obs = observations.len();
        
        if n_obs == 0 {
            return Ok(1.0);
        }
        
        // Convert observations to indices
        let obs_indices: Vec<usize> = observations
            .iter()
            .map(|o| self.obs_to_idx.get(o).copied()
                .context(format!("Unknown observation: {:?}", o)))
            .collect::<Result<Vec<_>>>()?;
        
        // Initialize forward table
        let mut alpha = DMatrix::zeros(n_states, n_obs);
        
        // Initialization
        let first_obs = obs_indices[0];
        for s in 0..n_states {
            alpha[(s, 0)] = self.initial_probs[s] * self.emission_matrix[(s, first_obs)];
        }
        
        // Induction
        for t in 1..n_obs {
            let obs_idx = obs_indices[t];
            for s in 0..n_states {
                let mut sum = 0.0;
                for prev_s in 0..n_states {
                    sum += alpha[(prev_s, t-1)] * self.transition_matrix[(prev_s, s)];
                }
                alpha[(s, t)] = sum * self.emission_matrix[(s, obs_idx)];
            }
        }
        
        // Termination
        let mut prob = 0.0;
        for s in 0..n_states {
            prob += alpha[(s, n_obs-1)];
        }
        
        Ok(prob)
    }
    
    /// Backward algorithm
    pub fn backward(&self, observations: &[O]) -> Result<DMatrix<f64>> {
        let n_states = self.states.len();
        let n_obs = observations.len();
        
        if n_obs == 0 {
            return Ok(DMatrix::zeros(n_states, 0));
        }
        
        // Convert observations to indices
        let obs_indices: Vec<usize> = observations
            .iter()
            .map(|o| self.obs_to_idx.get(o).copied()
                .context(format!("Unknown observation: {:?}", o)))
            .collect::<Result<Vec<_>>>()?;
        
        // Initialize backward table
        let mut beta = DMatrix::zeros(n_states, n_obs);
        
        // Initialization
        for s in 0..n_states {
            beta[(s, n_obs-1)] = 1.0;
        }
        
        // Induction
        for t in (0..n_obs-1).rev() {
            let next_obs = obs_indices[t+1];
            for s in 0..n_states {
                let mut sum = 0.0;
                for next_s in 0..n_states {
                    sum += self.transition_matrix[(s, next_s)] * 
                           self.emission_matrix[(next_s, next_obs)] * 
                           beta[(next_s, t+1)];
                }
                beta[(s, t)] = sum;
            }
        }
        
        Ok(beta)
    }
    
    /// Baum-Welch algorithm for parameter learning (EM algorithm)
    pub fn baum_welch(&mut self, observations: &[Vec<O>], max_iterations: usize, tolerance: f64) -> Result<()> {
        let mut prev_likelihood = f64::NEG_INFINITY;
        
        for iteration in 0..max_iterations {
            let mut total_likelihood = 0.0;
            
            // Accumulate statistics
            let mut gamma_sum = DVector::zeros(self.states.len());
            let mut xi_sum = DMatrix::zeros(self.states.len(), self.states.len());
            let mut emission_sum = DMatrix::zeros(self.states.len(), self.observations.len());
            let mut initial_sum = DVector::zeros(self.states.len());
            
            for obs_seq in observations {
                if obs_seq.is_empty() {
                    continue;
                }
                
                // E-step: compute forward and backward probabilities
                let alpha = self.forward_matrix(obs_seq)?;
                let beta = self.backward(obs_seq)?;
                let likelihood = self.forward(obs_seq)?;
                total_likelihood += likelihood.ln();
                
                // Compute gamma and xi
                for t in 0..obs_seq.len() {
                    for i in 0..self.states.len() {
                        let gamma = alpha[(i, t)] * beta[(i, t)] / likelihood;
                        gamma_sum[i] += gamma;
                        
                        if t == 0 {
                            initial_sum[i] += gamma;
                        }
                        
                        let obs_idx = self.obs_to_idx[&obs_seq[t]];
                        emission_sum[(i, obs_idx)] += gamma;
                        
                        if t < obs_seq.len() - 1 {
                            for j in 0..self.states.len() {
                                let next_obs_idx = self.obs_to_idx[&obs_seq[t+1]];
                                let xi = alpha[(i, t)] * 
                                        self.transition_matrix[(i, j)] * 
                                        self.emission_matrix[(j, next_obs_idx)] * 
                                        beta[(j, t+1)] / likelihood;
                                xi_sum[(i, j)] += xi;
                            }
                        }
                    }
                }
            }
            
            // M-step: update parameters
            let n_sequences = observations.len() as f64;
            
            // Update initial probabilities
            for i in 0..self.states.len() {
                self.initial_probs[i] = initial_sum[i] / n_sequences;
            }
            
            // Update transition matrix
            for i in 0..self.states.len() {
                let row_sum = xi_sum.row(i).sum();
                if row_sum > 0.0 {
                    for j in 0..self.states.len() {
                        self.transition_matrix[(i, j)] = xi_sum[(i, j)] / row_sum;
                    }
                }
            }
            
            // Update emission matrix
            for i in 0..self.states.len() {
                let state_sum = gamma_sum[i];
                if state_sum > 0.0 {
                    for j in 0..self.observations.len() {
                        self.emission_matrix[(i, j)] = emission_sum[(i, j)] / state_sum;
                    }
                }
            }
            
            // Check convergence
            if (total_likelihood - prev_likelihood).abs() < tolerance {
                debug!("Baum-Welch converged after {} iterations", iteration + 1);
                break;
            }
            prev_likelihood = total_likelihood;
        }
        
        Ok(())
    }
    
    /// Helper function to get forward matrix
    fn forward_matrix(&self, observations: &[O]) -> Result<DMatrix<f64>> {
        let n_states = self.states.len();
        let n_obs = observations.len();
        
        let obs_indices: Vec<usize> = observations
            .iter()
            .map(|o| self.obs_to_idx.get(o).copied()
                .context(format!("Unknown observation: {:?}", o)))
            .collect::<Result<Vec<_>>>()?;
        
        let mut alpha = DMatrix::zeros(n_states, n_obs);
        
        // Initialization
        let first_obs = obs_indices[0];
        for s in 0..n_states {
            alpha[(s, 0)] = self.initial_probs[s] * self.emission_matrix[(s, first_obs)];
        }
        
        // Induction
        for t in 1..n_obs {
            let obs_idx = obs_indices[t];
            for s in 0..n_states {
                let mut sum = 0.0;
                for prev_s in 0..n_states {
                    sum += alpha[(prev_s, t-1)] * self.transition_matrix[(prev_s, s)];
                }
                alpha[(s, t)] = sum * self.emission_matrix[(s, obs_idx)];
            }
        }
        
        Ok(alpha)
    }
    
    /// Generate a sequence of observations
    pub fn generate(&self, length: usize) -> Vec<(S, O)> {
        let mut rng = rand::thread_rng();
        let mut sequence = Vec::with_capacity(length);
        
        // Sample initial state
        let mut current_state = self.sample_from_distribution(&self.initial_probs, &mut rng);
        
        for _ in 0..length {
            // Sample observation from current state
            let emission_probs = self.emission_matrix.row(current_state);
            let obs_idx = self.sample_from_distribution(&emission_probs.transpose(), &mut rng);
            
            sequence.push((
                self.states[current_state].clone(),
                self.observations[obs_idx].clone()
            ));
            
            // Sample next state
            let transition_probs = self.transition_matrix.row(current_state);
            current_state = self.sample_from_distribution(&transition_probs.transpose(), &mut rng);
        }
        
        sequence
    }
    
    /// Sample from a probability distribution
    fn sample_from_distribution(&self, probs: &DVector<f64>, rng: &mut impl Rng) -> usize {
        let r: f64 = rng.gen();
        let mut cumsum = 0.0;
        
        for (idx, &p) in probs.iter().enumerate() {
            cumsum += p;
            if r <= cumsum {
                return idx;
            }
        }
        
        probs.len() - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_hmm_creation() {
        let states = vec!["Fair", "Loaded"];
        let observations = vec![1, 2, 3, 4, 5, 6];
        let initial = vec![0.5, 0.5];
        let transitions = vec![
            vec![0.7, 0.3],  // Fair -> Fair, Fair -> Loaded
            vec![0.4, 0.6],  // Loaded -> Fair, Loaded -> Loaded
        ];
        let emissions = vec![
            vec![1.0/6.0; 6],  // Fair die - uniform
            vec![0.1, 0.1, 0.1, 0.1, 0.1, 0.5],  // Loaded die - biased toward 6
        ];
        
        let hmm = HiddenMarkovModel::new(
            states, observations, initial, transitions, emissions
        ).unwrap();
        
        assert_eq!(hmm.states.len(), 2);
        assert_eq!(hmm.observations.len(), 6);
    }
    
    #[test]
    fn test_viterbi() {
        let states = vec!["Fair", "Loaded"];
        let observations = vec![1, 2, 3, 4, 5, 6];
        let initial = vec![0.5, 0.5];
        let transitions = vec![
            vec![0.7, 0.3],
            vec![0.4, 0.6],
        ];
        let emissions = vec![
            vec![1.0/6.0; 6],
            vec![0.1, 0.1, 0.1, 0.1, 0.1, 0.5],
        ];
        
        let hmm = HiddenMarkovModel::new(
            states, observations, initial, transitions, emissions
        ).unwrap();
        
        let obs_sequence = vec![6, 6, 6, 1, 1];
        let states = hmm.viterbi(&obs_sequence).unwrap();
        
        // With three 6s in a row, should detect "Loaded" state
        assert_eq!(states.len(), 5);
    }
}