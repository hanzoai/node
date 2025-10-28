// ATTESTATION CACHE - REMEMBER THE MATRIX
// Cache attestation results to avoid repeated expensive operations

use hanzo_kbs::attestation::AttestationResult;
use hanzo_kbs::types::PrivacyTier;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use chrono::{DateTime, Duration, Utc};

/// Cache for attestation results
pub struct AttestationCache {
    entries: Arc<RwLock<HashMap<CacheKey, CacheEntry>>>,
    max_entries: usize,
    default_ttl: Duration,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct CacheKey {
    tier: PrivacyTier,
    platform: String,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    result: AttestationResult,
    created_at: DateTime<Utc>,
    last_accessed: DateTime<Utc>,
    access_count: u64,
}

impl AttestationCache {
    pub fn new() -> Self {
        Self::with_config(100, Duration::hours(1))
    }
    
    pub fn with_config(max_entries: usize, default_ttl: Duration) -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            max_entries,
            default_ttl,
        }
    }
    
    /// Get cached attestation if valid
    pub async fn get(&self, tier: PrivacyTier) -> Option<AttestationResult> {
        let mut entries = self.entries.write().await;
        let platform = Self::get_platform();
        let key = CacheKey { tier, platform };
        
        if let Some(entry) = entries.get_mut(&key) {
            // Check if still valid
            if entry.result.expires_at > Utc::now() {
                entry.last_accessed = Utc::now();
                entry.access_count += 1;
                
                log::debug!(
                    "✅ Cache hit for tier {:?} (accessed {} times)",
                    tier,
                    entry.access_count
                );
                
                return Some(entry.result.clone());
            } else {
                log::debug!("❌ Cache entry expired for tier {:?}", tier);
                entries.remove(&key);
            }
        }
        
        None
    }
    
    /// Store attestation result in cache
    pub async fn set(&self, tier: PrivacyTier, result: AttestationResult) {
        let mut entries = self.entries.write().await;
        
        // Evict oldest entry if at capacity
        if entries.len() >= self.max_entries {
            Self::evict_oldest(&mut entries);
        }
        
        let platform = Self::get_platform();
        let key = CacheKey { tier, platform };
        
        let entry = CacheEntry {
            result,
            created_at: Utc::now(),
            last_accessed: Utc::now(),
            access_count: 0,
        };
        
        log::debug!("💾 Caching attestation for tier {:?}", tier);
        entries.insert(key, entry);
    }
    
    /// Invalidate cache entry for a tier
    pub async fn invalidate(&self, tier: PrivacyTier) {
        let mut entries = self.entries.write().await;
        let platform = Self::get_platform();
        let key = CacheKey { tier, platform };
        
        if entries.remove(&key).is_some() {
            log::debug!("🗑️ Invalidated cache for tier {:?}", tier);
        }
    }
    
    /// Clear entire cache
    pub async fn clear(&self) {
        let mut entries = self.entries.write().await;
        let count = entries.len();
        entries.clear();
        log::info!("🧹 Cleared {} cache entries", count);
    }
    
    /// Get cache statistics
    pub async fn stats(&self) -> CacheStats {
        let entries = self.entries.read().await;
        
        let mut stats = CacheStats {
            total_entries: entries.len(),
            valid_entries: 0,
            expired_entries: 0,
            total_access_count: 0,
            tier_distribution: HashMap::new(),
        };
        
        let now = Utc::now();
        
        for (key, entry) in entries.iter() {
            if entry.result.expires_at > now {
                stats.valid_entries += 1;
            } else {
                stats.expired_entries += 1;
            }
            
            stats.total_access_count += entry.access_count;
            
            *stats.tier_distribution.entry(key.tier).or_insert(0) += 1;
        }
        
        stats
    }
    
    /// Preload cache with attestations for all supported tiers
    pub async fn preload(&self, tiers: Vec<PrivacyTier>) {
        log::info!("🔄 Preloading cache for {} tiers", tiers.len());
        
        for tier in tiers {
            if tier.requires_attestation() {
                // In production, would generate actual attestations
                // For now, we just log the intent
                log::debug!("Would preload attestation for tier {:?}", tier);
            }
        }
    }
    
    /// Clean expired entries
    pub async fn cleanup(&self) {
        let mut entries = self.entries.write().await;
        let now = Utc::now();
        let before = entries.len();
        
        entries.retain(|key, entry| {
            let valid = entry.result.expires_at > now;
            if !valid {
                log::debug!("🧹 Removing expired entry for tier {:?}", key.tier);
            }
            valid
        });
        
        let removed = before - entries.len();
        if removed > 0 {
            log::info!("🧹 Cleaned up {} expired cache entries", removed);
        }
    }
    
    /// Evict oldest entry (LRU)
    fn evict_oldest(entries: &mut HashMap<CacheKey, CacheEntry>) {
        if let Some((key, _)) = entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_accessed)
            .map(|(k, e)| (k.clone(), e.clone()))
        {
            log::debug!("📤 Evicting oldest cache entry for tier {:?}", key.tier);
            entries.remove(&key);
        }
    }
    
    /// Get current platform identifier
    fn get_platform() -> String {
        // Detect platform type for cache key
        if std::path::Path::new("/dev/sev-guest").exists() {
            "sev-snp".to_string()
        } else if std::path::Path::new("/dev/tdx-guest").exists() {
            "tdx".to_string()
        } else if std::path::Path::new("/dev/sgx_enclave").exists() {
            "sgx".to_string()
        } else {
            "generic".to_string()
        }
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub total_entries: usize,
    pub valid_entries: usize,
    pub expired_entries: usize,
    pub total_access_count: u64,
    pub tier_distribution: HashMap<PrivacyTier, usize>,
}

impl CacheStats {
    pub fn hit_rate(&self) -> f64 {
        if self.total_access_count == 0 {
            0.0
        } else {
            self.valid_entries as f64 / self.total_entries.max(1) as f64
        }
    }
}

/// Background cache manager
pub struct CacheManager {
    cache: Arc<AttestationCache>,
    cleanup_interval: Duration,
}

impl CacheManager {
    pub fn new(cache: Arc<AttestationCache>) -> Self {
        Self {
            cache,
            cleanup_interval: Duration::minutes(5),
        }
    }
    
    /// Start background cleanup task
    pub fn start_cleanup_task(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(
                std::time::Duration::from_secs(self.cleanup_interval.num_seconds() as u64)
            );
            
            loop {
                interval.tick().await;
                self.cache.cleanup().await;
                
                let stats = self.cache.stats().await;
                log::debug!(
                    "📊 Cache stats: {} entries ({} valid), hit rate: {:.2}%",
                    stats.total_entries,
                    stats.valid_entries,
                    stats.hit_rate() * 100.0
                );
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hanzo_kbs::attestation::{Measurement, PlatformInfo};
    
    fn create_mock_attestation(tier: PrivacyTier) -> AttestationResult {
        AttestationResult {
            verified: true,
            max_tier: tier,
            measurements: vec![
                Measurement {
                    name: "test".to_string(),
                    value: vec![0; 32],
                    pcr_index: Some(0),
                }
            ],
            platform_info: PlatformInfo {
                platform_type: "test".to_string(),
                tcb_version: "1.0".to_string(),
                security_features: vec![],
                vendor_info: serde_json::json!({}),
            },
            expires_at: Utc::now() + Duration::hours(1),
        }
    }
    
    #[tokio::test]
    async fn test_cache_operations() {
        let cache = AttestationCache::new();
        
        // Test set and get
        let attestation = create_mock_attestation(PrivacyTier::CpuTee);
        cache.set(PrivacyTier::CpuTee, attestation.clone()).await;
        
        let cached = cache.get(PrivacyTier::CpuTee).await;
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().max_tier, PrivacyTier::CpuTee);
        
        // Test invalidate
        cache.invalidate(PrivacyTier::CpuTee).await;
        let cached = cache.get(PrivacyTier::CpuTee).await;
        assert!(cached.is_none());
    }
    
    #[tokio::test]
    async fn test_cache_expiration() {
        let cache = AttestationCache::with_config(10, Duration::seconds(1));
        
        // Create attestation that expires immediately
        let mut attestation = create_mock_attestation(PrivacyTier::GpuCc);
        attestation.expires_at = Utc::now() - Duration::seconds(1);
        
        cache.set(PrivacyTier::GpuCc, attestation).await;
        
        // Should not return expired entry
        let cached = cache.get(PrivacyTier::GpuCc).await;
        assert!(cached.is_none());
    }
    
    #[tokio::test]
    async fn test_cache_stats() {
        let cache = AttestationCache::new();
        
        // Add multiple entries
        for tier in [PrivacyTier::AtRest, PrivacyTier::CpuTee, PrivacyTier::GpuCc] {
            let attestation = create_mock_attestation(tier);
            cache.set(tier, attestation).await;
        }
        
        // Access some entries
        cache.get(PrivacyTier::CpuTee).await;
        cache.get(PrivacyTier::CpuTee).await;
        
        let stats = cache.stats().await;
        assert_eq!(stats.total_entries, 3);
        assert_eq!(stats.valid_entries, 3);
        assert!(stats.total_access_count > 0);
    }
}