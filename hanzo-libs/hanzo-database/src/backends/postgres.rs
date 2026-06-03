//! PostgreSQL backend implementation for transactional workloads
//! 
//! PostgreSQL is a powerful, open-source relational database with excellent
//! ACID compliance, rich SQL features, and extension support (including pgvector).

use crate::{DatabaseBackend, HanzoDbConfig, HanzoDbError, Result};
use async_trait::async_trait;
use serde_json::Value;
use sqlx::{postgres::{PgPoolOptions, PgPool}, Row};
use std::time::Duration;

/// PostgreSQL backend for transactional workloads
pub struct PostgresBackend {
    pool: PgPool,
    config: HanzoDbConfig,
}

impl PostgresBackend {
    /// Create a new PostgreSQL backend
    pub async fn new(config: HanzoDbConfig) -> Result<Self> {
        let database_url = config.url.as_ref()
            .ok_or_else(|| HanzoDbError::ConfigError(
                "PostgreSQL requires database URL".to_string()
            ))?;

        // Create connection pool
        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections.unwrap_or(10))
            .min_connections(config.min_connections.unwrap_or(2))
            .connect_timeout(Duration::from_secs(config.connect_timeout.unwrap_or(10)))
            .idle_timeout(Duration::from_secs(config.idle_timeout.unwrap_or(300)))
            .connect(database_url)
            .await
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

        // Test connection
        sqlx::query("SELECT 1")
            .fetch_one(&pool)
            .await
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

        Ok(Self { pool, config })
    }

    /// Enable pgvector extension
    pub async fn enable_vector_extension(&self) -> Result<()> {
        sqlx::query("CREATE EXTENSION IF NOT EXISTS vector")
            .execute(&self.pool)
            .await
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
        
        Ok(())
    }

    /// Create vector index for similarity search
    pub async fn create_vector_index(
        &self,
        table: &str,
        column: &str,
        dimensions: usize,
        index_type: &str, // "ivfflat" or "hnsw"
    ) -> Result<()> {
        let index_name = format!("{}_{}_idx", table, column);
        
        let query = match index_type {
            "ivfflat" => {
                format!(
                    "CREATE INDEX {} ON {} USING ivfflat ({} vector_l2_ops) WITH (lists = {})",
                    index_name, table, column, dimensions / 10
                )
            },
            "hnsw" => {
                format!(
                    "CREATE INDEX {} ON {} USING hnsw ({} vector_l2_ops)",
                    index_name, table, column
                )
            },
            _ => {
                return Err(HanzoDbError::InvalidSchema(
                    format!("Unknown index type: {}", index_type)
                ));
            }
        };

        sqlx::query(&query)
            .execute(&self.pool)
            .await
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        Ok(())
    }

    /// Begin transaction
    pub async fn begin_transaction(&self) -> Result<sqlx::Transaction<'_, sqlx::Postgres>> {
        self.pool.begin()
            .await
            .map_err(|e| HanzoDbError::TransactionError(e.to_string()))
    }

    /// Execute raw SQL with parameters
    pub async fn execute_raw(&self, query: &str, params: Vec<Value>) -> Result<u64> {
        let mut q = sqlx::query(query);
        
        for param in params {
            q = match param {
                Value::String(s) => q.bind(s),
                Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        q.bind(i)
                    } else if let Some(f) = n.as_f64() {
                        q.bind(f)
                    } else {
                        q.bind(Option::<i64>::None)
                    }
                },
                Value::Bool(b) => q.bind(b),
                Value::Null => q.bind(Option::<String>::None),
                _ => q.bind(param.to_string()),
            };
        }

        let result = q.execute(&self.pool)
            .await
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        Ok(result.rows_affected())
    }
}

#[async_trait]
impl DatabaseBackend for PostgresBackend {
    async fn connect(&mut self) -> Result<()> {
        // Already connected in new()
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.pool.close().await;
        Ok(())
    }

    async fn create_table(&self, name: &str, schema: &Value) -> Result<()> {
        let columns = schema.as_object()
            .ok_or_else(|| HanzoDbError::InvalidSchema("Schema must be object".to_string()))?;

        let mut column_defs = Vec::new();
        let mut has_vector = false;
        
        for (col_name, type_def) in columns {
            let sql_type = match type_def {
                Value::String(s) => match s.as_str() {
                    "string" => "TEXT",
                    "integer" => "INTEGER",
                    "bigint" => "BIGINT",
                    "float" => "REAL",
                    "double" => "DOUBLE PRECISION",
                    "boolean" => "BOOLEAN",
                    "timestamp" => "TIMESTAMP WITH TIME ZONE",
                    "date" => "DATE",
                    "json" | "jsonb" => "JSONB",
                    "uuid" => "UUID",
                    "vector" => {
                        has_vector = true;
                        "vector(1536)" // Default to OpenAI embedding size
                    },
                    t if t.starts_with("vector(") => {
                        has_vector = true;
                        t
                    },
                    _ => "TEXT",
                },
                Value::Object(obj) => {
                    // Handle complex type definitions
                    if let Some(Value::String(t)) = obj.get("type") {
                        match t.as_str() {
                            "vector" => {
                                has_vector = true;
                                if let Some(Value::Number(dim)) = obj.get("dimensions") {
                                    &format!("vector({})", dim)
                                } else {
                                    "vector(1536)"
                                }
                            },
                            _ => "TEXT",
                        }
                    } else {
                        "TEXT"
                    }
                },
                _ => "TEXT",
            };
            
            column_defs.push(format!("{} {}", col_name, sql_type));
        }

        // Enable pgvector if needed
        if has_vector {
            self.enable_vector_extension().await?;
        }

        // Add standard columns if not present
        if !columns.contains_key("id") {
            column_defs.insert(0, "id SERIAL PRIMARY KEY".to_string());
        }
        if !columns.contains_key("created_at") {
            column_defs.push("created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()".to_string());
        }
        if !columns.contains_key("updated_at") {
            column_defs.push("updated_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()".to_string());
        }

        let create_sql = format!(
            "CREATE TABLE IF NOT EXISTS {} ({})",
            name,
            column_defs.join(", ")
        );

        sqlx::query(&create_sql)
            .execute(&self.pool)
            .await
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        // Create update trigger for updated_at
        let trigger_sql = format!(
            "CREATE OR REPLACE TRIGGER update_{}_updated_at
             BEFORE UPDATE ON {}
             FOR EACH ROW
             EXECUTE FUNCTION update_updated_at_column()",
            name, name
        );

        // First create the function if it doesn't exist
        sqlx::query(
            "CREATE OR REPLACE FUNCTION update_updated_at_column()
             RETURNS TRIGGER AS $$
             BEGIN
                NEW.updated_at = NOW();
                RETURN NEW;
             END;
             $$ language 'plpgsql'"
        ).execute(&self.pool).await.ok(); // Ignore if already exists

        sqlx::query(&trigger_sql)
            .execute(&self.pool)
            .await
            .ok(); // Ignore if trigger already exists

        Ok(())
    }

    async fn drop_table(&self, name: &str) -> Result<()> {
        sqlx::query(&format!("DROP TABLE IF EXISTS {} CASCADE", name))
            .execute(&self.pool)
            .await
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        Ok(())
    }

    async fn insert(&self, table: &str, records: &[Value]) -> Result<()> {
        let mut tx = self.begin_transaction().await?;

        for record in records {
            let obj = record.as_object()
                .ok_or_else(|| HanzoDbError::InvalidData("Record must be object".to_string()))?;

            let columns: Vec<String> = obj.keys().cloned().collect();
            let placeholders: Vec<String> = (1..=columns.len())
                .map(|i| format!("${}", i))
                .collect();

            let insert_sql = format!(
                "INSERT INTO {} ({}) VALUES ({})",
                table,
                columns.join(", "),
                placeholders.join(", ")
            );

            let mut query = sqlx::query(&insert_sql);
            
            for value in obj.values() {
                query = match value {
                    Value::String(s) => query.bind(s),
                    Value::Number(n) => {
                        if let Some(i) = n.as_i64() {
                            query.bind(i)
                        } else if let Some(f) = n.as_f64() {
                            query.bind(f)
                        } else {
                            query.bind(Option::<i64>::None)
                        }
                    },
                    Value::Bool(b) => query.bind(b),
                    Value::Null => query.bind(Option::<String>::None),
                    Value::Array(a) => {
                        // Handle vector data
                        if a.iter().all(|v| v.is_number()) {
                            let floats: Vec<f32> = a.iter()
                                .filter_map(|v| v.as_f64().map(|f| f as f32))
                                .collect();
                            query.bind(floats)
                        } else {
                            query.bind(value.to_string())
                        }
                    },
                    Value::Object(_) => query.bind(value.to_string()),
                };
            }

            query.execute(&mut *tx)
                .await
                .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
        }

        tx.commit()
            .await
            .map_err(|e| HanzoDbError::TransactionError(e.to_string()))?;

        Ok(())
    }

    async fn query(&self, query: &str) -> Result<Vec<Value>> {
        let rows = sqlx::query(query)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        let mut results = Vec::new();
        
        for row in rows {
            let mut obj = serde_json::Map::new();
            
            // Get column information
            for (i, column) in row.columns().iter().enumerate() {
                let name = column.name();
                
                // Try to get value as different types
                if let Ok(val) = row.try_get::<String, _>(i) {
                    obj.insert(name.to_string(), Value::String(val));
                } else if let Ok(val) = row.try_get::<i64, _>(i) {
                    obj.insert(name.to_string(), Value::Number(val.into()));
                } else if let Ok(val) = row.try_get::<i32, _>(i) {
                    obj.insert(name.to_string(), Value::Number(val.into()));
                } else if let Ok(val) = row.try_get::<f64, _>(i) {
                    obj.insert(name.to_string(), Value::from(val));
                } else if let Ok(val) = row.try_get::<f32, _>(i) {
                    obj.insert(name.to_string(), Value::from(val as f64));
                } else if let Ok(val) = row.try_get::<bool, _>(i) {
                    obj.insert(name.to_string(), Value::Bool(val));
                } else if let Ok(val) = row.try_get::<serde_json::Value, _>(i) {
                    obj.insert(name.to_string(), val);
                } else if let Ok(val) = row.try_get::<Vec<f32>, _>(i) {
                    // Handle vector data
                    let array: Vec<Value> = val.iter().map(|f| Value::from(*f as f64)).collect();
                    obj.insert(name.to_string(), Value::Array(array));
                } else {
                    obj.insert(name.to_string(), Value::Null);
                }
            }
            
            results.push(Value::Object(obj));
        }

        Ok(results)
    }

    async fn update(&self, table: &str, id: &str, data: &Value) -> Result<()> {
        let obj = data.as_object()
            .ok_or_else(|| HanzoDbError::InvalidData("Update data must be object".to_string()))?;

        let set_clauses: Vec<String> = obj.keys()
            .enumerate()
            .map(|(i, key)| format!("{} = ${}", key, i + 2))
            .collect();

        let update_sql = format!(
            "UPDATE {} SET {}, updated_at = NOW() WHERE id = $1",
            table,
            set_clauses.join(", ")
        );

        let mut query = sqlx::query(&update_sql).bind(id);
        
        for value in obj.values() {
            query = match value {
                Value::String(s) => query.bind(s),
                Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        query.bind(i)
                    } else if let Some(f) = n.as_f64() {
                        query.bind(f)
                    } else {
                        query.bind(Option::<i64>::None)
                    }
                },
                Value::Bool(b) => query.bind(b),
                Value::Null => query.bind(Option::<String>::None),
                _ => query.bind(value.to_string()),
            };
        }

        query.execute(&self.pool)
            .await
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        Ok(())
    }

    async fn delete(&self, table: &str, id: &str) -> Result<()> {
        sqlx::query(&format!("DELETE FROM {} WHERE id = $1", table))
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        Ok(())
    }

    async fn vector_search(
        &self,
        table: &str,
        query_vector: &[f32],
        limit: usize,
        filter: Option<&str>,
    ) -> Result<Vec<Value>> {
        // Convert vector to PostgreSQL array format
        let vector_str = format!(
            "[{}]",
            query_vector.iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        let where_clause = if let Some(f) = filter {
            format!("WHERE {}", f)
        } else {
            String::new()
        };

        // Use pgvector's <-> operator for L2 distance
        let query = format!(
            "SELECT *, 
             embedding <-> '{}' as distance
             FROM {}
             {}
             ORDER BY embedding <-> '{}'
             LIMIT {}",
            vector_str, table, where_clause, vector_str, limit
        );

        self.query(&query).await
    }

    fn backend_type(&self) -> DatabaseBackend {
        DatabaseBackend::PostgreSQL
    }

    fn is_vector_capable(&self) -> bool {
        true // With pgvector extension
    }

    fn is_transactional(&self) -> bool {
        true // PostgreSQL has excellent ACID compliance
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_postgres_transactions() {
        // This test requires a PostgreSQL instance
        // Skip if not available
        let database_url = std::env::var("TEST_POSTGRES_URL")
            .unwrap_or_else(|_| "postgresql://postgres:postgres@localhost/test".to_string());

        let config = HanzoDbConfig {
            backend: DatabaseBackend::PostgreSQL,
            url: Some(database_url),
            ..Default::default()
        };

        let backend = match PostgresBackend::new(config).await {
            Ok(b) => b,
            Err(_) => {
                println!("PostgreSQL not available, skipping test");
                return;
            }
        };

        // Create table with vector support
        let schema = serde_json::json!({
            "name": "string",
            "embedding": {
                "type": "vector",
                "dimensions": 384
            }
        });
        
        backend.create_table("test_items", &schema).await.unwrap();
        
        // Test transaction
        let mut tx = backend.begin_transaction().await.unwrap();
        
        sqlx::query("INSERT INTO test_items (name) VALUES ($1)")
            .bind("test")
            .execute(&mut *tx)
            .await
            .unwrap();
        
        tx.commit().await.unwrap();

        // Clean up
        backend.drop_table("test_items").await.unwrap();
    }
}