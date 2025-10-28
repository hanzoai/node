//! Example: HLLM Regime Routing on Hanzo Network
//!
//! Shows how Hidden Markov Models track context and route to specialized models

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Latent regime states for the Hidden Markov Model
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
enum Regime {
    General,    // General conversation
    Medical,    // Medical/health context
    Legal,      // Legal/compliance context
    Code,       // Programming/technical
    Creative,   // Art/writing/creative
    Financial,  // Trading/DeFi/markets
}

/// HLLM Router that tracks regimes and routes accordingly
struct HLLMRouter {
    /// Current regime state
    current_regime: Regime,

    /// Transition probabilities P(z_t | z_{t-1})
    transition_matrix: HashMap<(Regime, Regime), f64>,

    /// Observation model P(x | z)
    observation_model: HashMap<Regime, RegimeClassifier>,

    /// Regime-specific adapters (LoRA weights)
    adapters: HashMap<Regime, AdapterWeights>,

    /// Hanzo node connection
    hanzo_client: HanzoClient,
}

/// Regime classifier based on input features
struct RegimeClassifier {
    keywords: Vec<String>,
    embedding_centroid: Vec<f32>,
    confidence_threshold: f64,
}

/// Adapter weights for each regime (BitDelta 1-bit quantized)
struct AdapterWeights {
    regime: Regime,
    base_model: String,
    delta_bits: Vec<u8>,      // 1-bit deltas
    scale: f32,                // Learned scale
    sparsity_mask: Vec<bool>,  // Which weights to modify
}

/// Hanzo network client
struct HanzoClient {
    node_url: String,
    wallet_key: String,
}

impl HLLMRouter {
    pub async fn new() -> Result<Self> {
        let mut router = Self {
            current_regime: Regime::General,
            transition_matrix: Self::init_transition_matrix(),
            observation_model: Self::init_observation_model(),
            adapters: Self::load_adapters().await?,
            hanzo_client: HanzoClient {
                node_url: std::env::var("HANZO_NODE_URL")
                    .unwrap_or_else(|_| "http://localhost:3690".to_string()),
                wallet_key: std::env::var("HANZO_WALLET_KEY")
                    .unwrap_or_else(|_| "test-key".to_string()),
            },
        };

        Ok(router)
    }

    /// Initialize HMM transition probabilities
    fn init_transition_matrix() -> HashMap<(Regime, Regime), f64> {
        let mut matrix = HashMap::new();

        // High probability of staying in same regime
        for regime in &[Regime::General, Regime::Medical, Regime::Legal,
                       Regime::Code, Regime::Creative, Regime::Financial] {
            matrix.insert((*regime, *regime), 0.7); // 70% stay
        }

        // General regime transitions
        matrix.insert((Regime::General, Regime::Medical), 0.05);
        matrix.insert((Regime::General, Regime::Legal), 0.05);
        matrix.insert((Regime::General, Regime::Code), 0.1);
        matrix.insert((Regime::General, Regime::Creative), 0.05);
        matrix.insert((Regime::General, Regime::Financial), 0.05);

        // Domain-specific transitions
        matrix.insert((Regime::Medical, Regime::General), 0.2);
        matrix.insert((Regime::Medical, Regime::Legal), 0.1); // Medical-legal overlap

        matrix.insert((Regime::Code, Regime::General), 0.15);
        matrix.insert((Regime::Code, Regime::Financial), 0.15); // Code-DeFi overlap

        matrix.insert((Regime::Financial, Regime::Legal), 0.1); // Compliance
        matrix.insert((Regime::Financial, Regime::Code), 0.1);  // Smart contracts

        matrix
    }

    /// Initialize observation models for each regime
    fn init_observation_model() -> HashMap<Regime, RegimeClassifier> {
        let mut models = HashMap::new();

        models.insert(Regime::Medical, RegimeClassifier {
            keywords: vec![
                "diagnosis".to_string(),
                "treatment".to_string(),
                "symptoms".to_string(),
                "patient".to_string(),
                "medication".to_string(),
            ],
            embedding_centroid: vec![0.1; 768], // Placeholder
            confidence_threshold: 0.7,
        });

        models.insert(Regime::Legal, RegimeClassifier {
            keywords: vec![
                "contract".to_string(),
                "liability".to_string(),
                "compliance".to_string(),
                "regulation".to_string(),
                "jurisdiction".to_string(),
            ],
            embedding_centroid: vec![0.2; 768],
            confidence_threshold: 0.8,
        });

        models.insert(Regime::Code, RegimeClassifier {
            keywords: vec![
                "function".to_string(),
                "variable".to_string(),
                "compile".to_string(),
                "debug".to_string(),
                "algorithm".to_string(),
            ],
            embedding_centroid: vec![0.3; 768],
            confidence_threshold: 0.6,
        });

        models.insert(Regime::Financial, RegimeClassifier {
            keywords: vec![
                "trading".to_string(),
                "liquidity".to_string(),
                "yield".to_string(),
                "defi".to_string(),
                "amm".to_string(),
            ],
            embedding_centroid: vec![0.4; 768],
            confidence_threshold: 0.7,
        });

        models
    }

    /// Load regime-specific adapters
    async fn load_adapters() -> Result<HashMap<Regime, AdapterWeights>> {
        let mut adapters = HashMap::new();

        // Load BitDelta adapters for each regime
        // These would be fetched from Hanzo network or local storage

        adapters.insert(Regime::Medical, AdapterWeights {
            regime: Regime::Medical,
            base_model: "qwen3-30b-a3b".to_string(),
            delta_bits: vec![0xFF; 1024], // Placeholder
            scale: 0.01,
            sparsity_mask: vec![true; 8192],
        });

        adapters.insert(Regime::Code, AdapterWeights {
            regime: Regime::Code,
            base_model: "qwen3-30b-a3b".to_string(),
            delta_bits: vec![0xAA; 1024],
            scale: 0.015,
            sparsity_mask: vec![true; 8192],
        });

        Ok(adapters)
    }

    /// Process a request with regime routing
    pub async fn process(&mut self, request: InferenceRequest) -> Result<InferenceResponse> {
        println!("📊 Processing request: {}", request.text);

        // 1. Infer current regime from observation
        let observed_regime = self.infer_regime(&request.text)?;
        println!("🔍 Observed regime: {:?}", observed_regime);

        // 2. Update regime state using HMM transition
        self.current_regime = self.transition_regime(observed_regime);
        println!("🎯 Current regime: {:?}", self.current_regime);

        // 3. Select adapter for current regime
        let adapter = self.adapters.get(&self.current_regime);

        // 4. Route to appropriate provider on Hanzo network
        let provider = self.select_provider(self.current_regime, &request.slo).await?;
        println!("🖥️ Selected provider: {}", provider.id);

        // 5. Calculate pricing using Hamiltonian market maker
        let price = self.calculate_price(&provider, &request).await?;
        println!("💰 Compute price: {} HANZO", price);

        // 6. Execute inference with regime-specific adapter
        let result = self.execute_inference(
            &request,
            &provider,
            adapter,
        ).await?;

        // 7. Generate proof of computation
        let proof = self.generate_proof(&result, &provider).await?;

        Ok(InferenceResponse {
            text: result,
            regime: self.current_regime,
            provider_id: provider.id,
            price_paid: price,
            proof_hash: proof,
            confidence: 0.95,
        })
    }

    /// Infer regime from text
    fn infer_regime(&self, text: &str) -> Result<Regime> {
        let text_lower = text.to_lowercase();
        let mut scores = HashMap::new();

        // Score each regime based on keyword matches
        for (regime, classifier) in &self.observation_model {
            let mut score = 0.0;
            for keyword in &classifier.keywords {
                if text_lower.contains(keyword) {
                    score += 1.0;
                }
            }
            scores.insert(regime, score / classifier.keywords.len() as f64);
        }

        // Find regime with highest score
        let best_regime = scores
            .iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(regime, _)| **regime)
            .unwrap_or(Regime::General);

        Ok(best_regime)
    }

    /// Transition regime using HMM
    fn transition_regime(&self, observed: Regime) -> Regime {
        // Sample from transition distribution
        let transition_prob = self.transition_matrix
            .get(&(self.current_regime, observed))
            .unwrap_or(&0.1);

        // For demo, use simple threshold
        if *transition_prob > 0.3 {
            observed
        } else {
            self.current_regime
        }
    }

    /// Select compute provider based on regime and SLO
    async fn select_provider(&self, regime: Regime, slo: &ServiceLevel) -> Result<Provider> {
        // Different regimes may prefer different providers
        // Medical: High-security TEE providers
        // Code: Fast GPU providers
        // Financial: Low-latency providers

        let provider = match regime {
            Regime::Medical => Provider {
                id: "secure-tee-001".to_string(),
                capabilities: vec!["SEV-SNP".to_string(), "HIPAA".to_string()],
                price_per_token: 0.0001,
                latency_ms: 100,
            },
            Regime::Code => Provider {
                id: "gpu-fast-002".to_string(),
                capabilities: vec!["A100".to_string(), "CUDA".to_string()],
                price_per_token: 0.00005,
                latency_ms: 50,
            },
            Regime::Financial => Provider {
                id: "low-latency-003".to_string(),
                capabilities: vec!["Colocated".to_string(), "10Gbps".to_string()],
                price_per_token: 0.00015,
                latency_ms: 20,
            },
            _ => Provider {
                id: "general-004".to_string(),
                capabilities: vec!["Standard".to_string()],
                price_per_token: 0.00003,
                latency_ms: 150,
            },
        };

        Ok(provider)
    }

    /// Calculate price using market dynamics
    async fn calculate_price(&self, provider: &Provider, request: &InferenceRequest) -> Result<f64> {
        let base_price = provider.price_per_token * request.max_tokens as f64;

        // Apply regime-specific pricing
        let regime_multiplier = match self.current_regime {
            Regime::Medical => 1.5,  // Premium for medical
            Regime::Legal => 1.3,    // Premium for legal
            Regime::Financial => 1.2, // Premium for financial
            _ => 1.0,
        };

        Ok(base_price * regime_multiplier)
    }

    /// Execute inference on Hanzo network
    async fn execute_inference(
        &self,
        request: &InferenceRequest,
        provider: &Provider,
        adapter: Option<&AdapterWeights>,
    ) -> Result<String> {
        // In production, this would:
        // 1. Submit job to Hanzo network
        // 2. Apply BitDelta adapter if present
        // 3. Stream results back

        Ok(format!(
            "Response from {} using {:?} regime adapter: {}",
            provider.id,
            self.current_regime,
            request.text
        ))
    }

    /// Generate proof of computation
    async fn generate_proof(&self, result: &str, provider: &Provider) -> Result<String> {
        // Generate proof hash (simplified)
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        result.hash(&mut hasher);
        provider.id.hash(&mut hasher);

        Ok(format!("{:x}", hasher.finish()))
    }
}

/// Inference request
#[derive(Debug, Serialize, Deserialize)]
struct InferenceRequest {
    text: String,
    max_tokens: u32,
    temperature: f32,
    slo: ServiceLevel,
}

/// Service level objectives
#[derive(Debug, Serialize, Deserialize)]
struct ServiceLevel {
    max_latency_ms: u32,
    min_accuracy: f32,
    privacy_tier: String,
}

/// Inference response
#[derive(Debug, Serialize, Deserialize)]
struct InferenceResponse {
    text: String,
    regime: Regime,
    provider_id: String,
    price_paid: f64,
    proof_hash: String,
    confidence: f64,
}

/// Compute provider
#[derive(Debug, Clone)]
struct Provider {
    id: String,
    capabilities: Vec<String>,
    price_per_token: f64,
    latency_ms: u32,
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("🚀 HLLM Regime Routing Example on Hanzo Network\n");

    let mut router = HLLMRouter::new().await?;

    // Example requests showing regime transitions
    let requests = vec![
        InferenceRequest {
            text: "What are the symptoms of diabetes?".to_string(),
            max_tokens: 100,
            temperature: 0.7,
            slo: ServiceLevel {
                max_latency_ms: 200,
                min_accuracy: 0.95,
                privacy_tier: "HIPAA".to_string(),
            },
        },
        InferenceRequest {
            text: "How do I implement a binary search tree?".to_string(),
            max_tokens: 200,
            temperature: 0.5,
            slo: ServiceLevel {
                max_latency_ms: 100,
                min_accuracy: 0.9,
                privacy_tier: "Standard".to_string(),
            },
        },
        InferenceRequest {
            text: "Calculate the impermanent loss for this liquidity position".to_string(),
            max_tokens: 150,
            temperature: 0.3,
            slo: ServiceLevel {
                max_latency_ms: 50,
                min_accuracy: 0.99,
                privacy_tier: "Financial".to_string(),
            },
        },
    ];

    for request in requests {
        println!("\n" + "═".repeat(60));
        let response = router.process(request).await?;
        println!("\n✅ Response:");
        println!("  Regime: {:?}", response.regime);
        println!("  Provider: {}", response.provider_id);
        println!("  Price: {} HANZO", response.price_paid);
        println!("  Proof: {}", response.proof_hash);
    }

    println!("\n" + "═".repeat(60));
    println!("✨ HLLM routing complete!");

    Ok(())
}