use hanzo_hvm::Market;

fn market() -> Market {
    Market { q: vec![1000.0, 1000.0], p: vec![1.0, 1.0], mass: vec![50.0, 50.0], scarcity: vec![100.0, 100.0] }
}

#[test]
fn quote_raises_price_on_demand() {
    let mut m = market();
    let p0 = m.p[0];
    let small = m.quote(0, 10.0, 0.1, 1.0);
    assert!(small > p0, "small buy must raise price: {p0} -> {small}");
    let large = market().quote(0, 500.0, 0.1, 1.0);
    assert!(large > small, "bigger buy must cost more: {small} vs {large}");
}

#[test]
fn symplectic_conserves_hamiltonian() {
    let mut m = market();
    let h0 = m.hamiltonian();
    for _ in 0..100_000 { m.step(0.001); }
    let drift = (m.hamiltonian() - h0).abs() / h0;
    assert!(drift < 1e-3, "symplectic integrator must conserve H; drift={drift:e}");
}

#[test]
fn depth_reduces_impact() {
    let mut shallow = Market { q: vec![1000.0], p: vec![1.0], mass: vec![10.0], scarcity: vec![100.0] };
    let mut deep = Market { q: vec![1000.0], p: vec![1.0], mass: vec![1000.0], scarcity: vec![100.0] };
    let si = (shallow.quote(0, 100.0, 0.1, 1.0) - 1.0).abs();
    let di = (deep.quote(0, 100.0, 0.1, 1.0) - 1.0).abs();
    assert!(di < si, "deeper market must have lower impact: shallow={si} deep={di}");
}
