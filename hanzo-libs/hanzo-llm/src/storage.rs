//! SQLite storage layer with vector search
//! 
//! Persistent storage for adapters and vector embeddings

use std::path::Path;
use anyhow::{Result, anyhow};
use serde::{Serialize, Deserialize};
use serde_json;
use rusqlite::{Connection, params, OptionalExtension};
use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;

use crate::{
    bitdelta::CompressedAdapter, 
    adapter::{UserStatistics, PerformanceHistory},
};

/// HLLM storage system
pub struct HLLMStorage {
    pool: Pool<SqliteConnectionManager>,
}

impl HLLMStorage {
    /// Create new storage system
    pub async fn new(db_path: &str) -> Result<Self> {
        let manager = SqliteConnectionManager::file(db_path);
        let pool = Pool::new(manager)?;
        
        // Initialize schema
        let conn = pool.get()?;
        Self::init_schema(&conn)?;
        
        Ok(Self { pool })
    }
    
    /// Initialize database schema
    fn init_schema(conn: &Connection) -> Result<()> {
        // Adapters table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS adapters (
                user_id TEXT PRIMARY KEY,
                compressed_data BLOB NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                metadata TEXT NOT NULL
            )",
            [],
        )?;
        
        // Statistics table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS user_statistics (
                user_id TEXT PRIMARY KEY,
                data TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;
        
        // Performance history table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS performance_history (
                user_id TEXT PRIMARY KEY,
                data TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;
        
        // Vector embeddings table for similarity search
        conn.execute(
            "CREATE TABLE IF NOT EXISTS vector_embeddings (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id TEXT NOT NULL,
                embedding BLOB NOT NULL,
                dimension INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                FOREIGN KEY (user_id) REFERENCES adapters(user_id)
            )",
            [],
        )?;
        
        // Create indices
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_vectors_user ON vector_embeddings(user_id)",
            [],
        )?;
        
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_adapters_updated ON adapters(updated_at)",
            [],
        )?;
        
        // Regime transitions table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS regime_transitions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                from_regime TEXT NOT NULL,
                to_regime TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                user_id TEXT,
                observations BLOB,
                FOREIGN KEY (user_id) REFERENCES adapters(user_id)
            )",
            [],
        )?;
        
        Ok(())
    }
    
    /// Get connection from pool
    fn conn(&self) -> Result<PooledConnection<SqliteConnectionManager>> {
        self.pool.get().map_err(|e| anyhow!("Failed to get connection: {}", e))
    }
    
    /// Save compressed adapter
    pub async fn save_adapter(&self, user_id: &str, adapter: CompressedAdapter) -> Result<()> {
        let conn = self.conn()?;
        let now = chrono::Utc::now().timestamp();
        
        // Serialize adapter data
        let data = bincode::serialize(&adapter)?;
        let metadata = serde_json::to_string(&adapter.metadata)?;
        
        conn.execute(
            "INSERT INTO adapters (user_id, compressed_data, created_at, updated_at, metadata) 
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(user_id) DO UPDATE SET 
                compressed_data = ?2,
                updated_at = ?4,
                metadata = ?5",
            params![user_id, data, now, now, metadata],
        )?;
        
        Ok(())
    }
    
    /// Load compressed adapter
    pub async fn load_adapter(&self, user_id: &str) -> Result<CompressedAdapter> {
        let conn = self.conn()?;
        
        let data: Vec<u8> = conn.query_row(
            "SELECT compressed_data FROM adapters WHERE user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )?;
        
        bincode::deserialize(&data).map_err(|e| anyhow!("Failed to deserialize adapter: {}", e))
    }
    
    /// Save user statistics
    pub async fn save_statistics(&self, user_id: &str, stats: &UserStatistics) -> Result<()> {
        let conn = self.conn()?;
        let now = chrono::Utc::now().timestamp();
        let data = serde_json::to_string(stats)?;
        
        conn.execute(
            "INSERT INTO user_statistics (user_id, data, updated_at) 
             VALUES (?1, ?2, ?3)
             ON CONFLICT(user_id) DO UPDATE SET 
                data = ?2,
                updated_at = ?3",
            params![user_id, data, now],
        )?;
        
        Ok(())
    }
    
    /// Load user statistics
    pub async fn load_statistics(&self, user_id: &str) -> Result<UserStatistics> {
        let conn = self.conn()?;
        
        let data: String = conn.query_row(
            "SELECT data FROM user_statistics WHERE user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )?;
        
        serde_json::from_str(&data).map_err(|e| anyhow!("Failed to parse statistics: {}", e))
    }
    
    /// Save performance history
    pub async fn save_performance(&self, user_id: &str, perf: &PerformanceHistory) -> Result<()> {
        let conn = self.conn()?;
        let now = chrono::Utc::now().timestamp();
        let data = serde_json::to_string(perf)?;
        
        conn.execute(
            "INSERT INTO performance_history (user_id, data, updated_at) 
             VALUES (?1, ?2, ?3)
             ON CONFLICT(user_id) DO UPDATE SET 
                data = ?2,
                updated_at = ?3",
            params![user_id, data, now],
        )?;
        
        Ok(())
    }
    
    /// Load performance history
    pub async fn load_performance(&self, user_id: &str) -> Result<PerformanceHistory> {
        let conn = self.conn()?;
        
        let data: String = conn.query_row(
            "SELECT data FROM performance_history WHERE user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )?;
        
        serde_json::from_str(&data).map_err(|e| anyhow!("Failed to parse performance: {}", e))
    }
    
    /// Store vector embedding
    pub async fn store_embedding(
        &self,
        user_id: &str,
        embedding: &[f32],
    ) -> Result<u64> {
        let conn = self.conn()?;
        let now = chrono::Utc::now().timestamp();
        
        // Convert to bytes
        let bytes: Vec<u8> = embedding.iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        
        conn.execute(
            "INSERT INTO vector_embeddings (user_id, embedding, dimension, created_at) 
             VALUES (?1, ?2, ?3, ?4)",
            params![user_id, bytes, embedding.len() as i64, now],
        )?;
        
        Ok(conn.last_insert_rowid() as u64)
    }
    
    /// Find similar vectors (simple cosine similarity)
    pub async fn find_similar(
        &self,
        query: &[f32],
        limit: usize,
    ) -> Result<Vec<SimilarityResult>> {
        let conn = self.conn()?;
        
        let mut stmt = conn.prepare(
            "SELECT id, user_id, embedding, dimension FROM vector_embeddings"
        )?;
        
        let results = stmt.query_map([], |row| {
            let id: i64 = row.get(0)?;
            let user_id: String = row.get(1)?;
            let bytes: Vec<u8> = row.get(2)?;
            let dimension: i64 = row.get(3)?;
            
            Ok((id, user_id, bytes, dimension))
        })?;
        
        let mut similarities = Vec::new();
        
        for result in results {
            let (id, user_id, bytes, dimension) = result?;
            
            // Convert bytes back to float vector
            let embedding: Vec<f32> = bytes.chunks(4)
                .map(|chunk| {
                    let arr = [chunk[0], chunk[1], chunk[2], chunk[3]];
                    f32::from_le_bytes(arr)
                })
                .collect();
            
            if embedding.len() == query.len() {
                let similarity = cosine_similarity(query, &embedding);
                similarities.push(SimilarityResult {
                    id: id as u64,
                    user_id,
                    similarity,
                });
            }
        }
        
        // Sort by similarity descending
        similarities.sort_by(|a, b| b.similarity.partial_cmp(&a.similarity).unwrap());
        similarities.truncate(limit);
        
        Ok(similarities)
    }
    
    /// Record regime transition
    pub async fn record_transition(
        &self,
        from: &str,
        to: &str,
        user_id: Option<&str>,
        observations: Option<&[f64]>,
    ) -> Result<()> {
        let conn = self.conn()?;
        let now = chrono::Utc::now().timestamp();
        
        let obs_bytes = observations.map(|obs| {
            obs.iter()
                .flat_map(|f| f.to_le_bytes())
                .collect::<Vec<u8>>()
        });
        
        conn.execute(
            "INSERT INTO regime_transitions (from_regime, to_regime, timestamp, user_id, observations) 
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![from, to, now, user_id, obs_bytes],
        )?;
        
        Ok(())
    }
    
    /// Get transition statistics
    pub async fn get_transition_stats(&self) -> Result<TransitionStatistics> {
        let conn = self.conn()?;
        
        let mut stmt = conn.prepare(
            "SELECT from_regime, to_regime, COUNT(*) as count 
             FROM regime_transitions 
             GROUP BY from_regime, to_regime"
        )?;
        
        let transitions = stmt.query_map([], |row| {
            Ok(TransitionCount {
                from: row.get(0)?,
                to: row.get(1)?,
                count: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
        
        let total: u64 = conn.query_row(
            "SELECT COUNT(*) FROM regime_transitions",
            [],
            |row| row.get(0),
        )?;
        
        Ok(TransitionStatistics {
            transitions,
            total_transitions: total,
        })
    }
    
    /// Cleanup old data
    pub async fn cleanup_old_data(&self, days_old: i64) -> Result<usize> {
        let conn = self.conn()?;
        let cutoff = chrono::Utc::now().timestamp() - (days_old * 24 * 3600);
        
        let deleted = conn.execute(
            "DELETE FROM vector_embeddings WHERE created_at < ?1",
            params![cutoff],
        )?;
        
        conn.execute(
            "DELETE FROM regime_transitions WHERE timestamp < ?1",
            params![cutoff],
        )?;
        
        Ok(deleted)
    }
}

/// Vector index for fast similarity search
pub struct VectorIndex {
    /// Dimension of vectors
    dimension: usize,
    
    /// Indexed vectors
    vectors: Vec<IndexedVector>,
}

impl VectorIndex {
    /// Create new index
    pub fn new(dimension: usize) -> Self {
        Self {
            dimension,
            vectors: Vec::new(),
        }
    }
    
    /// Add vector to index
    pub fn add(&mut self, id: String, vector: Vec<f32>) -> Result<()> {
        if vector.len() != self.dimension {
            return Err(anyhow!("Vector dimension mismatch"));
        }
        
        let norm = calculate_norm(&vector);
        self.vectors.push(IndexedVector {
            id,
            vector,
            norm,
        });
        
        Ok(())
    }
    
    /// Find k nearest neighbors
    pub fn knn(&self, query: &[f32], k: usize) -> Result<Vec<(String, f32)>> {
        if query.len() != self.dimension {
            return Err(anyhow!("Query dimension mismatch"));
        }
        
        let query_norm = calculate_norm(query);
        
        let mut distances: Vec<_> = self.vectors.iter()
            .map(|v| {
                let similarity = dot_product(query, &v.vector) / (query_norm * v.norm);
                (v.id.clone(), similarity)
            })
            .collect();
        
        distances.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        distances.truncate(k);
        
        Ok(distances)
    }
    
    /// Build from storage
    pub async fn from_storage(storage: &HLLMStorage, dimension: usize) -> Result<Self> {
        let mut index = Self::new(dimension);
        
        // Load all embeddings (in production, paginate this)
        // For now, simplified implementation
        
        Ok(index)
    }
}

/// Indexed vector
struct IndexedVector {
    id: String,
    vector: Vec<f32>,
    norm: f32,
}

/// Similarity search result
#[derive(Debug, Clone)]
pub struct SimilarityResult {
    pub id: u64,
    pub user_id: String,
    pub similarity: f32,
}

/// Transition count
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionCount {
    pub from: String,
    pub to: String,
    pub count: u64,
}

/// Transition statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionStatistics {
    pub transitions: Vec<TransitionCount>,
    pub total_transitions: u64,
}

/// Calculate cosine similarity
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot = dot_product(a, b);
    let norm_a = calculate_norm(a);
    let norm_b = calculate_norm(b);
    
    if norm_a > 0.0 && norm_b > 0.0 {
        dot / (norm_a * norm_b)
    } else {
        0.0
    }
}

/// Calculate dot product
fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Calculate vector norm
fn calculate_norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    
    #[tokio::test]
    async fn test_storage_creation() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        
        let storage = HLLMStorage::new(db_path.to_str().unwrap()).await.unwrap();
        
        // Should create tables
        let conn = storage.conn().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table'",
            [],
            |row| row.get(0),
        ).unwrap();
        
        assert!(count > 0);
    }
    
    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0, 3.0];
        let c = vec![-1.0, -2.0, -3.0];
        
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);
        assert!((cosine_similarity(&a, &c) + 1.0).abs() < 1e-6);
    }
}