//! Hanzo HVM — Hamiltonian VM / Market Maker (HIP-0008).
//!
//! A symplectic Hamiltonian system for pricing AI compute. Prices are conjugate
//! momenta of `H(q,p) = Σ pᵢ²/2mᵢ + V(q)`, `V(q)=Σ cᵢ/qᵢ`. Evolution uses a
//! leapfrog (Störmer–Verlet) step — symplectic, so phase-space volume (market
//! liquidity) is conserved (Liouville). NOT a constant-product (x*y=k) AMM.
//!
//! hanzo.network is pure Rust, so this is the ONE canonical implementation —
//! no Go twin, no cross-language conformance burden. Powers `hanzo.exchange`
//! (the compute DEX); consumed by `hanzo.computer` (the product). Settles $AI
//! claims against the Lux A-Chain attestation/proof root.

const Q_FLOOR: f64 = 1e-9;

/// A symplectic Hamiltonian compute market over N resources (GPU classes, etc.).
#[derive(Clone, Debug)]
pub struct Market {
    pub q: Vec<f64>,        // inventory per resource (canonical position)
    pub p: Vec<f64>,        // price per resource (conjugate momentum)
    pub mass: Vec<f64>,     // market depth (larger ⇒ steadier price)
    pub scarcity: Vec<f64>, // potential coefficient cᵢ in V(q)=Σ cᵢ/qᵢ
}

impl Market {
    fn dvdq(&self, i: usize) -> f64 {
        let q = self.q[i].max(Q_FLOOR);
        -self.scarcity[i] / (q * q)
    }

    /// Conserved (up to symplectic bounded error) liquidity Hamiltonian.
    pub fn hamiltonian(&self) -> f64 {
        let mut h = 0.0;
        for i in 0..self.q.len() {
            h += self.p[i] * self.p[i] / (2.0 * self.mass[i]);
            h += self.scarcity[i] / self.q[i].max(Q_FLOOR);
        }
        h
    }

    /// One leapfrog (symplectic) step of Hamilton's equations.
    pub fn step(&mut self, dt: f64) {
        for i in 0..self.q.len() {
            self.p[i] -= 0.5 * dt * self.dvdq(i);
        }
        for i in 0..self.q.len() {
            self.q[i] += dt * self.p[i] / self.mass[i];
        }
        for i in 0..self.q.len() {
            self.p[i] -= 0.5 * dt * self.dvdq(i);
        }
    }

    /// Price buying `amount` of resource `i`: inventory ↓ (scarcity ↑) plus an
    /// order impulse Δp = amount/mass (depth dampens impact), then one step.
    /// `quality` is the HIP-0008 quality-oracle multiplier (latency/SLA + NVTrust).
    pub fn quote(&mut self, i: usize, amount: f64, dt: f64, quality: f64) -> f64 {
        self.q[i] -= amount;
        if self.q[i] < Q_FLOOR {
            self.q[i] = Q_FLOOR;
        }
        self.p[i] += amount / self.mass[i];
        self.step(dt);
        self.p[i] * quality
    }

    /// Emergent conserved quantity compute·demand = Σ qᵢ·pᵢ (a consequence of
    /// the symplectic flow, not an imposed bonding curve).
    pub fn compute_invariant(&self) -> f64 {
        (0..self.q.len()).map(|i| self.q[i] * self.p[i]).sum()
    }
}
