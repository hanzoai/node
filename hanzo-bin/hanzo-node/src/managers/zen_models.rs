//! Native Zen model SKU catalog.
//!
//! Single source of truth for every Zen model SKU served by Hanzo Node
//! (cross-checked against `zen-gateway/gateway/config.yaml`,
//! `pricing/src/models.mjs::zenCatalog` and `cloud/conf/models.yaml`).
//!
//! Node operators that point a `LLMProviderInterface::Ollama` /
//! `LLMProviderInterface::OpenRouter` / `LLMProviderInterface::HanzoBackend`
//! provider at a `model_type` listed here get first-class capability,
//! context-window, cost and tool/reasoning metadata via the helpers in this
//! module. The `model_capabilities_manager` consults this catalog before
//! falling back to its provider-generic heuristics.
//!
//! NOTE on branding: all `display_name` / `description` strings reference
//! the Zen brand only. Upstream base-model attribution (Qwen / DeepSeek /
//! MiniMax / GLM / Llama) is preserved in source code comments because the
//! HuggingFace `base_model:` chain points to those repos, but it is never
//! surfaced to operators or end users.

use super::model_capabilities_manager::{ModelCapability, ModelCost, ModelPrivacy};

/// Modalities a Zen SKU exposes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZenModality {
    Text,
    Vision,
    Audio,
    Video,
    Embedding,
    Rerank,
    AudioTranscription,
    AudioSpeech,
}

/// Compact metadata for one Zen SKU.
#[derive(Clone, Debug)]
pub struct ZenModelInfo {
    /// Canonical SKU string a node operator references (e.g. `"zen5"`).
    pub sku: &'static str,
    /// HuggingFace repo id where weights live (per Agent 1's catalog).
    /// `None` for SKUs that are gateway-only routing aliases without
    /// distinct weights of their own.
    pub hf_repo: Option<&'static str>,
    /// Maximum context window (tokens).
    pub context_window: usize,
    /// Default max output tokens.
    pub max_output_tokens: usize,
    /// Modalities exposed by this SKU.
    pub modalities: &'static [ZenModality],
    /// Input price per million tokens, USD.
    pub input_price_per_mtok: f64,
    /// Output price per million tokens, USD.
    pub output_price_per_mtok: f64,
    /// Tool / function-calling capability.
    pub tools: bool,
    /// Reasoning / chain-of-thought capability.
    pub reasoning: bool,
}

impl ZenModelInfo {
    pub fn has_modality(&self, m: ZenModality) -> bool {
        self.modalities.contains(&m)
    }

    pub fn capabilities(&self) -> Vec<ModelCapability> {
        let mut caps = Vec::with_capacity(4);
        // Treat embedding/rerank/asr/tts as text inference for the
        // ModelCapability surface (no dedicated variants exist today),
        // but skip TextInference for pure embedding/rerank.
        let is_pure_embedding = self
            .modalities
            .iter()
            .all(|m| matches!(m, ZenModality::Embedding | ZenModality::Rerank));
        if !is_pure_embedding {
            caps.push(ModelCapability::TextInference);
        } else {
            // Embedding/rerank still count as TextInference for routing
            // purposes — clients ask for "text" workloads either way.
            caps.push(ModelCapability::TextInference);
        }
        if self.has_modality(ZenModality::Vision) {
            caps.push(ModelCapability::ImageAnalysis);
        }
        if self.has_modality(ZenModality::Video) {
            caps.push(ModelCapability::VideoAnalysis);
        }
        if self.has_modality(ZenModality::Audio)
            || self.has_modality(ZenModality::AudioTranscription)
            || self.has_modality(ZenModality::AudioSpeech)
        {
            caps.push(ModelCapability::AudioAnalysis);
        }
        caps
    }

    pub fn cost(&self) -> ModelCost {
        // Bucket boundaries match what `model_capabilities_manager` uses for
        // other provider families.
        let avg = (self.input_price_per_mtok + self.output_price_per_mtok) / 2.0;
        match avg {
            x if x == 0.0 => ModelCost::Free,
            x if x < 0.30 => ModelCost::VeryCheap,
            x if x < 2.00 => ModelCost::Cheap,
            x if x < 8.00 => ModelCost::GoodValue,
            _ => ModelCost::Expensive,
        }
    }

    pub fn privacy(&self) -> ModelPrivacy {
        // Zen SKUs can be served either locally (via Ollama / a node operator
        // hosting OSS weights from zenlm/) or remotely via the gateway.
        // Default to Local because the registry is for node operators who
        // serve weights directly; remote routing is a separate concern.
        ModelPrivacy::Local
    }
}

/// All Zen text/multimodal SKUs supported by Hanzo Node, in the same
/// generational ordering as the gateway catalog.
///
/// Pricing reflects the public api.hanzo.ai rates (3x upstream margin)
/// — operators self-hosting OSS weights see zero marginal cost, but the
/// price field is exposed so node operators can compute reimbursement.
pub const ZEN_MODELS: &[ZenModelInfo] = &[
    // =====================================================================
    // Zen5 canonical ladder
    // =====================================================================
    ZenModelInfo {
        sku: "zen5-nano-0.8B",
        hf_repo: Some("zenlm/zen-5-nano-0.8b"),
        context_window: 32_000,
        max_output_tokens: 8_192,
        modalities: &[ZenModality::Text, ZenModality::Vision],
        input_price_per_mtok: 0.03,
        output_price_per_mtok: 0.09,
        tools: true,
        reasoning: false,
    },
    ZenModelInfo {
        sku: "zen5-nano-2B",
        hf_repo: Some("zenlm/zen-5-nano-2b"),
        context_window: 32_000,
        max_output_tokens: 8_192,
        modalities: &[ZenModality::Text, ZenModality::Vision],
        input_price_per_mtok: 0.05,
        output_price_per_mtok: 0.15,
        tools: true,
        reasoning: false,
    },
    ZenModelInfo {
        sku: "zen5-nano-4B",
        hf_repo: Some("zenlm/zen-5-nano-4b"),
        context_window: 32_000,
        max_output_tokens: 8_192,
        modalities: &[ZenModality::Text, ZenModality::Vision],
        input_price_per_mtok: 0.08,
        output_price_per_mtok: 0.24,
        tools: true,
        reasoning: false,
    },
    ZenModelInfo {
        sku: "zen5-nano-9B",
        hf_repo: Some("zenlm/zen-5-nano-9b"),
        context_window: 64_000,
        max_output_tokens: 16_384,
        modalities: &[ZenModality::Text, ZenModality::Vision],
        input_price_per_mtok: 0.15,
        output_price_per_mtok: 0.45,
        tools: true,
        reasoning: false,
    },
    ZenModelInfo {
        sku: "zen5-flash",
        // Dense 4B, text-only, fastest TTFT.
        hf_repo: Some("zenlm/zen-5-flash"),
        context_window: 32_000,
        max_output_tokens: 8_192,
        modalities: &[ZenModality::Text],
        input_price_per_mtok: 0.10,
        output_price_per_mtok: 0.30,
        tools: true,
        reasoning: false,
    },
    ZenModelInfo {
        sku: "zen5-mini",
        // Frontier agentic MoE — 230B params, 10B active.
        hf_repo: Some("zenlm/zen-5-mini"),
        context_window: 256_000,
        max_output_tokens: 32_768,
        modalities: &[ZenModality::Text],
        input_price_per_mtok: 0.30,
        output_price_per_mtok: 0.90,
        tools: true,
        reasoning: true,
    },
    ZenModelInfo {
        sku: "zen5",
        // Canonical default — 35B MoE / 3B active, 256K ctx, multimodal.
        hf_repo: Some("zenlm/zen-5"),
        context_window: 256_000,
        max_output_tokens: 32_768,
        modalities: &[ZenModality::Text, ZenModality::Vision],
        input_price_per_mtok: 0.60,
        output_price_per_mtok: 1.20,
        tools: true,
        reasoning: true,
    },
    ZenModelInfo {
        sku: "zen5-coder",
        // 80B code-specialized MoE.
        hf_repo: Some("zenlm/zen-5-coder"),
        context_window: 256_000,
        max_output_tokens: 32_768,
        modalities: &[ZenModality::Text],
        input_price_per_mtok: 1.00,
        output_price_per_mtok: 3.00,
        tools: true,
        reasoning: true,
    },
    ZenModelInfo {
        sku: "zen5-pro",
        // Zen Flash IQ2_XXS quant (81 GB GGUF) — fits 128 GB unified RAM.
        hf_repo: Some("zenlm/zen-5-pro-gguf"),
        context_window: 256_000,
        max_output_tokens: 32_768,
        modalities: &[ZenModality::Text],
        input_price_per_mtok: 2.70,
        output_price_per_mtok: 5.40,
        tools: true,
        reasoning: true,
    },
    ZenModelInfo {
        sku: "zen5-max",
        // Zen Pro full (432 GB) — multi-GPU / 512+ GB unified RAM class.
        hf_repo: Some("zenlm/zen-5-max-gguf"),
        context_window: 256_000,
        max_output_tokens: 32_768,
        modalities: &[ZenModality::Text],
        input_price_per_mtok: 9.00,
        output_price_per_mtok: 27.00,
        tools: true,
        reasoning: true,
    },
    // =====================================================================
    // Zen5 embeddings
    // =====================================================================
    ZenModelInfo {
        sku: "zen5-embedding-0.6B",
        hf_repo: Some("zenlm/zen-5-embedding-0.6b"),
        context_window: 32_000,
        max_output_tokens: 0,
        modalities: &[ZenModality::Embedding],
        input_price_per_mtok: 0.05,
        output_price_per_mtok: 0.05,
        tools: false,
        reasoning: false,
    },
    ZenModelInfo {
        sku: "zen5-embedding-4B",
        hf_repo: Some("zenlm/zen-5-embedding-4b"),
        context_window: 32_000,
        max_output_tokens: 0,
        modalities: &[ZenModality::Embedding],
        input_price_per_mtok: 0.10,
        output_price_per_mtok: 0.10,
        tools: false,
        reasoning: false,
    },
    ZenModelInfo {
        sku: "zen5-embedding-8B",
        hf_repo: Some("zenlm/zen-5-embedding-8b"),
        context_window: 32_000,
        max_output_tokens: 0,
        modalities: &[ZenModality::Embedding],
        input_price_per_mtok: 0.20,
        output_price_per_mtok: 0.20,
        tools: false,
        reasoning: false,
    },
    // =====================================================================
    // Zen4 generation sunset 2026-05-30 — zenlm/zen-4* HF mirrors deleted.
    // Routing for legacy callers still resolves via the gateway's Fireworks
    // aliases (zen-gateway/gateway/config.yaml model_access_groups), but
    // node operators can no longer self-host these SKUs. Migrate to the
    // zen5 ladder (zen5-flash / zen5-mini / zen5 / zen5-coder / zen5-pro /
    // zen5-max) for new integrations.
    // =====================================================================
    // Zen3 multimodal & specialty
    // =====================================================================
    ZenModelInfo {
        sku: "zen3-omni",
        // GLM-4.7-class general-purpose multimodal.
        hf_repo: Some("zenlm/zen-3-omni"),
        context_window: 202_000,
        max_output_tokens: 16_384,
        modalities: &[
            ZenModality::Text,
            ZenModality::Vision,
            ZenModality::Audio,
        ],
        input_price_per_mtok: 1.80,
        output_price_per_mtok: 6.60,
        tools: true,
        reasoning: false,
    },
    ZenModelInfo {
        sku: "zen3-vl",
        // 30B-A3B MoE vision-language default.
        hf_repo: Some("zenlm/zen-3-vl"),
        context_window: 262_000,
        max_output_tokens: 16_384,
        modalities: &[ZenModality::Text, ZenModality::Vision, ZenModality::Video],
        input_price_per_mtok: 0.45,
        output_price_per_mtok: 1.80,
        tools: true,
        reasoning: false,
    },
    ZenModelInfo {
        sku: "zen3-vl-2B",
        hf_repo: Some("zenlm/zen-3-vl-2b"),
        context_window: 128_000,
        max_output_tokens: 8_192,
        modalities: &[ZenModality::Text, ZenModality::Vision],
        input_price_per_mtok: 0.05,
        output_price_per_mtok: 0.20,
        tools: true,
        reasoning: false,
    },
    ZenModelInfo {
        sku: "zen3-vl-8B",
        hf_repo: Some("zenlm/zen-3-vl-8b"),
        context_window: 128_000,
        max_output_tokens: 8_192,
        modalities: &[ZenModality::Text, ZenModality::Vision],
        input_price_per_mtok: 0.15,
        output_price_per_mtok: 0.60,
        tools: true,
        reasoning: false,
    },
    ZenModelInfo {
        sku: "zen3-vl-32B",
        hf_repo: Some("zenlm/zen-3-vl-32b"),
        context_window: 256_000,
        max_output_tokens: 16_384,
        modalities: &[ZenModality::Text, ZenModality::Vision, ZenModality::Video],
        input_price_per_mtok: 0.60,
        output_price_per_mtok: 2.40,
        tools: true,
        reasoning: false,
    },
    ZenModelInfo {
        sku: "zen3-vl-235B-A22B",
        hf_repo: Some("zenlm/zen-3-vl-235b-a22b"),
        context_window: 262_000,
        max_output_tokens: 16_384,
        modalities: &[ZenModality::Text, ZenModality::Vision, ZenModality::Video],
        input_price_per_mtok: 2.70,
        output_price_per_mtok: 10.80,
        tools: true,
        reasoning: true,
    },
    // zen3-vl-reranker-{2B,8B}, zen3-vl-embedding-{2B,8B} and
    // zen3-web-{8B,14B,32B} were virtual SKUs with no HF weights — sunset
    // 2026-05-30. The canonical zen3-vl size variants above remain.
    ZenModelInfo {
        sku: "zen3-asr",
        hf_repo: Some("zenlm/zen-3-asr-1.7b"),
        context_window: 32_000,
        max_output_tokens: 0,
        modalities: &[ZenModality::Audio, ZenModality::AudioTranscription],
        input_price_per_mtok: 0.04,
        output_price_per_mtok: 0.04,
        tools: false,
        reasoning: false,
    },
    ZenModelInfo {
        sku: "zen3-asr-0.6B",
        hf_repo: Some("zenlm/zen-3-asr-0.6b"),
        context_window: 32_000,
        max_output_tokens: 0,
        modalities: &[ZenModality::Audio, ZenModality::AudioTranscription],
        input_price_per_mtok: 0.02,
        output_price_per_mtok: 0.02,
        tools: false,
        reasoning: false,
    },
    ZenModelInfo {
        sku: "zen3-asr-aligner",
        hf_repo: Some("zenlm/zen-3-asr-aligner-0.6b"),
        context_window: 32_000,
        max_output_tokens: 0,
        modalities: &[ZenModality::Audio, ZenModality::AudioTranscription],
        input_price_per_mtok: 0.02,
        output_price_per_mtok: 0.02,
        tools: false,
        reasoning: false,
    },
    ZenModelInfo {
        sku: "zen3-tts",
        hf_repo: Some("zenlm/zen-3-tts-1.7b"),
        context_window: 8_192,
        max_output_tokens: 0,
        modalities: &[ZenModality::AudioSpeech],
        input_price_per_mtok: 5.00,
        output_price_per_mtok: 5.00,
        tools: false,
        reasoning: false,
    },
    ZenModelInfo {
        sku: "zen3-tts-0.6B",
        hf_repo: Some("zenlm/zen-3-tts-0.6b"),
        context_window: 8_192,
        max_output_tokens: 0,
        modalities: &[ZenModality::AudioSpeech],
        input_price_per_mtok: 2.00,
        output_price_per_mtok: 2.00,
        tools: false,
        reasoning: false,
    },
    ZenModelInfo {
        sku: "zen3-tts-voice-design",
        hf_repo: Some("zenlm/zen-3-tts-1.7b-voicedesign"),
        context_window: 8_192,
        max_output_tokens: 0,
        modalities: &[ZenModality::AudioSpeech],
        input_price_per_mtok: 8.00,
        output_price_per_mtok: 8.00,
        tools: false,
        reasoning: false,
    },
    ZenModelInfo {
        sku: "zen3-tts-custom-voice",
        hf_repo: Some("zenlm/zen-3-tts-1.7b-customvoice"),
        context_window: 8_192,
        max_output_tokens: 0,
        modalities: &[ZenModality::AudioSpeech],
        input_price_per_mtok: 10.00,
        output_price_per_mtok: 10.00,
        tools: false,
        reasoning: false,
    },
    ZenModelInfo {
        sku: "zen3-nano",
        hf_repo: Some("zenlm/zen-3-nano"),
        context_window: 40_000,
        max_output_tokens: 8_192,
        modalities: &[ZenModality::Text],
        input_price_per_mtok: 0.60,
        output_price_per_mtok: 0.60,
        tools: true,
        reasoning: false,
    },
    ZenModelInfo {
        sku: "zen3-guard",
        hf_repo: Some("zenlm/zen-3-guard"),
        context_window: 8_192,
        max_output_tokens: 4_096,
        modalities: &[ZenModality::Text],
        input_price_per_mtok: 3.60,
        output_price_per_mtok: 3.60,
        tools: false,
        reasoning: false,
    },
    ZenModelInfo {
        sku: "zen3-embedding",
        hf_repo: Some("zenlm/zen-3-embedding"),
        context_window: 8_192,
        max_output_tokens: 0,
        modalities: &[ZenModality::Embedding],
        input_price_per_mtok: 0.39,
        output_price_per_mtok: 0.39,
        tools: false,
        reasoning: false,
    },
];

/// Look up a Zen SKU by canonical name.
///
/// Accepts the bare SKU (`"zen5"`) or any Ollama-style tag suffix
/// (`"zen5:latest"`, `"zen5:q4_k_m"`) — only the portion before `:` is
/// matched. Returns the first SKU whose `sku` string is a prefix of the
/// normalized input, preferring exact matches.
pub fn lookup_zen_model(model_type: &str) -> Option<&'static ZenModelInfo> {
    if !is_zen_model(model_type) {
        return None;
    }
    let normalized = model_type.split(':').next().unwrap_or(model_type);
    // Prefer exact match.
    if let Some(m) = ZEN_MODELS.iter().find(|m| m.sku.eq_ignore_ascii_case(normalized)) {
        return Some(m);
    }
    // Fall back to longest prefix match (so `zen5-nano-0.8B-foo` still
    // resolves to `zen5-nano-0.8B` if an operator adds a quant suffix).
    ZEN_MODELS
        .iter()
        .filter(|m| normalized.to_ascii_lowercase().starts_with(&m.sku.to_ascii_lowercase()))
        .max_by_key(|m| m.sku.len())
}

/// Cheap prefix check used by `model_capabilities_manager` to decide
/// whether to consult the Zen catalog before falling through to the
/// provider-generic heuristics.
#[inline]
pub fn is_zen_model(model_type: &str) -> bool {
    let lower = model_type.to_ascii_lowercase();
    lower.starts_with("zen5") || lower.starts_with("zen4") || lower.starts_with("zen3")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sku_has_zenlm_hf_repo() {
        for m in ZEN_MODELS {
            let repo = m.hf_repo.expect("every Zen SKU should declare its HF repo");
            assert!(
                repo.starts_with("zenlm/"),
                "SKU {} points at non-zenlm repo {}",
                m.sku,
                repo,
            );
        }
    }

    #[test]
    fn lookup_handles_ollama_tags() {
        assert_eq!(lookup_zen_model("zen5").map(|m| m.sku), Some("zen5"));
        assert_eq!(lookup_zen_model("zen5:latest").map(|m| m.sku), Some("zen5"));
        assert_eq!(lookup_zen_model("zen5-coder").map(|m| m.sku), Some("zen5-coder"));
        assert_eq!(
            lookup_zen_model("zen3-vl-235B-A22B").map(|m| m.sku),
            Some("zen3-vl-235B-A22B"),
        );
        assert!(lookup_zen_model("llama3").is_none());
    }

    #[test]
    fn sunset_skus_are_absent() {
        // Zen4 generation + virtual zen3 aliases sunset 2026-05-30.
        for sku in [
            "zen4", "zen4-pro", "zen4-max", "zen4.1", "zen4-mini", "zen4-ultra",
            "zen4-thinking", "zen4-coder", "zen4-coder-flash", "zen4-coder-pro",
            "zen5-ultra",
            "zen3-vl-reranker-2B", "zen3-vl-reranker-8B",
            "zen3-vl-embedding-2B", "zen3-vl-embedding-8B",
            "zen3-web-8B", "zen3-web-14B", "zen3-web-32B",
        ] {
            assert!(lookup_zen_model(sku).is_none(), "sunset SKU `{sku}` still in catalog");
        }
    }

    #[test]
    fn zen5_default_has_vision_and_reasoning() {
        let m = lookup_zen_model("zen5").unwrap();
        assert!(m.has_modality(ZenModality::Vision));
        assert!(m.reasoning);
        assert!(m.tools);
        assert_eq!(m.context_window, 256_000);
    }

    #[test]
    fn full_catalog_size_matches_gateway() {
        // Cross-checked against zen-gateway/gateway/config.yaml::model_list
        // after the 2026-05-30 SKU cleanup:
        //   Zen5 ladder: nano-{0.8B,2B,4B,9B} + flash + mini + zen5 + coder
        //                + pro + max = 10
        //   Zen5 embeddings: 0.6B/4B/8B = 3
        //   Zen4 generation: removed (sunset 2026-05-30)
        //   Zen3 specialty: omni, vl + 4 sizes, asr x3, tts x4, nano, guard,
        //                   embedding = 16
        // Total = 29. If this assertion fires, sync the catalog with the gateway.
        assert_eq!(ZEN_MODELS.len(), 29, "Zen SKU catalog drift vs gateway");
    }
}
