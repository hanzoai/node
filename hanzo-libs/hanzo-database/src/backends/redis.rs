//! Redis backend implementation for caching and real-time operations
//! 
//! Redis is an in-memory data structure store, perfect for caching,
//! session management, real-time analytics, and pub/sub messaging.

use crate::{DatabaseBackend, HanzoDbConfig, HanzoDbError, Result};
use async_trait::async_trait;
use redis::{aio::ConnectionManager, AsyncCommands, Client};
use serde_json::Value;
use std::time::Duration;

/// Redis backend for caching and real-time operations
pub struct RedisBackend {
    client: Client,
    connection: ConnectionManager,
    config: HanzoDbConfig,
}

impl RedisBackend {
    /// Create a new Redis backend
    pub async fn new(config: HanzoDbConfig) -> Result<Self> {
        let redis_url = config.url.as_ref()
            .unwrap_or(&"redis://127.0.0.1:6379".to_string())
            .clone();

        // Create Redis client
        let client = Client::open(redis_url.as_str())
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

        // Create connection manager for connection pooling
        let connection = ConnectionManager::new(client.clone()).await
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

        Ok(Self {
            client,
            connection,
            config,
        })
    }

    /// Set value with optional TTL
    pub async fn set_with_ttl(&mut self, key: &str, value: &Value, ttl_seconds: Option<u64>) -> Result<()> {
        let serialized = serde_json::to_string(value)
            .map_err(|e| HanzoDbError::SerializationError(e.to_string()))?;

        if let Some(ttl) = ttl_seconds {
            self.connection
                .set_ex(key, serialized, ttl)
                .await
                .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
        } else {
            self.connection
                .set(key, serialized)
                .await
                .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
        }

        Ok(())
    }

    /// Get value by key
    pub async fn get(&mut self, key: &str) -> Result<Option<Value>> {
        let value: Option<String> = self.connection
            .get(key)
            .await
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        match value {
            Some(v) => {
                let parsed = serde_json::from_str(&v)
                    .map_err(|e| HanzoDbError::DeserializationError(e.to_string()))?;
                Ok(Some(parsed))
            },
            None => Ok(None),
        }
    }

    /// Delete key
    pub async fn del(&mut self, key: &str) -> Result<()> {
        self.connection
            .del(key)
            .await
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
        Ok(())
    }

    /// Check if key exists
    pub async fn exists(&mut self, key: &str) -> Result<bool> {
        let exists: bool = self.connection
            .exists(key)
            .await
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
        Ok(exists)
    }

    /// Set multiple keys at once
    pub async fn mset(&mut self, items: &[(String, Value)]) -> Result<()> {
        let serialized: Vec<(String, String)> = items
            .iter()
            .map(|(k, v)| {
                serde_json::to_string(v)
                    .map(|s| (k.clone(), s))
                    .map_err(|e| HanzoDbError::SerializationError(e.to_string()))
            })
            .collect::<Result<Vec<_>>>()?;

        self.connection
            .set_multiple(&serialized)
            .await
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        Ok(())
    }

    /// Get multiple keys at once
    pub async fn mget(&mut self, keys: &[String]) -> Result<Vec<Option<Value>>> {
        let values: Vec<Option<String>> = self.connection
            .get(keys)
            .await
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        values
            .into_iter()
            .map(|opt_v| {
                match opt_v {
                    Some(v) => {
                        serde_json::from_str(&v)
                            .map(Some)
                            .map_err(|e| HanzoDbError::DeserializationError(e.to_string()))
                    },
                    None => Ok(None),
                }
            })
            .collect()
    }

    /// Increment counter
    pub async fn incr(&mut self, key: &str, delta: i64) -> Result<i64> {
        let new_value: i64 = self.connection
            .incr(key, delta)
            .await
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
        Ok(new_value)
    }

    /// Add to set
    pub async fn sadd(&mut self, key: &str, members: &[String]) -> Result<()> {
        for member in members {
            self.connection
                .sadd(key, member)
                .await
                .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
        }
        Ok(())
    }

    /// Get set members
    pub async fn smembers(&mut self, key: &str) -> Result<Vec<String>> {
        let members: Vec<String> = self.connection
            .smembers(key)
            .await
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
        Ok(members)
    }

    /// Publish message to channel
    pub async fn publish(&mut self, channel: &str, message: &Value) -> Result<()> {
        let serialized = serde_json::to_string(message)
            .map_err(|e| HanzoDbError::SerializationError(e.to_string()))?;

        self.connection
            .publish(channel, serialized)
            .await
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        Ok(())
    }

    /// Add to sorted set with score
    pub async fn zadd(&mut self, key: &str, member: &str, score: f64) -> Result<()> {
        self.connection
            .zadd(key, member, score)
            .await
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
        Ok(())
    }

    /// Get sorted set range by score
    pub async fn zrangebyscore(&mut self, key: &str, min: f64, max: f64, limit: Option<usize>) -> Result<Vec<String>> {
        let mut cmd = redis::cmd("ZRANGEBYSCORE");
        cmd.arg(key).arg(min).arg(max);
        
        if let Some(l) = limit {
            cmd.arg("LIMIT").arg(0).arg(l);
        }

        let members: Vec<String> = cmd
            .query_async(&mut self.connection)
            .await
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
        
        Ok(members)
    }

    /// Store vector embedding with metadata
    pub async fn store_embedding(
        &mut self,
        key: &str,
        embedding: &[f32],
        metadata: Option<&Value>,
    ) -> Result<()> {
        // Redis doesn't have native vector search, but we can store for caching
        let data = serde_json::json!({
            "embedding": embedding,
            "metadata": metadata,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        });

        self.set_with_ttl(key, &data, Some(3600)).await
    }

    /// Cache query result
    pub async fn cache_result(&mut self, query_hash: &str, result: &Value, ttl_seconds: u64) -> Result<()> {
        let cache_key = format!("cache:{}", query_hash);
        self.set_with_ttl(&cache_key, result, Some(ttl_seconds)).await
    }

    /// Get cached result
    pub async fn get_cached(&mut self, query_hash: &str) -> Result<Option<Value>> {
        let cache_key = format!("cache:{}", query_hash);
        self.get(&cache_key).await
    }
}

#[async_trait]
impl DatabaseBackend for RedisBackend {
    async fn connect(&mut self) -> Result<()> {
        // Already connected in new()
        // Test connection
        redis::cmd("PING")
            .query_async::<_, String>(&mut self.connection)
            .await
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        // Connection manager handles cleanup
        Ok(())
    }

    async fn create_table(&self, name: &str, _schema: &Value) -> Result<()> {
        // Redis doesn't have tables, use key prefixes instead
        // Store table metadata
        let mut conn = self.connection.clone();
        let table_meta = serde_json::json!({
            "name": name,
            "created_at": chrono::Utc::now().to_rfc3339(),
            "type": "redis_namespace"
        });
        
        let meta_key = format!("_meta:table:{}", name);
        conn.set(meta_key, table_meta.to_string())
            .await
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
        
        Ok(())
    }

    async fn drop_table(&self, name: &str) -> Result<()> {
        // Delete all keys with table prefix
        let mut conn = self.connection.clone();
        let pattern = format!("{}:*", name);
        
        // Get all keys matching pattern
        let keys: Vec<String> = redis::cmd("KEYS")
            .arg(&pattern)
            .query_async(&mut conn)
            .await
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
        
        // Delete keys in batches
        for chunk in keys.chunks(1000) {
            redis::cmd("DEL")
                .arg(chunk)
                .query_async::<_, ()>(&mut conn)
                .await
                .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
        }
        
        // Delete table metadata
        let meta_key = format!("_meta:table:{}", name);
        conn.del(meta_key)
            .await
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
        
        Ok(())
    }

    async fn insert(&self, table: &str, records: &[Value]) -> Result<()> {
        let mut conn = self.connection.clone();
        
        for (i, record) in records.iter().enumerate() {
            let key = if let Some(id) = record.get("id").and_then(|v| v.as_str()) {
                format!("{}:{}", table, id)
            } else {
                format!("{}:{}", table, uuid::Uuid::new_v4())
            };
            
            let serialized = serde_json::to_string(record)
                .map_err(|e| HanzoDbError::SerializationError(e.to_string()))?;
            
            conn.set(key, serialized)
                .await
                .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
            
            // Add to table index
            let index_key = format!("_index:{}", table);
            conn.sadd(index_key, &key)
                .await
                .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
        }
        
        Ok(())
    }

    async fn query(&self, query: &str) -> Result<Vec<Value>> {
        // Redis doesn't support SQL, interpret as key pattern
        let mut conn = self.connection.clone();
        
        let keys: Vec<String> = if query.starts_with("SELECT * FROM ") {
            // Extract table name from simple query
            let table = query
                .strip_prefix("SELECT * FROM ")
                .and_then(|s| s.split_whitespace().next())
                .ok_or_else(|| HanzoDbError::InvalidQuery("Invalid SELECT query".to_string()))?;
            
            // Get all keys from table index
            let index_key = format!("_index:{}", table);
            conn.smembers(index_key)
                .await
                .map_err(|e| HanzoDbError::QueryError(e.to_string()))?
        } else {
            // Treat as key pattern
            redis::cmd("KEYS")
                .arg(query)
                .query_async(&mut conn)
                .await
                .map_err(|e| HanzoDbError::QueryError(e.to_string()))?
        };
        
        // Get all values
        let mut results = Vec::new();
        for key in keys {
            let value: Option<String> = conn.get(&key)
                .await
                .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
            
            if let Some(v) = value {
                if let Ok(parsed) = serde_json::from_str(&v) {
                    results.push(parsed);
                }
            }
        }
        
        Ok(results)
    }

    async fn update(&self, table: &str, id: &str, data: &Value) -> Result<()> {
        let mut conn = self.connection.clone();
        let key = format!("{}:{}", table, id);
        
        // Get existing record
        let existing: Option<String> = conn.get(&key)
            .await
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
        
        let mut record = if let Some(e) = existing {
            serde_json::from_str(&e)
                .map_err(|e| HanzoDbError::DeserializationError(e.to_string()))?
        } else {
            serde_json::json!({})
        };
        
        // Merge updates
        if let (Some(rec_obj), Some(data_obj)) = (record.as_object_mut(), data.as_object()) {
            for (k, v) in data_obj {
                rec_obj.insert(k.clone(), v.clone());
            }
        }
        
        // Save updated record
        let serialized = serde_json::to_string(&record)
            .map_err(|e| HanzoDbError::SerializationError(e.to_string()))?;
        
        conn.set(key, serialized)
            .await
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
        
        Ok(())
    }

    async fn delete(&self, table: &str, id: &str) -> Result<()> {
        let mut conn = self.connection.clone();
        let key = format!("{}:{}", table, id);
        
        // Remove from index
        let index_key = format!("_index:{}", table);
        conn.srem(index_key, &key)
            .await
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
        
        // Delete key
        conn.del(key)
            .await
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
        
        Ok(())
    }

    async fn vector_search(
        &self,
        _table: &str,
        query_vector: &[f32],
        limit: usize,
        _filter: Option<&str>,
    ) -> Result<Vec<Value>> {
        // Redis doesn't have native vector search
        // For production, use RedisSearch with vector similarity
        // This is a placeholder that returns cached results if available
        
        let mut conn = self.connection.clone();
        let query_hash = blake3::hash(
            &query_vector.iter()
                .flat_map(|f| f.to_le_bytes())
                .collect::<Vec<_>>()
        ).to_hex();
        
        let cache_key = format!("vector_cache:{}", query_hash.as_str());
        
        if let Some(cached) = self.get_cached(query_hash.as_str()).await? {
            return Ok(vec![cached]);
        }
        
        // Return empty result - real implementation would use RedisSearch
        Ok(vec![])
    }

    fn backend_type(&self) -> DatabaseBackend {
        DatabaseBackend::Redis
    }

    fn is_cache(&self) -> bool {
        true // Redis is primarily for caching
    }

    fn is_real_time(&self) -> bool {
        true // Redis excels at real-time operations
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_redis_operations() {
        // Skip if Redis not available
        let config = HanzoDbConfig {
            backend: DatabaseBackend::Redis,
            url: Some("redis://localhost:6379".to_string()),
            ..Default::default()
        };

        let mut backend = match RedisBackend::new(config).await {
            Ok(b) => b,
            Err(_) => {
                println!("Redis not available, skipping test");
                return;
            }
        };

        // Test basic operations
        let key = "test_key";
        let value = serde_json::json!({"foo": "bar"});
        
        backend.set_with_ttl(key, &value, Some(60)).await.unwrap();
        
        let retrieved = backend.get(key).await.unwrap();
        assert_eq!(retrieved, Some(value));
        
        backend.del(key).await.unwrap();
        
        let deleted = backend.get(key).await.unwrap();
        assert_eq!(deleted, None);
    }
}