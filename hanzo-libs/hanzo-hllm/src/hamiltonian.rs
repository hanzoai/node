//! Hamiltonian mechanics for price dynamics
//! 
//! Models price evolution using classical mechanics principles

use std::sync::Arc;
use serde::{Serialize, Deserialize};
use anyhow::{Result, anyhow};
use nalgebra::{DVector, DMatrix};
use ode_solvers::{Rk4, System, Vector3, Dop853};
use num_traits::Float;

/// Hamiltonian dynamics system
pub struct HamiltonianDynamics {
    /// Energy scale factor
    energy_scale: f64,
    
    /// Dimension of phase space
    dimension: usize,
    
    /// Current phase space state
    phase_space: PhaseSpace,
    
    /// Potential energy function
    potential: Box<dyn PotentialFunction>,
    
    /// Friction coefficient for dissipation
    friction: f64,
}

impl HamiltonianDynamics {
    /// Create new Hamiltonian system
    pub fn new(energy_scale: f64, dimension: usize) -> Self {
        let phase_space = PhaseSpace::new(dimension);
        let potential = Box::new(QuadraticPotential::new(energy_scale));
        
        Self {
            energy_scale,
            dimension,
            phase_space,
            potential,
            friction: 0.1, // Small friction for realistic dynamics
        }
    }
    
    /// Evolve the system forward in time
    pub fn evolve(&mut self, dt: f64) -> Result<()> {
        // Use Hamiltonian equations of motion
        // dq/dt = ∂H/∂p = p/m
        // dp/dt = -∂H/∂q - friction*p
        
        let m = 1.0; // Unit mass for simplicity
        
        // Update positions
        let new_positions = &self.phase_space.positions + &self.phase_space.momenta * (dt / m);
        
        // Calculate forces from potential
        let forces = self.potential.gradient(&self.phase_space.positions)?;
        
        // Update momenta with friction
        let new_momenta = &self.phase_space.momenta 
            - forces * dt 
            - &self.phase_space.momenta * (self.friction * dt);
        
        self.phase_space.positions = new_positions;
        self.phase_space.momenta = new_momenta;
        self.phase_space.time += dt;
        
        Ok(())
    }
    
    /// Calculate total energy (Hamiltonian)
    pub fn total_energy(&self) -> Result<f64> {
        let kinetic = self.kinetic_energy();
        let potential = self.potential.value(&self.phase_space.positions)?;
        Ok(kinetic + potential)
    }
    
    /// Calculate kinetic energy T = p²/2m
    fn kinetic_energy(&self) -> f64 {
        let m = 1.0;
        self.phase_space.momenta.dot(&self.phase_space.momenta) / (2.0 * m)
    }
    
    /// Calculate price from phase space
    pub fn calculate_price(&self, base_price: f64) -> Result<f64> {
        let energy = self.total_energy()?;
        
        // Price modulation based on energy
        // Higher energy = higher volatility = higher price uncertainty
        let volatility_factor = (energy / self.energy_scale).tanh();
        
        // Phase space position influences base price
        let position_factor = self.phase_space.positions.norm();
        
        // Momentum indicates trend
        let momentum_factor = self.phase_space.momenta.mean();
        
        // Combine factors for final price
        let price = base_price * (1.0 + 0.1 * position_factor) 
            * (1.0 + 0.05 * momentum_factor)
            * (1.0 + 0.2 * volatility_factor);
        
        Ok(price.max(0.0)) // Ensure non-negative
    }
    
    /// Get current phase space
    pub fn get_phase_space(&self) -> Result<PhaseSpace> {
        Ok(self.phase_space.clone())
    }
    
    /// Set custom potential function
    pub fn set_potential(&mut self, potential: Box<dyn PotentialFunction>) {
        self.potential = potential;
    }
    
    /// Inject energy into the system (perturbation)
    pub fn perturb(&mut self, energy: f64) -> Result<()> {
        // Add random momentum kick
        let mut rng = rand::thread_rng();
        use rand::Rng;
        
        for i in 0..self.dimension {
            let kick = rng.gen_range(-1.0..1.0) * energy.sqrt();
            self.phase_space.momenta[i] += kick;
        }
        
        Ok(())
    }
    
    /// Calculate Lyapunov exponent (chaos measure)
    pub fn lyapunov_exponent(&self, steps: usize, dt: f64) -> Result<f64> {
        let epsilon = 1e-8;
        let mut system1 = self.phase_space.clone();
        let mut system2 = self.phase_space.clone();
        
        // Perturb second system slightly
        system2.positions[0] += epsilon;
        
        let mut sum_log_divergence = 0.0;
        
        for _ in 0..steps {
            // Evolve both systems
            system1.evolve_step(&*self.potential, dt, self.friction)?;
            system2.evolve_step(&*self.potential, dt, self.friction)?;
            
            // Calculate divergence
            let distance = (&system2.positions - &system1.positions).norm();
            sum_log_divergence += (distance / epsilon).ln();
            
            // Renormalize
            let scale = epsilon / distance;
            system2.positions = &system1.positions + (&system2.positions - &system1.positions) * scale;
        }
        
        Ok(sum_log_divergence / (steps as f64 * dt))
    }
}

/// Phase space representation
#[derive(Debug, Clone)]
pub struct PhaseSpace {
    /// Generalized coordinates (positions)
    pub positions: DVector<f64>,
    
    /// Generalized momenta
    pub momenta: DVector<f64>,
    
    /// Current time
    pub time: f64,
}

impl PhaseSpace {
    /// Create new phase space
    pub fn new(dimension: usize) -> Self {
        Self {
            positions: DVector::zeros(dimension),
            momenta: DVector::zeros(dimension),
            time: 0.0,
        }
    }
    
    /// Initialize with random state
    pub fn random(dimension: usize, scale: f64) -> Self {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        
        let positions = DVector::from_fn(dimension, |_, _| rng.gen_range(-scale..scale));
        let momenta = DVector::from_fn(dimension, |_, _| rng.gen_range(-scale..scale));
        
        Self {
            positions,
            momenta,
            time: 0.0,
        }
    }
    
    /// Calculate total energy
    pub fn total_energy(&self) -> f64 {
        // Assuming unit mass and quadratic potential
        let kinetic = self.momenta.dot(&self.momenta) / 2.0;
        let potential = self.positions.dot(&self.positions) / 2.0;
        kinetic + potential
    }
    
    /// Evolve one time step
    fn evolve_step(
        &mut self,
        potential: &dyn PotentialFunction,
        dt: f64,
        friction: f64,
    ) -> Result<()> {
        // Leapfrog integration for better energy conservation
        let forces = potential.gradient(&self.positions)?;
        
        // Half step momentum update
        self.momenta -= forces * (dt / 2.0);
        self.momenta *= 1.0 - friction * dt / 2.0;
        
        // Full step position update
        self.positions += &self.momenta * dt;
        
        // Half step momentum update
        let forces = potential.gradient(&self.positions)?;
        self.momenta -= forces * (dt / 2.0);
        self.momenta *= 1.0 - friction * dt / 2.0;
        
        self.time += dt;
        
        Ok(())
    }
}

/// Trait for potential energy functions
pub trait PotentialFunction: Send + Sync {
    /// Calculate potential energy value
    fn value(&self, positions: &DVector<f64>) -> Result<f64>;
    
    /// Calculate gradient (forces)
    fn gradient(&self, positions: &DVector<f64>) -> Result<DVector<f64>>;
}

/// Simple quadratic potential V = k*q²/2
pub struct QuadraticPotential {
    spring_constant: f64,
}

impl QuadraticPotential {
    pub fn new(k: f64) -> Self {
        Self { spring_constant: k }
    }
}

impl PotentialFunction for QuadraticPotential {
    fn value(&self, positions: &DVector<f64>) -> Result<f64> {
        Ok(self.spring_constant * positions.dot(positions) / 2.0)
    }
    
    fn gradient(&self, positions: &DVector<f64>) -> Result<DVector<f64>> {
        Ok(positions * self.spring_constant)
    }
}

/// Anharmonic potential with quartic term
pub struct AnharmonicPotential {
    quadratic_coeff: f64,
    quartic_coeff: f64,
}

impl AnharmonicPotential {
    pub fn new(k2: f64, k4: f64) -> Self {
        Self {
            quadratic_coeff: k2,
            quartic_coeff: k4,
        }
    }
}

impl PotentialFunction for AnharmonicPotential {
    fn value(&self, positions: &DVector<f64>) -> Result<f64> {
        let r2 = positions.dot(positions);
        Ok(self.quadratic_coeff * r2 / 2.0 + self.quartic_coeff * r2 * r2 / 4.0)
    }
    
    fn gradient(&self, positions: &DVector<f64>) -> Result<DVector<f64>> {
        let r2 = positions.dot(positions);
        Ok(positions * (self.quadratic_coeff + self.quartic_coeff * r2))
    }
}

/// Price dynamics model using Hamiltonian mechanics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceDynamics {
    /// Base price level
    pub base_price: f64,
    
    /// Volatility parameter
    pub volatility: f64,
    
    /// Mean reversion rate
    pub mean_reversion: f64,
    
    /// Current price
    pub current_price: f64,
    
    /// Price momentum
    pub momentum: f64,
}

impl PriceDynamics {
    /// Create new price dynamics
    pub fn new(base_price: f64, volatility: f64, mean_reversion: f64) -> Self {
        Self {
            base_price,
            volatility,
            mean_reversion,
            current_price: base_price,
            momentum: 0.0,
        }
    }
    
    /// Update price using Hamiltonian dynamics
    pub fn update(&mut self, hamiltonian: &HamiltonianDynamics, dt: f64) -> Result<()> {
        // Get energy-based modulation
        let energy = hamiltonian.total_energy()?;
        let energy_factor = (energy / hamiltonian.energy_scale).tanh();
        
        // Mean reversion force
        let reversion_force = self.mean_reversion * (self.base_price - self.current_price);
        
        // Stochastic component
        use rand_distr::{Normal, Distribution};
        let noise_dist = Normal::new(0.0, self.volatility * dt.sqrt()).unwrap();
        let noise = noise_dist.sample(&mut rand::thread_rng());
        
        // Update momentum with forces
        self.momentum += (reversion_force + noise * energy_factor) * dt;
        self.momentum *= 1.0 - 0.1 * dt; // Friction
        
        // Update price
        self.current_price += self.momentum * dt;
        self.current_price = self.current_price.max(0.01); // Floor at 1 cent
        
        Ok(())
    }
    
    /// Calculate effective cost per token
    pub fn cost_per_token(&self) -> f64 {
        self.current_price / 1_000_000.0 // Price per million tokens
    }
    
    /// Get price statistics
    pub fn statistics(&self) -> PriceStatistics {
        PriceStatistics {
            current: self.current_price,
            base: self.base_price,
            momentum: self.momentum,
            volatility: self.volatility,
            deviation: (self.current_price - self.base_price).abs() / self.base_price,
        }
    }
}

/// Price statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceStatistics {
    pub current: f64,
    pub base: f64,
    pub momentum: f64,
    pub volatility: f64,
    pub deviation: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_hamiltonian_energy_conservation() {
        let mut ham = HamiltonianDynamics::new(1.0, 3);
        ham.phase_space = PhaseSpace::random(3, 1.0);
        ham.friction = 0.0; // No friction for energy conservation test
        
        let initial_energy = ham.total_energy().unwrap();
        
        // Evolve system
        for _ in 0..100 {
            ham.evolve(0.01).unwrap();
        }
        
        let final_energy = ham.total_energy().unwrap();
        
        // Energy should be approximately conserved (within numerical error)
        assert!((final_energy - initial_energy).abs() < 0.1);
    }
    
    #[test]
    fn test_price_dynamics() {
        let ham = HamiltonianDynamics::new(1.0, 2);
        let mut price = PriceDynamics::new(100.0, 0.2, 0.1);
        
        for _ in 0..10 {
            price.update(&ham, 0.1).unwrap();
        }
        
        // Price should remain positive
        assert!(price.current_price > 0.0);
        
        // Should have some movement
        assert!((price.current_price - price.base_price).abs() > 0.0);
    }
}