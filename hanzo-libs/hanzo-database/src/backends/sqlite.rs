//! SQLite backend implementation for embedded database operations
//! 
//! SQLite is a lightweight, serverless, self-contained SQL database engine,
//! perfect for embedded applications, testing, and local development.

use crate::{HanzoDbConfig, HanzoDatabase, HanzoDbError, TableSchema, Record, Query, QueryResult, VectorQuery, SearchResult, Transaction, DatabaseStats};
use anyhow::Result;
use async_trait::async_trait;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::task;

/// SQLite backend for embedded database operations
pub struct SqliteBackend {
    connection: Arc<Mutex<Connection>>,
    path: PathBuf,
    config: HanzoDbConfig,
}

impl SqliteBackend {
    /// Create a new SQLite backend
    pub fn new(config: HanzoDbConfig) -> Result<Self> {
        let path = config.path.clone().unwrap_or_else(|| {
            PathBuf::from("./storage/hanzo-sqlite.db")
        });

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;
        }

        // Open SQLite connection
        let connection = if path.to_str() == Some(":memory:") {
            Connection::open_in_memory()
        } else {
            Connection::open(&path)
        }.map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

        // Configure SQLite for optimal performance
        connection.execute_batch("
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA cache_size = -64000;
            PRAGMA mmap_size = 30000000000;
            PRAGMA temp_store = MEMORY;
            PRAGMA foreign_keys = ON;
        ").map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        // Enable JSON1 extension
        connection.execute_batch("
            CREATE TABLE IF NOT EXISTS _meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );
        ").map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            path,
            config,
        })
    }

    /// Execute a query and return results as JSON
    fn execute_query(&self, query: &str) -> Result<Vec<Value>> {
        let conn = self.connection.lock()
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

        let mut stmt = conn.prepare(query)
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        let column_names: Vec<String> = stmt
            .column_names()
            .into_iter()
            .map(|s| s.to_string())
            .collect();

        let rows = stmt.query_map(params![], |row| {
            let mut obj = serde_json::Map::new();
            
            for (i, name) in column_names.iter().enumerate() {
                let value = match row.get_ref(i) {
                    Ok(val) => {
                        use rusqlite::types::ValueRef;
                        match val {
                            ValueRef::Null => Value::Null,
                            ValueRef::Integer(i) => Value::Number(i.into()),
                            ValueRef::Real(f) => Value::from(f),
                            ValueRef::Text(s) => {
                                // Try to parse as JSON first
                                if let Ok(json) = serde_json::from_str(std::str::from_utf8(s).unwrap_or("")) {
                                    json
                                } else {
                                    Value::String(std::str::from_utf8(s).unwrap_or("").to_string())
                                }
                            },
                            ValueRef::Blob(b) => {
                                // Convert blob to base64
                                Value::String(base64::encode(b))
                            },
                        }
                    },
                    Err(_) => Value::Null,
                };
                
                obj.insert(name.clone(), value);
            }
            
            Ok(Value::Object(obj))
        }).map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))
    }

    /// Create vector extension tables
    pub fn enable_vector_support(&self) -> Result<()> {
        let conn = self.connection.lock()
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

        // Create vector storage table
        conn.execute_batch("
            CREATE TABLE IF NOT EXISTS _vectors (
                id TEXT PRIMARY KEY,
                table_name TEXT NOT NULL,
                record_id TEXT NOT NULL,
                vector BLOB NOT NULL,
                metadata TEXT,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(table_name, record_id)
            );
            
            CREATE INDEX IF NOT EXISTS idx_vectors_table ON _vectors(table_name);
            CREATE INDEX IF NOT EXISTS idx_vectors_record ON _vectors(table_name, record_id);
        ").map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        Ok(())
    }

    /// Store vector embedding
    pub fn store_vector(
        &self,
        table: &str,
        record_id: &str,
        vector: &[f32],
        metadata: Option<&Value>,
    ) -> Result<()> {
        let conn = self.connection.lock()
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

        // Convert vector to bytes
        let vector_bytes: Vec<u8> = vector.iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();

        let metadata_str = metadata
            .map(|m| serde_json::to_string(m))
            .transpose()
            .map_err(|e| HanzoDbError::SerializationError(e.to_string()))?;

        conn.execute(
            "INSERT OR REPLACE INTO _vectors (id, table_name, record_id, vector, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                format!("{}:{}", table, record_id),
                table,
                record_id,
                vector_bytes,
                metadata_str
            ]
        ).map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        Ok(())
    }

    /// Simple cosine similarity search (not optimized)
    pub fn vector_similarity_search(
        &self,
        table: &str,
        query_vector: &[f32],
        limit: usize,
    ) -> Result<Vec<Value>> {
        let conn = self.connection.lock()
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

        // Get all vectors from table
        let mut stmt = conn.prepare(
            "SELECT record_id, vector, metadata FROM _vectors WHERE table_name = ?1"
        ).map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        let vectors = stmt.query_map(params![table], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        }).map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        // Calculate cosine similarity for each vector
        let mut results: Vec<(String, f32, Option<String>)> = Vec::new();
        
        for vector_result in vectors {
            let (record_id, vector_bytes, metadata) = vector_result
                .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

            // Convert bytes back to f32 vector
            let vector: Vec<f32> = vector_bytes
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect();

            // Calculate cosine similarity
            let similarity = cosine_similarity(query_vector, &vector);
            results.push((record_id, similarity, metadata));
        }

        // Sort by similarity (descending)
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        // Take top N results
        let top_results: Vec<Value> = results
            .into_iter()
            .take(limit)
            .map(|(id, score, metadata)| {
                let mut obj = serde_json::Map::new();
                obj.insert("record_id".to_string(), Value::String(id));
                obj.insert("similarity".to_string(), Value::from(score));
                if let Some(meta) = metadata {
                    if let Ok(meta_json) = serde_json::from_str::<Value>(&meta) {
                        obj.insert("metadata".to_string(), meta_json);
                    }
                }
                Value::Object(obj)
            })
            .collect();

        Ok(top_results)
    }
}

#[async_trait]
impl HanzoDatabase for SqliteBackend {
    async fn connect(&mut self) -> Result<()> {
        // Already connected in new()
        // Test connection
        let conn = self.connection.clone();
        task::spawn_blocking(move || {
            let c = conn.lock()
                .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;
            c.execute("SELECT 1", params![])
                .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;
            Ok::<(), HanzoDbError>(())
        }).await
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))??;
        
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        // Connection will be closed when dropped
        Ok(())
    }

    async fn create_table(&self, name: &str, schema: &Value) -> Result<()> {
        let conn = self.connection.clone();
        let name = name.to_string();
        let schema = schema.clone();

        task::spawn_blocking(move || {
            let c = conn.lock()
                .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

            let columns = schema.as_object()
                .ok_or_else(|| HanzoDbError::InvalidSchema("Schema must be object".to_string()))?;

            let mut column_defs = Vec::new();
            
            // Add ID column if not present
            if !columns.contains_key("id") {
                column_defs.push("id INTEGER PRIMARY KEY AUTOINCREMENT".to_string());
            }

            for (col_name, type_def) in columns {
                let sql_type = match type_def.as_str() {
                    Some("string") => "TEXT",
                    Some("integer") => "INTEGER",
                    Some("bigint") => "INTEGER",
                    Some("float") | Some("double") => "REAL",
                    Some("boolean") => "INTEGER", // SQLite uses 0/1 for boolean
                    Some("timestamp") => "DATETIME",
                    Some("date") => "DATE",
                    Some("json") | Some("jsonb") => "TEXT", // Store JSON as TEXT
                    Some("vector") => "BLOB", // Store vectors as BLOB
                    _ => "TEXT",
                };
                
                column_defs.push(format!("{} {}", col_name, sql_type));
            }

            // Add timestamps if not present
            if !columns.contains_key("created_at") {
                column_defs.push("created_at DATETIME DEFAULT CURRENT_TIMESTAMP".to_string());
            }
            if !columns.contains_key("updated_at") {
                column_defs.push("updated_at DATETIME DEFAULT CURRENT_TIMESTAMP".to_string());
            }

            let create_sql = format!(
                "CREATE TABLE IF NOT EXISTS {} ({})",
                name,
                column_defs.join(", ")
            );

            c.execute(&create_sql, params![])
                .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

            // Create update trigger for updated_at
            let trigger_sql = format!(
                "CREATE TRIGGER IF NOT EXISTS update_{}_timestamp
                 AFTER UPDATE ON {}
                 FOR EACH ROW
                 BEGIN
                   UPDATE {} SET updated_at = CURRENT_TIMESTAMP WHERE id = NEW.id;
                 END",
                name, name, name
            );

            c.execute(&trigger_sql, params![])
                .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

            Ok::<(), HanzoDbError>(())
        }).await
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))??;

        // Enable vector support if needed
        if schema.as_object()
            .and_then(|o| o.keys().find(|k| k.contains("vector") || k.contains("embedding")))
            .is_some() 
        {
            self.enable_vector_support()?;
        }

        Ok(())
    }

    async fn drop_table(&self, name: &str) -> Result<()> {
        let conn = self.connection.clone();
        let name = name.to_string();

        task::spawn_blocking(move || {
            let c = conn.lock()
                .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

            c.execute(&format!("DROP TABLE IF EXISTS {}", name), params![])
                .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

            Ok::<(), HanzoDbError>(())
        }).await
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))??;

        Ok(())
    }

    async fn insert(&self, table: &str, records: &[Value]) -> Result<()> {
        let conn = self.connection.clone();
        let table = table.to_string();
        let records = records.to_vec();

        task::spawn_blocking(move || {
            let c = conn.lock()
                .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

            let tx = c.transaction()
                .map_err(|e| HanzoDbError::TransactionError(e.to_string()))?;

            for record in records {
                let obj = record.as_object()
                    .ok_or_else(|| HanzoDbError::InvalidData("Record must be object".to_string()))?;

                let columns: Vec<String> = obj.keys().cloned().collect();
                let placeholders: Vec<String> = (1..=columns.len())
                    .map(|i| format!("?{}", i))
                    .collect();

                let insert_sql = format!(
                    "INSERT INTO {} ({}) VALUES ({})",
                    table,
                    columns.join(", "),
                    placeholders.join(", ")
                );

                let params: Vec<rusqlite::types::Value> = obj.values().map(|v| {
                    match v {
                        Value::String(s) => rusqlite::types::Value::Text(s.clone()),
                        Value::Number(n) => {
                            if let Some(i) = n.as_i64() {
                                rusqlite::types::Value::Integer(i)
                            } else if let Some(f) = n.as_f64() {
                                rusqlite::types::Value::Real(f)
                            } else {
                                rusqlite::types::Value::Null
                            }
                        },
                        Value::Bool(b) => rusqlite::types::Value::Integer(if b { 1 } else { 0 }),
                        Value::Null => rusqlite::types::Value::Null,
                        Value::Array(_) | Value::Object(_) => {
                            // Store complex types as JSON
                            rusqlite::types::Value::Text(v.to_string())
                        }
                    }
                }).collect();

                tx.execute(&insert_sql, params.as_slice())
                    .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
            }

            tx.commit()
                .map_err(|e| HanzoDbError::TransactionError(e.to_string()))?;

            Ok::<(), HanzoDbError>(())
        }).await
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))??;

        Ok(())
    }

    async fn query(&self, query: &str) -> Result<Vec<Value>> {
        let conn = self.connection.clone();
        let query = query.to_string();

        task::spawn_blocking(move || {
            let c = conn.lock()
                .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

            // Use the helper method to execute query and get JSON results
            drop(c); // Release lock before calling execute_query
            
            // Re-acquire for execute_query (this is safe as it's in the same thread)
            let backend = SqliteBackend {
                connection: conn,
                path: PathBuf::new(), // Not used in execute_query
                config: HanzoDbConfig::default(), // Not used in execute_query
            };
            
            backend.execute_query(&query)
        }).await
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?
    }

    async fn update(&self, table: &str, id: &str, data: &Value) -> Result<()> {
        let conn = self.connection.clone();
        let table = table.to_string();
        let id = id.to_string();
        let data = data.clone();

        task::spawn_blocking(move || {
            let c = conn.lock()
                .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

            let obj = data.as_object()
                .ok_or_else(|| HanzoDbError::InvalidData("Update data must be object".to_string()))?;

            let set_clauses: Vec<String> = obj.keys()
                .enumerate()
                .map(|(i, key)| format!("{} = ?{}", key, i + 2))
                .collect();

            let update_sql = format!(
                "UPDATE {} SET {}, updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
                table,
                set_clauses.join(", ")
            );

            let mut params: Vec<rusqlite::types::Value> = vec![rusqlite::types::Value::Text(id)];
            
            for value in obj.values() {
                params.push(match value {
                    Value::String(s) => rusqlite::types::Value::Text(s.clone()),
                    Value::Number(n) => {
                        if let Some(i) = n.as_i64() {
                            rusqlite::types::Value::Integer(i)
                        } else if let Some(f) = n.as_f64() {
                            rusqlite::types::Value::Real(f)
                        } else {
                            rusqlite::types::Value::Null
                        }
                    },
                    Value::Bool(b) => rusqlite::types::Value::Integer(if *b { 1 } else { 0 }),
                    Value::Null => rusqlite::types::Value::Null,
                    _ => rusqlite::types::Value::Text(value.to_string()),
                });
            }

            c.execute(&update_sql, params.as_slice())
                .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

            Ok::<(), HanzoDbError>(())
        }).await
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))??;

        Ok(())
    }

    async fn delete(&self, table: &str, id: &str) -> Result<()> {
        let conn = self.connection.clone();
        let table = table.to_string();
        let id = id.to_string();

        task::spawn_blocking(move || {
            let c = conn.lock()
                .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

            c.execute(
                &format!("DELETE FROM {} WHERE id = ?1", table),
                params![id]
            ).map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

            Ok::<(), HanzoDbError>(())
        }).await
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))??;

        Ok(())
    }

    async fn vector_search(
        &self,
        table: &str,
        query_vector: &[f32],
        limit: usize,
        _filter: Option<&str>,
    ) -> Result<Vec<Value>> {
        let table = table.to_string();
        let query_vector = query_vector.to_vec();
        let conn = self.connection.clone();
        let path = self.path.clone();
        let config = self.config.clone();

        task::spawn_blocking(move || {
            let backend = SqliteBackend {
                connection: conn,
                path,
                config,
            };
            
            backend.vector_similarity_search(&table, &query_vector, limit)
        }).await
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?
    }

    fn backend_type(&self) -> DatabaseBackend {
        DatabaseBackend::SQLite
    }

    fn is_embedded(&self) -> bool {
        true // SQLite is embedded
    }

    fn is_lightweight(&self) -> bool {
        true // SQLite is very lightweight
    }

    async fn init(&self) -> Result<()> {
        // SQLite is initialized in new(), nothing to do here
        Ok(())
    }

    async fn begin_transaction(&self) -> Result<Transaction> {
        Err(anyhow::anyhow!(HanzoDbError::NotImplemented(
            "Transactions not yet implemented for SQLite backend".to_string()
        )))
    }

    async fn optimize(&self) -> Result<()> {
        let conn = self.connection.clone();
        task::spawn_blocking(move || {
            let c = conn.lock()
                .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;
            c.execute("VACUUM", params![])
                .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
            c.execute("ANALYZE", params![])
                .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
            Ok::<(), HanzoDbError>(())
        }).await
            .map_err(|e| anyhow::anyhow!(e))??;
        Ok(())
    }

    async fn stats(&self) -> Result<DatabaseStats> {
        let conn = self.connection.clone();
        task::spawn_blocking(move || {
            let c = conn.lock()
                .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

            // Count tables
            let mut stmt = c.prepare("SELECT COUNT(*) FROM sqlite_master WHERE type='table'")
                .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
            let table_count: usize = stmt.query_row(params![], |row| row.get(0))
                .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

            // Get database page count and size
            let mut stmt = c.prepare("PRAGMA page_count")
                .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
            let page_count: usize = stmt.query_row(params![], |row| row.get(0))
                .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

            let mut stmt = c.prepare("PRAGMA page_size")
                .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
            let page_size: usize = stmt.query_row(params![], |row| row.get(0))
                .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

            Ok::<DatabaseStats, HanzoDbError>(DatabaseStats {
                backend: crate::DatabaseBackend::SQLite,
                table_count,
                total_rows: 0, // Would need to sum all tables
                total_size_bytes: page_count * page_size,
                index_count: 0,
                cache_hit_rate: 0.0,
            })
        }).await
            .map_err(|e| anyhow::anyhow!(e))?
    }
}

// Helper function for cosine similarity
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }

    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    dot_product / (norm_a * norm_b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sqlite_operations() {
        let config = HanzoDbConfig {
            backend: DatabaseBackend::SQLite,
            path: Some(PathBuf::from(":memory:")),
            ..Default::default()
        };

        let backend = SqliteBackend::new(config).unwrap();
        
        // Create table
        let schema = serde_json::json!({
            "name": "string",
            "age": "integer",
            "active": "boolean"
        });
        backend.create_table("users", &schema).await.unwrap();

        // Insert records
        let records = vec![
            serde_json::json!({"name": "Alice", "age": 30, "active": true}),
            serde_json::json!({"name": "Bob", "age": 25, "active": false}),
        ];
        backend.insert("users", &records).await.unwrap();

        // Query records
        let results = backend.query("SELECT * FROM users WHERE age > 20").await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0, 3.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);

        let c = vec![-1.0, -2.0, -3.0];
        assert!((cosine_similarity(&a, &c) + 1.0).abs() < 0.001);
    }
}