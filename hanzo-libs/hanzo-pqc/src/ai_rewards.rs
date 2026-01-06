//! AI Reward Distribution for Lux Network
//!
//! This module implements the AI reward mining model per LP-5610 Section 7:
//!
//! - 10% of block rewards go to AI Compute Pool
//! - 90% of block rewards go to traditional validators
//! - AI providers earn rewards for availability (random mining)
//! - Rewards scaled by CC tier, modeling level, and trust score
//!
//! Modeling Levels (complexity of AI work):
//!   Level 1 — "Inference-Light": Embeddings, small models (<7B)
//!   Level 2 — "Inference-Standard": Medium models (7B-70B), chat
//!   Level 3 — "Inference-Heavy": Large models (70B+), multimodal
//!   Level 4 — "Training": Fine-tuning, RLHF, distributed training
//!   Level 5 — "Specialized": PQ crypto, ZK proofs, custom compute

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::cc_tier::{CCTier, TierAttestation};

/// Percentage of block rewards allocated to AI compute (10%)
pub const AI_REWARD_POOL_SHARE: f64 = 0.10;

/// Percentage for traditional validators (90%)
pub const VALIDATOR_REWARD_SHARE: f64 = 0.90;

/// Modeling level represents the complexity tier of AI workloads
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum ModelingLevel {
    /// Embeddings and small models (<7B params)
    /// Examples: text-embedding-3, Phi-3-mini, Qwen3-0.6B
    InferenceLight = 1,

    /// Medium models (7B-70B params)
    /// Examples: Llama-3-8B, Qwen3-14B, Mistral-7B
    InferenceStandard = 2,

    /// Large models (70B+ params)
    /// Examples: Llama-3-70B, Qwen3-72B, multimodal models
    InferenceHeavy = 3,

    /// Fine-tuning and training workloads
    /// Examples: LoRA, QLoRA, full fine-tuning, RLHF
    Training = 4,

    /// Specialized compute
    /// Examples: PQ crypto operations, ZK proof generation, custom kernels
    Specialized = 5,
}

impl ModelingLevel {
    /// Returns human-readable name
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InferenceLight => "Inference-Light",
            Self::InferenceStandard => "Inference-Standard",
            Self::InferenceHeavy => "Inference-Heavy",
            Self::Training => "Training",
            Self::Specialized => "Specialized",
        }
    }

    /// Base reward multiplier for this level
    /// Higher complexity = higher rewards
    pub fn base_reward_multiplier(&self) -> f64 {
        match self {
            Self::InferenceLight => 0.5,
            Self::InferenceStandard => 1.0,
            Self::InferenceHeavy => 1.5,
            Self::Training => 2.0,
            Self::Specialized => 2.5,
        }
    }

    /// Minimum VRAM required (GB)
    pub fn min_vram_gb(&self) -> u64 {
        match self {
            Self::InferenceLight => 8,    // 8GB for small models
            Self::InferenceStandard => 24, // 24GB for 7B-13B models
            Self::InferenceHeavy => 80,   // 80GB for 70B+ models
            Self::Training => 48,         // 48GB minimum for training
            Self::Specialized => 16,      // Varies, 16GB baseline
        }
    }

    /// Try to parse from u8
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::InferenceLight),
            2 => Some(Self::InferenceStandard),
            3 => Some(Self::InferenceHeavy),
            4 => Some(Self::Training),
            5 => Some(Self::Specialized),
            _ => None,
        }
    }
}

impl std::fmt::Display for ModelingLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// AI compute provider in the reward pool
#[derive(Debug, Clone)]
pub struct AIProvider {
    /// Unique identifier
    pub provider_id: String,

    /// Current CC tier attestation
    pub attestation: Option<TierAttestation>,

    /// Highest modeling level supported
    pub max_modeling_level: ModelingLevel,

    /// Current active workload level
    pub current_modeling_level: Option<ModelingLevel>,

    /// Staked amount in LUX tokens
    pub stake_lux: u64,

    /// Last heartbeat timestamp
    pub last_heartbeat: SystemTime,

    /// Consecutive epochs online
    pub consecutive_epochs: u64,

    /// Tasks completed in current epoch
    pub tasks_this_epoch: u64,

    /// Lifetime tasks completed
    pub total_tasks_completed: u64,

    /// Historical reputation (0.0-1.0)
    pub reputation_score: f64,
}

impl AIProvider {
    /// Create new provider
    pub fn new(provider_id: String, stake_lux: u64, max_level: ModelingLevel) -> Self {
        Self {
            provider_id,
            attestation: None,
            max_modeling_level: max_level,
            current_modeling_level: None,
            stake_lux,
            last_heartbeat: SystemTime::now(),
            consecutive_epochs: 0,
            tasks_this_epoch: 0,
            total_tasks_completed: 0,
            reputation_score: 0.5, // Start neutral
        }
    }

    /// Check if provider is online
    pub fn is_online(&self, max_heartbeat_age: Duration) -> bool {
        match self.last_heartbeat.elapsed() {
            Ok(elapsed) => elapsed < max_heartbeat_age,
            Err(_) => false,
        }
    }

    /// Get effective CC tier (Tier4 if no valid attestation)
    pub fn effective_tier(&self) -> CCTier {
        self.attestation
            .as_ref()
            .filter(|a| a.is_valid())
            .map(|a| a.tier)
            .unwrap_or(CCTier::Tier4Standard)
    }

    /// Calculate provider's weight in reward pool
    /// Weight = TierMult * ModelingMult * StakeWeight * UptimeBonus * RepBonus
    pub fn reward_weight(&self) -> f64 {
        let tier = self.effective_tier();

        // Base tier multiplier (1.5x for Tier1, down to 0.5x for Tier4)
        let tier_mult = tier.reward_multiplier();

        // Modeling level multiplier
        let model_mult = self.max_modeling_level.base_reward_multiplier();

        // Stake weight (logarithmic to prevent plutocracy)
        // sqrt(stake / 1000) capped at 10x
        let stake_weight = if self.stake_lux > 1000 {
            ((self.stake_lux as f64 / 1000.0).sqrt()).min(10.0)
        } else {
            1.0
        };

        // Uptime bonus (up to 1.5x for long-term providers)
        let uptime_bonus = 1.0 + (self.consecutive_epochs as f64 / 1000.0).min(0.5);

        // Reputation bonus (0.8x to 1.2x based on history)
        let rep_bonus = 0.8 + (self.reputation_score * 0.4);

        tier_mult * model_mult * stake_weight * uptime_bonus * rep_bonus
    }
}

/// Result of participation reward calculation
#[derive(Debug, Clone)]
pub struct ParticipationRewardResult {
    /// Provider receiving reward
    pub provider_id: String,

    /// Reward amount in LUX (wei)
    pub reward_lux_wei: u128,

    /// Provider's calculated weight
    pub weight: f64,

    /// Provider's share of total weight
    pub weight_share: f64,

    /// Provider's CC tier
    pub tier: CCTier,

    /// Provider's max modeling level
    pub modeling_level: ModelingLevel,
}

/// Result of task completion reward
#[derive(Debug, Clone)]
pub struct TaskRewardResult {
    /// Provider receiving reward
    pub provider_id: String,

    /// Task identifier
    pub task_id: String,

    /// Reward amount in LUX (wei)
    pub reward_lux_wei: u128,

    /// Task's modeling level
    pub modeling_level: ModelingLevel,

    /// Compute units consumed
    pub compute_units: u64,
}

/// AI Reward Pool manages distribution
pub struct AIRewardPool {
    /// Registered providers
    pub providers: HashMap<String, AIProvider>,

    /// Current epoch number
    pub epoch_number: u64,

    /// Epoch duration
    pub epoch_duration: Duration,

    /// Total LUX in AI pool for this epoch (wei)
    pub total_pool_lux_wei: u128,

    /// % of AI pool for random availability rewards (default: 30%)
    pub participation_share: f64,

    /// % of AI pool for task completion rewards (default: 70%)
    pub task_share: f64,
}

impl AIRewardPool {
    /// Create new reward pool
    pub fn new(epoch_duration: Duration) -> Self {
        Self {
            providers: HashMap::new(),
            epoch_number: 0,
            epoch_duration,
            total_pool_lux_wei: 0,
            participation_share: 0.30, // 30% for availability
            task_share: 0.70,          // 70% for tasks
        }
    }

    /// Register a provider
    pub fn register_provider(&mut self, provider: AIProvider) -> Result<(), &'static str> {
        if provider.provider_id.is_empty() {
            return Err("provider ID required");
        }

        let min_stake = CCTier::Tier4Standard.min_stake_lux();
        if provider.stake_lux < min_stake {
            return Err("insufficient stake");
        }

        self.providers.insert(provider.provider_id.clone(), provider);
        Ok(())
    }

    /// Calculate participation (random mining) rewards
    pub fn calculate_participation_rewards(
        &self,
        max_heartbeat_age: Duration,
    ) -> Vec<ParticipationRewardResult> {
        // Get participation pool amount
        let participation_pool = (self.total_pool_lux_wei as f64 * self.participation_share) as u128;

        // Calculate total weight of online providers
        let mut total_weight = 0.0;
        let mut online_providers: Vec<&AIProvider> = Vec::new();

        for provider in self.providers.values() {
            if !provider.is_online(max_heartbeat_age) {
                continue;
            }
            if provider.attestation.as_ref().map(|a| a.is_valid()).unwrap_or(false) {
                let weight = provider.reward_weight();
                total_weight += weight;
                online_providers.push(provider);
            }
        }

        if total_weight == 0.0 || online_providers.is_empty() {
            return Vec::new();
        }

        // Distribute rewards proportionally
        let mut results = Vec::with_capacity(online_providers.len());

        for provider in online_providers {
            let weight = provider.reward_weight();
            let share = weight / total_weight;
            let reward = (participation_pool as f64 * share) as u128;

            results.push(ParticipationRewardResult {
                provider_id: provider.provider_id.clone(),
                reward_lux_wei: reward,
                weight,
                weight_share: share,
                tier: provider.effective_tier(),
                modeling_level: provider.max_modeling_level,
            });
        }

        results
    }

    /// Calculate reward for completed task
    pub fn calculate_task_reward(
        &self,
        provider: &AIProvider,
        task_id: String,
        modeling_level: ModelingLevel,
        compute_units: u64,
    ) -> TaskRewardResult {
        // Base rate per compute unit (in wei)
        // 1 compute unit = 1 GPU-second at Tier 2 / Level 2
        let base_rate_wei: u128 = 1_000_000_000_000; // 0.000001 LUX

        // Calculate reward
        let mut reward = base_rate_wei * compute_units as u128;

        // Apply tier multiplier
        let tier_mult = provider.effective_tier().reward_multiplier();
        reward = (reward as f64 * tier_mult) as u128;

        // Apply modeling level multiplier
        let level_mult = modeling_level.base_reward_multiplier();
        reward = (reward as f64 * level_mult) as u128;

        TaskRewardResult {
            provider_id: provider.provider_id.clone(),
            task_id,
            reward_lux_wei: reward,
            modeling_level,
            compute_units,
        }
    }
}

/// Split block reward between validators and AI pool
pub fn calculate_block_reward_split(total_block_reward_wei: u128) -> (u128, u128) {
    // 90% to validators
    let validator_reward = (total_block_reward_wei as f64 * VALIDATOR_REWARD_SHARE) as u128;

    // 10% to AI pool
    let ai_pool_reward = total_block_reward_wei - validator_reward;

    (validator_reward, ai_pool_reward)
}

/// Check if provider is eligible for random mining rewards
pub fn random_mining_eligibility(
    provider: &AIProvider,
    max_heartbeat_age: Duration,
) -> Result<(), &'static str> {
    if !provider.is_online(max_heartbeat_age) {
        return Err("provider offline");
    }

    let attestation = provider.attestation.as_ref().ok_or("no attestation")?;

    if !attestation.is_valid() {
        return Err("attestation expired");
    }

    let min_stake = provider.effective_tier().min_stake_lux();
    if provider.stake_lux < min_stake {
        return Err("insufficient stake");
    }

    Ok(())
}

/// Epoch reward summary
#[derive(Debug)]
pub struct EpochRewardSummary {
    pub epoch_number: u64,
    pub total_block_rewards_wei: u128,
    pub validator_rewards_wei: u128,
    pub ai_pool_rewards_wei: u128,
    pub participation_rewards_wei: u128,
    pub task_rewards_wei: u128,
    pub online_providers: u64,
    pub total_providers: u64,
    pub tier_distribution: HashMap<CCTier, u64>,
}

impl AIRewardPool {
    /// Calculate full epoch reward distribution
    pub fn calculate_epoch_rewards(
        &mut self,
        total_block_rewards_wei: u128,
        max_heartbeat_age: Duration,
    ) -> EpochRewardSummary {
        let (validator_rewards, ai_pool_rewards) =
            calculate_block_reward_split(total_block_rewards_wei);

        // Update pool total
        self.total_pool_lux_wei = ai_pool_rewards;

        // Calculate pool splits
        let participation_pool = (ai_pool_rewards as f64 * self.participation_share) as u128;
        let task_pool = ai_pool_rewards - participation_pool;

        // Count tiers and online providers
        let mut tier_dist: HashMap<CCTier, u64> = HashMap::new();
        let mut online_count = 0u64;

        for provider in self.providers.values() {
            if provider.is_online(max_heartbeat_age) {
                online_count += 1;
                let tier = provider.effective_tier();
                *tier_dist.entry(tier).or_insert(0) += 1;
            }
        }

        EpochRewardSummary {
            epoch_number: self.epoch_number,
            total_block_rewards_wei,
            validator_rewards_wei: validator_rewards,
            ai_pool_rewards_wei: ai_pool_rewards,
            participation_rewards_wei: participation_pool,
            task_rewards_wei: task_pool,
            online_providers: online_count,
            total_providers: self.providers.len() as u64,
            tier_distribution: tier_dist,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_modeling_level_multipliers() {
        assert!(ModelingLevel::InferenceLight.base_reward_multiplier() <
                ModelingLevel::InferenceStandard.base_reward_multiplier());
        assert!(ModelingLevel::InferenceStandard.base_reward_multiplier() <
                ModelingLevel::InferenceHeavy.base_reward_multiplier());
        assert!(ModelingLevel::InferenceHeavy.base_reward_multiplier() <
                ModelingLevel::Training.base_reward_multiplier());
        assert!(ModelingLevel::Training.base_reward_multiplier() <
                ModelingLevel::Specialized.base_reward_multiplier());
    }

    #[test]
    fn test_block_reward_split() {
        // 100 LUX (in wei)
        let total = 100_000_000_000_000_000_000u128; // 100 * 1e18

        let (validator, ai_pool) = calculate_block_reward_split(total);

        // Validator should get ~90 LUX
        let expected_validator = 90_000_000_000_000_000_000u128;
        assert_eq!(validator, expected_validator);

        // AI pool should get ~10 LUX
        let expected_ai = 10_000_000_000_000_000_000u128;
        assert_eq!(ai_pool, expected_ai);

        // Total should equal original
        assert_eq!(validator + ai_pool, total);
    }

    #[test]
    fn test_provider_reward_weight() {
        let mut provider = AIProvider::new(
            "test".to_string(),
            100_000,
            ModelingLevel::InferenceHeavy,
        );

        provider.attestation = Some(TierAttestation::new_valid(
            CCTier::Tier1GpuNativeCC,
            Duration::from_secs(3600 * 5), // 5 hours
        ));
        provider.reputation_score = 0.9;
        provider.consecutive_epochs = 500;

        let weight = provider.reward_weight();

        // Tier1 (1.5) * Level3 (1.5) * stake_sqrt (10) * uptime (1.5) * rep (1.16) = ~39
        assert!(weight > 10.0, "Expected weight > 10, got {}", weight);
        assert!(weight < 50.0, "Expected weight < 50, got {}", weight);
    }

    #[test]
    fn test_pool_registration() {
        let mut pool = AIRewardPool::new(Duration::from_secs(3600));

        let provider = AIProvider::new(
            "provider-1".to_string(),
            50_000,
            ModelingLevel::InferenceStandard,
        );

        assert!(pool.register_provider(provider).is_ok());
        assert!(pool.providers.contains_key("provider-1"));

        // Low stake should fail
        let low_stake = AIProvider::new(
            "low-stake".to_string(),
            100, // Below minimum
            ModelingLevel::InferenceLight,
        );

        assert!(pool.register_provider(low_stake).is_err());
    }

    #[test]
    fn test_random_mining_eligibility() {
        let mut provider = AIProvider::new(
            "test".to_string(),
            50_000,
            ModelingLevel::InferenceStandard,
        );

        // No attestation - should fail
        assert!(random_mining_eligibility(&provider, Duration::from_secs(300)).is_err());

        // Add valid attestation
        provider.attestation = Some(TierAttestation::new_valid(
            CCTier::Tier2ConfidentialVM,
            Duration::from_secs(3600 * 23),
        ));

        // Should pass now
        assert!(random_mining_eligibility(&provider, Duration::from_secs(300)).is_ok());

        // Simulate going offline
        provider.last_heartbeat = SystemTime::now() - Duration::from_secs(600);
        assert!(random_mining_eligibility(&provider, Duration::from_secs(300)).is_err());
    }
}
