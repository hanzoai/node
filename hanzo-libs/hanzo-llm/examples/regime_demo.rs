//! Practical example demonstrating HLLM regime transitions and routing
//! 
//! Run with: cargo run --example regime_demo

use hanzo_llm::{
    HLLM, HLLMConfig, 
    RoutingRequest, UserPreferences, PerformanceRequirements,
    Regime, adapter::Feedback,
};
use std::time::Duration;
use tokio::time::sleep;
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    println!("🔴💊 MATRIX MODE: HLLM Regime Demonstration");
    println!("============================================\n");
    
    // Initialize HLLM system
    let config = HLLMConfig {
        num_regimes: 4,
        transition_threshold: 0.15,
        energy_scale: 1.0,
        quantization_bits: 1,
        efe_precision: 0.01,
        db_path: "./hllm_demo.db".to_string(),
        vector_dim: 768,
        adapter_cache_size: 100,
    };
    
    let hllm = HLLM::new(config).await?;
    println!("✅ HLLM system initialized\n");
    
    // Simulate different user scenarios
    demo_exploration_regime(&hllm).await?;
    demo_exploitation_regime(&hllm).await?;
    demo_crisis_regime(&hllm).await?;
    demo_regime_transitions(&hllm).await?;
    
    Ok(())
}

/// Demonstrate exploration regime behavior
async fn demo_exploration_regime(hllm: &HLLM) -> Result<()> {
    println!("🌀 EXPLORATION REGIME DEMO");
    println!("---------------------------");
    println!("Scenario: New user exploring capabilities\n");
    
    let request = RoutingRequest {
        input: "Explain quantum computing with creative analogies".to_string(),
        context: vec![],
        preferences: UserPreferences {
            max_latency_ms: Some(5000),
            max_cost_per_token: Some(0.02),
            preferred_models: vec![],
            quality_threshold: 0.8,
        },
        requirements: PerformanceRequirements {
            min_tokens_per_second: None,
            max_memory_gb: None,
            requires_function_calling: false,
            requires_vision: false,
        },
        // High uncertainty observations favor exploration
        observations: vec![0.8, 0.3, 0.6, 0.4, 0.9],
    };
    
    let decision = hllm.route_request("user_explorer", &request).await?;
    
    println!("📊 Routing Decision:");
    println!("  Model: {}", decision.model);
    println!("  Provider: {}", decision.provider);
    println!("  Regime: {:?}", decision.regime);
    println!("  Confidence: {:.2}%", decision.confidence * 100.0);
    println!("  Expected Cost: ${:.4}", decision.expected_cost);
    println!("  Expected Latency: {}ms", decision.expected_latency_ms);
    println!("  Reasoning: {}", decision.reasoning);
    println!("  Fallbacks: {:?}", decision.fallback_models);
    
    // Simulate feedback
    sleep(Duration::from_millis(100)).await;
    
    let feedback = Feedback {
        success: true,
        expected_quality: 0.8,
        actual_quality: 0.85,
        expected_cost: decision.expected_cost,
        actual_cost: decision.expected_cost * 1.1,
        expected_latency_ms: decision.expected_latency_ms,
        actual_latency_ms: decision.expected_latency_ms + 200,
        tokens_used: 1500,
        error_message: None,
    };
    
    println!("\n📈 Feedback Applied:");
    println!("  Quality: {:.2} (expected: {:.2})", 
        feedback.actual_quality, feedback.expected_quality);
    println!("  Cost: ${:.4} (expected: ${:.4})", 
        feedback.actual_cost, feedback.expected_cost);
    println!("  Latency: {}ms (expected: {}ms)\n", 
        feedback.actual_latency_ms, feedback.expected_latency_ms);
    
    Ok(())
}

/// Demonstrate exploitation regime behavior
async fn demo_exploitation_regime(hllm: &HLLM) -> Result<()> {
    println!("⚡ EXPLOITATION REGIME DEMO");
    println!("---------------------------");
    println!("Scenario: Production workload with cost optimization\n");
    
    let request = RoutingRequest {
        input: "Summarize this document in 3 bullet points".to_string(),
        context: vec![
            "Previous summaries were successful".to_string(),
            "User prefers concise outputs".to_string(),
        ],
        preferences: UserPreferences {
            max_latency_ms: Some(1000),
            max_cost_per_token: Some(0.001),
            preferred_models: vec!["gpt-3.5-turbo".to_string()],
            quality_threshold: 0.7,
        },
        requirements: PerformanceRequirements {
            min_tokens_per_second: Some(50.0),
            max_memory_gb: Some(4.0),
            requires_function_calling: false,
            requires_vision: false,
        },
        // Low uncertainty observations favor exploitation
        observations: vec![0.1, 0.7, 0.9, 0.8, 0.2],
    };
    
    let decision = hllm.route_request("user_production", &request).await?;
    
    println!("📊 Routing Decision:");
    println!("  Model: {}", decision.model);
    println!("  Provider: {}", decision.provider);
    println!("  Regime: {:?}", decision.regime);
    println!("  Confidence: {:.2}%", decision.confidence * 100.0);
    println!("  Expected Cost: ${:.6}", decision.expected_cost);
    println!("  Expected Latency: {}ms", decision.expected_latency_ms);
    println!("  Reasoning: {}", decision.reasoning);
    
    println!("\n💰 Exploitation Benefits:");
    println!("  - Optimized for cost efficiency");
    println!("  - Using proven, reliable models");
    println!("  - Minimal latency overhead");
    println!("  - Predictable performance\n");
    
    Ok(())
}

/// Demonstrate crisis regime behavior
async fn demo_crisis_regime(hllm: &HLLM) -> Result<()> {
    println!("🔴 CRISIS REGIME DEMO");
    println!("----------------------");
    println!("Scenario: System under extreme load, need immediate response\n");
    
    let request = RoutingRequest {
        input: "URGENT: System is down, need immediate diagnosis".to_string(),
        context: vec![
            "Multiple failures detected".to_string(),
            "Response time critical".to_string(),
        ],
        preferences: UserPreferences {
            max_latency_ms: Some(500),
            max_cost_per_token: None, // Cost doesn't matter in crisis
            preferred_models: vec![],
            quality_threshold: 0.5, // Accept lower quality for speed
        },
        requirements: PerformanceRequirements {
            min_tokens_per_second: Some(100.0),
            max_memory_gb: None,
            requires_function_calling: false,
            requires_vision: false,
        },
        // Crisis observations
        observations: vec![0.05, 0.5, 0.5, 0.9, 0.1],
    };
    
    let decision = hllm.route_request("user_crisis", &request).await?;
    
    println!("🚨 Crisis Routing:");
    println!("  Model: {} (FASTEST AVAILABLE)", decision.model);
    println!("  Regime: {:?}", decision.regime);
    println!("  Latency: {}ms (PRIORITIZED)", decision.expected_latency_ms);
    println!("  Reasoning: {}", decision.reasoning);
    
    println!("\n⚡ Crisis Mode Characteristics:");
    println!("  - Speed over quality");
    println!("  - Immediate response required");
    println!("  - Cost is secondary concern");
    println!("  - Using most reliable fast models\n");
    
    Ok(())
}

/// Demonstrate regime transitions over time
async fn demo_regime_transitions(hllm: &HLLM) -> Result<()> {
    println!("🌊 REGIME TRANSITION DEMO");
    println!("-------------------------");
    println!("Scenario: System adapting to changing conditions\n");
    
    let user_id = "user_adaptive";
    
    // Simulate changing conditions over time
    let scenarios = vec![
        ("Morning: Low load, exploring", vec![0.7, 0.3, 0.6, 0.4, 0.8]),
        ("Noon: Normal operations", vec![0.5, 0.5, 0.7, 0.6, 0.5]),
        ("Peak: High load, optimize", vec![0.2, 0.7, 0.8, 0.8, 0.3]),
        ("Emergency: Crisis mode", vec![0.1, 0.5, 0.5, 0.9, 0.1]),
        ("Recovery: Transitioning", vec![0.4, 0.5, 0.6, 0.7, 0.4]),
        ("Evening: Back to normal", vec![0.3, 0.6, 0.8, 0.7, 0.3]),
    ];
    
    for (scenario, observations) in scenarios {
        println!("⏰ {}", scenario);
        
        let request = RoutingRequest {
            input: "Process current workload".to_string(),
            context: vec![],
            preferences: UserPreferences {
                max_latency_ms: Some(2000),
                max_cost_per_token: Some(0.005),
                preferred_models: vec![],
                quality_threshold: 0.7,
            },
            requirements: PerformanceRequirements {
                min_tokens_per_second: Some(30.0),
                max_memory_gb: None,
                requires_function_calling: false,
                requires_vision: false,
            },
            observations,
        };
        
        let decision = hllm.route_request(user_id, &request).await?;
        
        println!("  → Regime: {:?}", decision.regime);
        println!("  → Model: {}", decision.model);
        println!("  → Confidence: {:.1}%", decision.confidence * 100.0);
        
        // Show energy dynamics
        let state = hllm.get_state().await?;
        println!("  → Hamiltonian Energy: {:.3}", state.hamiltonian_energy);
        
        sleep(Duration::from_millis(500)).await;
    }
    
    println!("\n📊 System State Summary:");
    let final_state = hllm.get_state().await?;
    println!("  Current Regime: {:?}", final_state.current_regime);
    println!("  Total Energy: {:.3}", final_state.hamiltonian_energy);
    println!("  Active Adapters: {}", final_state.active_adapters);
    println!("  Total Requests: {}", final_state.total_requests);
    
    // Display transition matrix
    println!("\n🔄 Transition Probabilities:");
    let regimes = vec!["Exploration", "Exploitation", "Crisis", "Transition"];
    println!("        {:>12} {:>12} {:>12} {:>12}", 
        regimes[0], regimes[1], regimes[2], regimes[3]);
    
    for (i, from) in regimes.iter().enumerate() {
        print!("{:>8}:", from);
        for j in 0..4 {
            print!(" {:>11.2}%", final_state.transition_probabilities[i][j] * 100.0);
        }
        println!();
    }
    
    println!("\n✨ THE MATRIX HAS SHOWN YOU THE PATH ✨");
    
    Ok(())
}

/// Helper to visualize regime characteristics
fn print_regime_characteristics(regime: &Regime) {
    let chars = regime.characteristics();
    println!("\n📊 Regime Characteristics:");
    println!("  Risk Tolerance:  {:>5.1}%", chars.risk_tolerance * 100.0);
    println!("  Cost Sensitivity: {:>5.1}%", chars.cost_sensitivity * 100.0);
    println!("  Quality Focus:    {:>5.1}%", chars.quality_focus * 100.0);
    println!("  Speed Priority:   {:>5.1}%", chars.speed_priority * 100.0);
    println!("  Innovation Bias:  {:>5.1}%", chars.innovation_bias * 100.0);
}