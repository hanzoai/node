//! User adapter management system
//! 
//! Manages personalized BitDelta adapters for each user

use std::sync::Arc;
use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use anyhow::{Result, anyhow};
use dashmap::DashMap;
use tokio::sync::RwLock;

use crate::{
    bitdelta::{BitDeltaAdapter, AdapterCache, CompressedAdapter},
    storage::HLLMStorage,
    routing::RoutingDecision,
    RoutingRequest,
};

/// User adapter with extended metadata
#[derive(Debug, Clone)]
pub struct UserAdapter {
    /// BitDelta adapter
    adapter: BitDeltaAdapter,
    
    /// User statistics
    statistics: UserStatistics,
    
    /// Performance history
    performance: PerformanceHistory,
}

impl UserAdapter {
    /// Create new user adapter
    pub fn new(user_id: String, dimension: usize, num_layers: usize) -> Self {
        Self {
            adapter: BitDeltaAdapter::new(user_id.clone(), dimension, num_layers),
            statistics: UserStatistics::new(user_id),
            performance: PerformanceHistory::new(),
        }
    }
    
    /// Get adapter parameters
    pub fn get_parameters(&self) -> Vec<f64> {
        // Convert scale factors to f64
        self.adapter.scale_factors
            .iter()
            .map(|s| s.to_f32() as f64)
            .collect()
    }
    
    /// Get the BitDelta adapter
    pub fn get_adapter(&self) -> &BitDeltaAdapter {
        &self.adapter
    }
    
    /// Update adapter with feedback
    pub fn update_with_feedback(
        &mut self,
        decision: &RoutingDecision,
        feedback: &Feedback,
    ) -> Result<()> {
        // Calculate gradients from feedback
        let gradients = self.calculate_gradients(decision, feedback)?;
        
        // Update BitDelta adapter
        let learning_rate = self.adaptive_learning_rate();
        self.adapter.update(&gradients, learning_rate)?;
        
        // Update statistics
        self.statistics.update(decision, feedback);
        
        // Update performance history
        self.performance.add_entry(decision.model.clone(), feedback.clone());
        
        Ok(())
    }
    
    /// Calculate gradients from feedback
    fn calculate_gradients(
        &self,
        decision: &RoutingDecision,
        feedback: &Feedback,
    ) -> Result<Vec<f32>> {
        let dimension = self.adapter.weight_deltas.len();
        let mut gradients = vec![0.0f32; dimension];
        
        // Performance gradient
        let performance_error = feedback.expected_quality - feedback.actual_quality;
        
        // Cost gradient
        let cost_error = feedback.expected_cost - feedback.actual_cost;
        
        // Latency gradient
        let latency_error = (feedback.expected_latency_ms as f64 - feedback.actual_latency_ms as f64) / 1000.0;
        
        // Combine errors into gradient signal
        let combined_error = performance_error * 0.5 + cost_error * 0.3 + latency_error * 0.2;
        
        // Apply to different dimensions based on error type
        for i in 0..dimension {
            let factor = ((i as f32 / dimension as f32) * std::f32::consts::PI).sin();
            gradients[i] = combined_error as f32 * factor * decision.confidence as f32;
        }
        
        Ok(gradients)
    }
    
    /// Calculate adaptive learning rate
    fn adaptive_learning_rate(&self) -> f32 {
        let base_lr = 0.01;
        let decay = 0.9999_f32.powi(self.statistics.total_requests as i32);
        
        // Adjust based on recent performance
        let performance_factor = if self.performance.recent_success_rate() > 0.8 {
            0.5 // Reduce learning rate if performing well
        } else if self.performance.recent_success_rate() < 0.5 {
            2.0 // Increase learning rate if performing poorly
        } else {
            1.0
        };
        
        base_lr * decay * performance_factor
    }
    
    /// Get similarity score with model
    pub fn model_affinity(&self, model_name: &str) -> f64 {
        self.performance.model_success_rate(model_name)
    }
}

/// Manager for user adapters
pub struct AdapterManager {
    /// Storage backend
    storage: Arc<HLLMStorage>,
    
    /// In-memory cache
    cache: AdapterCache,
    
    /// Active adapters
    active_adapters: Arc<DashMap<String, Arc<RwLock<UserAdapter>>>>,
    
    /// Quantization bits
    quantization_bits: usize,
}

impl AdapterManager {
    /// Create new adapter manager
    pub fn new(
        storage: Arc<HLLMStorage>,
        cache_size: usize,
        quantization_bits: usize,
    ) -> Self {
        Self {
            storage,
            cache: AdapterCache::new(cache_size),
            active_adapters: Arc::new(DashMap::new()),
            quantization_bits,
        }
    }
    
    /// Get or create adapter for user
    pub async fn get_or_create(&self, user_id: &str) -> Result<Arc<RwLock<UserAdapter>>> {
        // Check active adapters first
        if let Some(adapter) = self.active_adapters.get(user_id) {
            return Ok(Arc::clone(&*adapter));
        }
        
        // Check cache
        if let Some(cached) = self.cache.get(user_id) {
            let adapter = UserAdapter {
                adapter: cached.as_ref().clone(),
                statistics: self.load_statistics(user_id).await?,
                performance: self.load_performance(user_id).await?,
            };
            
            let adapter = Arc::new(RwLock::new(adapter));
            self.active_adapters.insert(user_id.to_string(), Arc::clone(&adapter));
            return Ok(adapter);
        }
        
        // Load from storage or create new
        let adapter = match self.storage.load_adapter(user_id).await {
            Ok(compressed) => {
                let bitdelta = BitDeltaAdapter::decompress(compressed)?;
                UserAdapter {
                    adapter: bitdelta,
                    statistics: self.load_statistics(user_id).await?,
                    performance: self.load_performance(user_id).await?,
                }
            },
            Err(_) => {
                // Create new adapter
                UserAdapter::new(
                    user_id.to_string(),
                    768, // Standard dimension
                    4,   // Default layers
                )
            }
        };
        
        let adapter = Arc::new(RwLock::new(adapter));
        self.active_adapters.insert(user_id.to_string(), Arc::clone(&adapter));
        
        Ok(adapter)
    }
    
    /// Update adapter based on routing decision
    pub async fn update_adapter(
        &self,
        user_id: &str,
        decision: &RoutingDecision,
        request: &RoutingRequest,
    ) -> Result<()> {
        let adapter = self.get_or_create(user_id).await?;
        
        // Record decision
        {
            let mut adapter_guard = adapter.write().await;
            adapter_guard.statistics.record_decision(decision);
            
            // Add regime to history
            adapter_guard.adapter.metadata.regime_history.push(
                format!("{:?}", decision.regime)
            );
            
            // Keep history bounded
            if adapter_guard.adapter.metadata.regime_history.len() > 100 {
                adapter_guard.adapter.metadata.regime_history.remove(0);
            }
        }
        
        // Save to storage periodically
        if self.should_persist(user_id).await {
            self.persist_adapter(user_id).await?;
        }
        
        Ok(())
    }
    
    /// Apply feedback to adapter
    pub async fn apply_feedback(
        &self,
        user_id: &str,
        decision: &RoutingDecision,
        feedback: Feedback,
    ) -> Result<()> {
        let adapter = self.get_or_create(user_id).await?;
        
        {
            let mut adapter_guard = adapter.write().await;
            adapter_guard.update_with_feedback(decision, &feedback)?;
        }
        
        // Always persist after feedback
        self.persist_adapter(user_id).await?;
        
        Ok(())
    }
    
    /// Load user statistics
    async fn load_statistics(&self, user_id: &str) -> Result<UserStatistics> {
        match self.storage.load_statistics(user_id).await {
            Ok(stats) => Ok(stats),
            Err(_) => Ok(UserStatistics::new(user_id.to_string())),
        }
    }
    
    /// Load performance history
    async fn load_performance(&self, user_id: &str) -> Result<PerformanceHistory> {
        match self.storage.load_performance(user_id).await {
            Ok(perf) => Ok(perf),
            Err(_) => Ok(PerformanceHistory::new()),
        }
    }
    
    /// Check if adapter should be persisted
    async fn should_persist(&self, user_id: &str) -> bool {
        if let Some(adapter) = self.active_adapters.get(user_id) {
            let adapter_guard = adapter.read().await;
            // Persist every 10 updates
            adapter_guard.adapter.metadata.update_count % 10 == 0
        } else {
            false
        }
    }
    
    /// Persist adapter to storage
    async fn persist_adapter(&self, user_id: &str) -> Result<()> {
        if let Some(adapter) = self.active_adapters.get(user_id) {
            let adapter_guard = adapter.read().await;
            
            // Save compressed adapter
            let compressed = adapter_guard.adapter.compress()?;
            self.storage.save_adapter(user_id, compressed).await?;
            
            // Save statistics
            self.storage.save_statistics(user_id, &adapter_guard.statistics).await?;
            
            // Save performance
            self.storage.save_performance(user_id, &adapter_guard.performance).await?;
            
            // Update cache
            self.cache.insert(adapter_guard.adapter.clone());
        }
        
        Ok(())
    }
    
    /// Get cache statistics
    pub async fn get_cache_stats(&self) -> Result<CacheStatistics> {
        let cache_stats = self.cache.stats();
        
        Ok(CacheStatistics {
            active_count: self.active_adapters.len(),
            cached_count: cache_stats.size,
            total_requests: cache_stats.total_accesses,
            hit_rate: cache_stats.hit_rate,
        })
    }
    
    /// Cleanup inactive adapters
    pub async fn cleanup_inactive(&self, inactive_threshold_secs: u64) -> Result<usize> {
        let now = chrono::Utc::now().timestamp() as u64;
        let mut removed = 0;
        
        let mut to_remove = Vec::new();
        
        for entry in self.active_adapters.iter() {
            let adapter = entry.value().read().await;
            if now - adapter.adapter.metadata.last_updated > inactive_threshold_secs {
                to_remove.push(entry.key().clone());
            }
        }
        
        for key in to_remove {
            // Persist before removing
            self.persist_adapter(&key).await?;
            self.active_adapters.remove(&key);
            removed += 1;
        }
        
        Ok(removed)
    }
}

/// User statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserStatistics {
    pub user_id: String,
    pub total_requests: u64,
    pub successful_requests: u64,
    pub total_tokens: u64,
    pub total_cost: f64,
    pub avg_latency_ms: f64,
    pub model_usage: HashMap<String, u64>,
    pub regime_distribution: HashMap<String, u64>,
}

impl UserStatistics {
    fn new(user_id: String) -> Self {
        Self {
            user_id,
            total_requests: 0,
            successful_requests: 0,
            total_tokens: 0,
            total_cost: 0.0,
            avg_latency_ms: 0.0,
            model_usage: HashMap::new(),
            regime_distribution: HashMap::new(),
        }
    }
    
    fn record_decision(&mut self, decision: &RoutingDecision) {
        self.total_requests += 1;
        
        *self.model_usage.entry(decision.model.clone()).or_insert(0) += 1;
        *self.regime_distribution.entry(format!("{:?}", decision.regime)).or_insert(0) += 1;
    }
    
    fn update(&mut self, decision: &RoutingDecision, feedback: &Feedback) {
        if feedback.success {
            self.successful_requests += 1;
        }
        
        self.total_tokens += feedback.tokens_used;
        self.total_cost += feedback.actual_cost;
        
        // Update rolling average latency
        let alpha = 0.1; // Exponential smoothing factor
        self.avg_latency_ms = self.avg_latency_ms * (1.0 - alpha) 
            + feedback.actual_latency_ms as f64 * alpha;
    }
}

/// Performance history tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceHistory {
    entries: Vec<PerformanceEntry>,
    max_entries: usize,
}

impl PerformanceHistory {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            max_entries: 1000,
        }
    }
    
    fn add_entry(&mut self, model: String, feedback: Feedback) {
        self.entries.push(PerformanceEntry {
            timestamp: chrono::Utc::now().timestamp() as u64,
            model,
            success: feedback.success,
            quality: feedback.actual_quality,
            cost: feedback.actual_cost,
            latency_ms: feedback.actual_latency_ms,
        });
        
        // Keep bounded
        if self.entries.len() > self.max_entries {
            self.entries.remove(0);
        }
    }
    
    fn recent_success_rate(&self) -> f64 {
        let recent = self.entries.iter().rev().take(20);
        let total = recent.clone().count();
        
        if total == 0 {
            return 0.5; // Default
        }
        
        let successful = recent.filter(|e| e.success).count();
        successful as f64 / total as f64
    }
    
    fn model_success_rate(&self, model: &str) -> f64 {
        let model_entries: Vec<_> = self.entries.iter()
            .filter(|e| e.model == model)
            .collect();
        
        if model_entries.is_empty() {
            return 0.5; // Default
        }
        
        let successful = model_entries.iter().filter(|e| e.success).count();
        successful as f64 / model_entries.len() as f64
    }
}

/// Performance entry
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PerformanceEntry {
    timestamp: u64,
    model: String,
    success: bool,
    quality: f64,
    cost: f64,
    latency_ms: u64,
}

/// Feedback from model execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feedback {
    pub success: bool,
    pub expected_quality: f64,
    pub actual_quality: f64,
    pub expected_cost: f64,
    pub actual_cost: f64,
    pub expected_latency_ms: u64,
    pub actual_latency_ms: u64,
    pub tokens_used: u64,
    pub error_message: Option<String>,
}

/// Cache statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStatistics {
    pub active_count: usize,
    pub cached_count: usize,
    pub total_requests: u64,
    pub hit_rate: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_user_adapter_creation() {
        let adapter = UserAdapter::new("test_user".into(), 100, 4);
        assert_eq!(adapter.statistics.user_id, "test_user");
        assert_eq!(adapter.performance.entries.len(), 0);
    }
    
    #[test]
    fn test_adaptive_learning_rate() {
        let mut adapter = UserAdapter::new("test_user".into(), 100, 4);
        
        // Initial learning rate
        let lr1 = adapter.adaptive_learning_rate();
        
        // After some requests
        adapter.statistics.total_requests = 1000;
        let lr2 = adapter.adaptive_learning_rate();
        
        assert!(lr2 < lr1); // Should decay
    }
}