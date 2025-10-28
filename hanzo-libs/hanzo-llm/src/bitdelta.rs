//! BitDelta 1-bit quantization for efficient adapter storage
//! 
//! Implements extreme quantization for personalized model adaptation

use std::collections::HashMap;
use std::sync::Arc;
use serde::{Serialize, Deserialize};
use anyhow::{Result, anyhow};
use bitvec::prelude::*;
use bitvec::order::Lsb0;
use half::f16;
use dashmap::DashMap;

/// BitDelta adapter for 1-bit weight updates
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BitDeltaAdapter {
    /// User identifier
    pub user_id: String,
    
    /// 1-bit weight deltas
    pub weight_deltas: BitVec<u8, Lsb0>,
    
    /// Scale factors for each layer
    pub scale_factors: Vec<f16>,
    
    /// Bias terms (kept at higher precision)
    pub biases: Vec<f16>,
    
    /// Adapter metadata
    pub metadata: AdapterMetadata,
}

impl BitDeltaAdapter {
    /// Create new adapter for user
    pub fn new(user_id: String, dimension: usize, num_layers: usize) -> Self {
        Self {
            user_id,
            weight_deltas: bitvec![u8, Lsb0; 0; dimension],
            scale_factors: vec![f16::from_f32(1.0); num_layers],
            biases: vec![f16::ZERO; num_layers],
            metadata: AdapterMetadata::new(),
        }
    }
    
    /// Apply adapter to base weights
    pub fn apply(&self, base_weights: &[f32]) -> Result<Vec<f32>> {
        if base_weights.len() != self.weight_deltas.len() {
            return Err(anyhow!("Weight dimension mismatch"));
        }
        
        let mut adapted = Vec::with_capacity(base_weights.len());
        let scale = self.get_average_scale();
        
        for (i, &base) in base_weights.iter().enumerate() {
            let delta = if self.weight_deltas[i] {
                scale // Positive delta
            } else {
                -scale // Negative delta
            };
            
            adapted.push(base + delta);
        }
        
        Ok(adapted)
    }
    
    /// Update adapter with gradient information
    pub fn update(&mut self, gradients: &[f32], learning_rate: f32) -> Result<()> {
        if gradients.len() != self.weight_deltas.len() {
            return Err(anyhow!("Gradient dimension mismatch"));
        }
        
        // Compute threshold for quantization
        let abs_grads: Vec<f32> = gradients.iter().map(|g| g.abs()).collect();
        let threshold = Self::compute_threshold(&abs_grads);
        
        // Update 1-bit deltas
        for (i, &grad) in gradients.iter().enumerate() {
            if grad.abs() > threshold {
                // Flip bit if gradient is significant
                let new_bit = grad > 0.0;
                self.weight_deltas.set(i, new_bit);
            }
        }
        
        // Update scale factors
        self.update_scales(&abs_grads, learning_rate);
        
        // Update metadata
        self.metadata.update_count += 1;
        self.metadata.last_updated = chrono::Utc::now().timestamp() as u64;
        
        Ok(())
    }
    
    /// Compute threshold for bit quantization
    fn compute_threshold(gradients: &[f32]) -> f32 {
        let mut sorted = gradients.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        
        // Use 90th percentile as threshold
        let idx = (sorted.len() as f32 * 0.9) as usize;
        sorted[idx.min(sorted.len() - 1)]
    }
    
    /// Update scale factors based on gradient magnitudes
    fn update_scales(&mut self, grad_magnitudes: &[f32], learning_rate: f32) {
        let mean_magnitude: f32 = grad_magnitudes.iter().sum::<f32>() / grad_magnitudes.len() as f32;
        
        for scale in &mut self.scale_factors {
            let current = scale.to_f32();
            let new_scale = current * (1.0 + learning_rate * mean_magnitude);
            *scale = f16::from_f32(new_scale.clamp(0.01, 10.0));
        }
    }
    
    /// Get average scale factor
    fn get_average_scale(&self) -> f32 {
        let sum: f32 = self.scale_factors.iter().map(|s| s.to_f32()).sum();
        sum / self.scale_factors.len() as f32
    }
    
    /// Compress adapter for storage
    pub fn compress(&self) -> Result<CompressedAdapter> {
        // Convert BitVec to Vec<u8> for storage
        let bytes: Vec<u8> = self.weight_deltas.clone().into_vec();
        
        Ok(CompressedAdapter {
            user_id: self.user_id.clone(),
            weight_bits: bytes,
            scales: self.scale_factors.iter().map(|s| s.to_bits()).collect(),
            biases: self.biases.iter().map(|b| b.to_bits()).collect(),
            metadata: self.metadata.clone(),
        })
    }
    
    /// Decompress adapter from storage
    pub fn decompress(compressed: CompressedAdapter) -> Result<Self> {
        let weight_deltas = BitVec::from_vec(compressed.weight_bits);
        
        let scale_factors = compressed.scales
            .into_iter()
            .map(f16::from_bits)
            .collect();
        
        let biases = compressed.biases
            .into_iter()
            .map(f16::from_bits)
            .collect();
        
        Ok(Self {
            user_id: compressed.user_id,
            weight_deltas,
            scale_factors,
            biases,
            metadata: compressed.metadata,
        })
    }
    
    /// Merge with another adapter
    pub fn merge(&mut self, other: &BitDeltaAdapter, weight: f32) -> Result<()> {
        if self.weight_deltas.len() != other.weight_deltas.len() {
            return Err(anyhow!("Cannot merge adapters of different sizes"));
        }
        
        // Weighted bit voting
        for i in 0..self.weight_deltas.len() {
            if weight > 0.5 {
                // Other adapter dominates
                self.weight_deltas.set(i, other.weight_deltas[i]);
            }
            // Otherwise keep current bit
        }
        
        // Weighted average of scales
        for (i, scale) in self.scale_factors.iter_mut().enumerate() {
            if i < other.scale_factors.len() {
                let current = scale.to_f32();
                let other_scale = other.scale_factors[i].to_f32();
                *scale = f16::from_f32(current * (1.0 - weight) + other_scale * weight);
            }
        }
        
        Ok(())
    }
    
    /// Calculate similarity with another adapter
    pub fn similarity(&self, other: &BitDeltaAdapter) -> f32 {
        if self.weight_deltas.len() != other.weight_deltas.len() {
            return 0.0;
        }
        
        let matching_bits = self.weight_deltas
            .iter()
            .zip(other.weight_deltas.iter())
            .filter(|(a, b)| a == b)
            .count();
        
        matching_bits as f32 / self.weight_deltas.len() as f32
    }
}

/// Compressed adapter for storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressedAdapter {
    pub user_id: String,
    pub weight_bits: Vec<u8>,
    pub scales: Vec<u16>,
    pub biases: Vec<u16>,
    pub metadata: AdapterMetadata,
}

/// Adapter metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterMetadata {
    pub created_at: u64,
    pub last_updated: u64,
    pub update_count: u64,
    pub total_tokens: u64,
    pub regime_history: Vec<String>,
}

impl AdapterMetadata {
    fn new() -> Self {
        let now = chrono::Utc::now().timestamp() as u64;
        Self {
            created_at: now,
            last_updated: now,
            update_count: 0,
            total_tokens: 0,
            regime_history: Vec::new(),
        }
    }
}

/// Bit quantizer for weight compression
pub struct BitQuantizer {
    /// Quantization threshold
    threshold: f32,
    
    /// Adaptive threshold adjustment
    adaptive: bool,
}

impl BitQuantizer {
    /// Create new quantizer
    pub fn new(threshold: f32, adaptive: bool) -> Self {
        Self { threshold, adaptive }
    }
    
    /// Quantize float weights to bits
    pub fn quantize(&self, weights: &[f32]) -> (BitVec<u8, Lsb0>, f32) {
        let mean = weights.iter().sum::<f32>() / weights.len() as f32;
        let threshold = if self.adaptive {
            self.compute_adaptive_threshold(weights)
        } else {
            self.threshold
        };
        
        let mut bits = BitVec::<u8, Lsb0>::with_capacity(weights.len());
        
        for &w in weights {
            bits.push(w - mean > threshold);
        }
        
        (bits, mean)
    }
    
    /// Dequantize bits back to floats
    pub fn dequantize(&self, bits: &BitVec<u8, Lsb0>, mean: f32, scale: f32) -> Vec<f32> {
        bits.iter()
            .map(|bit| {
                if *bit {
                    mean + scale
                } else {
                    mean - scale
                }
            })
            .collect()
    }
    
    /// Compute adaptive threshold based on weight distribution
    fn compute_adaptive_threshold(&self, weights: &[f32]) -> f32 {
        let mean = weights.iter().sum::<f32>() / weights.len() as f32;
        let variance = weights.iter()
            .map(|w| (w - mean).powi(2))
            .sum::<f32>() / weights.len() as f32;
        
        variance.sqrt() * 0.5 // Use half standard deviation
    }
}

/// Cache for frequently used adapters
pub struct AdapterCache {
    cache: Arc<DashMap<String, Arc<BitDeltaAdapter>>>,
    max_size: usize,
    access_counts: Arc<DashMap<String, u64>>,
}

impl AdapterCache {
    /// Create new cache
    pub fn new(max_size: usize) -> Self {
        Self {
            cache: Arc::new(DashMap::new()),
            max_size,
            access_counts: Arc::new(DashMap::new()),
        }
    }
    
    /// Get adapter from cache
    pub fn get(&self, user_id: &str) -> Option<Arc<BitDeltaAdapter>> {
        self.access_counts.entry(user_id.to_string())
            .and_modify(|count| *count += 1)
            .or_insert(1);
        
        self.cache.get(user_id).map(|entry| Arc::clone(&*entry))
    }
    
    /// Insert adapter into cache
    pub fn insert(&self, adapter: BitDeltaAdapter) {
        let user_id = adapter.user_id.clone();
        
        // Evict least recently used if at capacity
        if self.cache.len() >= self.max_size {
            self.evict_lru();
        }
        
        self.cache.insert(user_id.clone(), Arc::new(adapter));
        self.access_counts.insert(user_id, 0);
    }
    
    /// Evict least recently used adapter
    fn evict_lru(&self) {
        if let Some(min_entry) = self.access_counts
            .iter()
            .min_by_key(|entry| *entry.value())
        {
            let key = min_entry.key().clone();
            self.cache.remove(&key);
            self.access_counts.remove(&key);
        }
    }
    
    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        let total_accesses = self.access_counts
            .iter()
            .map(|entry| *entry.value())
            .sum();
        
        CacheStats {
            size: self.cache.len(),
            max_size: self.max_size,
            total_accesses,
            hit_rate: 0.0, // Would need to track hits/misses
        }
    }
    
    /// Clear cache
    pub fn clear(&self) {
        self.cache.clear();
        self.access_counts.clear();
    }
}

/// Cache statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStats {
    pub size: usize,
    pub max_size: usize,
    pub total_accesses: u64,
    pub hit_rate: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_bitdelta_adapter() {
        let mut adapter = BitDeltaAdapter::new("user1".to_string(), 100, 4);
        let base_weights = vec![0.5; 100];
        
        // Apply adapter
        let adapted = adapter.apply(&base_weights).unwrap();
        assert_eq!(adapted.len(), base_weights.len());
        
        // Update with gradients
        let gradients = vec![0.01; 100];
        adapter.update(&gradients, 0.1).unwrap();
        
        assert_eq!(adapter.metadata.update_count, 1);
    }
    
    #[test]
    fn test_bit_quantizer() {
        let quantizer = BitQuantizer::new(0.1, false);
        let weights = vec![0.1, 0.2, -0.1, -0.2, 0.3];
        
        let (bits, mean) = quantizer.quantize(&weights);
        assert_eq!(bits.len(), weights.len());
        
        let dequantized = quantizer.dequantize(&bits, mean, 0.15);
        assert_eq!(dequantized.len(), weights.len());
    }
    
    #[test]
    fn test_adapter_cache() {
        let cache = AdapterCache::new(2);
        
        let adapter1 = BitDeltaAdapter::new("user1".to_string(), 10, 2);
        let adapter2 = BitDeltaAdapter::new("user2".to_string(), 10, 2);
        let adapter3 = BitDeltaAdapter::new("user3".to_string(), 10, 2);
        
        cache.insert(adapter1);
        cache.insert(adapter2);
        
        assert!(cache.get("user1").is_some());
        
        // Should evict when inserting third
        cache.insert(adapter3);
        assert_eq!(cache.stats().size, 2);
    }
}