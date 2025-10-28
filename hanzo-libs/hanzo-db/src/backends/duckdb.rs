//! DuckDB backend implementation for analytical workloads
//! 
//! DuckDB is an embedded analytical database optimized for OLAP workloads.
//! It provides excellent performance for analytics, aggregations, and complex queries.

use crate::{DatabaseBackend, HanzoDbConfig, HanzoDbError, Result};
use async_trait::async_trait;
use duckdb::{Connection, params};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// DuckDB backend for analytical workloads
pub struct DuckDbBackend {
    connection: Arc<Mutex<Connection>>,
    path: PathBuf,
    config: HanzoDbConfig,
}

impl DuckDbBackend {
    /// Create a new DuckDB backend
    pub fn new(config: HanzoDbConfig) -> Result<Self> {
        let path = config.path.clone().unwrap_or_else(|| {
            PathBuf::from("./storage/hanzo-duckdb.db")
        });

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;
        }

        // Open DuckDB connection
        let connection = if path.to_str() == Some(":memory:") {
            Connection::open_in_memory()
        } else {
            Connection::open(&path)
        }.map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

        // Configure DuckDB for optimal performance
        connection.execute_batch("
            -- Set memory limit
            SET memory_limit='4GB';
            -- Enable parallel execution
            SET threads=4;
            -- Enable progress bar for long queries
            SET enable_progress_bar=true;
            -- Optimize for analytics
            SET enable_optimizer=true;
        ").map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            path,
            config,
        })
    }

    /// Execute analytical query
    pub fn analyze(&self, query: &str) -> Result<Vec<Value>> {
        let conn = self.connection.lock()
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

        let mut stmt = conn.prepare(query)
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        let rows = stmt.query_map(params![], |row| {
            // Convert row to JSON value
            let mut obj = serde_json::Map::new();
            
            // Get column count and names
            let column_count = row.column_count();
            for i in 0..column_count {
                let column_name = row.column_name(i)
                    .unwrap_or(&format!("column_{}", i))
                    .to_string();
                
                // Try to get value as different types
                if let Ok(val) = row.get::<_, String>(i) {
                    obj.insert(column_name, Value::String(val));
                } else if let Ok(val) = row.get::<_, i64>(i) {
                    obj.insert(column_name, Value::Number(val.into()));
                } else if let Ok(val) = row.get::<_, f64>(i) {
                    obj.insert(column_name, Value::from(val));
                } else if let Ok(val) = row.get::<_, bool>(i) {
                    obj.insert(column_name, Value::Bool(val));
                } else {
                    obj.insert(column_name, Value::Null);
                }
            }
            
            Ok(Value::Object(obj))
        }).map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))
    }

    /// Import Parquet file
    pub fn import_parquet(&self, path: &str, table_name: &str) -> Result<()> {
        let conn = self.connection.lock()
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

        let query = format!(
            "CREATE TABLE {} AS SELECT * FROM read_parquet('{}')",
            table_name, path
        );

        conn.execute(&query, params![])
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        Ok(())
    }

    /// Export to Parquet
    pub fn export_parquet(&self, table_name: &str, path: &str) -> Result<()> {
        let conn = self.connection.lock()
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

        let query = format!(
            "COPY {} TO '{}' (FORMAT PARQUET)",
            table_name, path
        );

        conn.execute(&query, params![])
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        Ok(())
    }

    /// Create aggregate view
    pub fn create_aggregate_view(&self, view_name: &str, query: &str) -> Result<()> {
        let conn = self.connection.lock()
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

        let create_view = format!(
            "CREATE VIEW {} AS {}",
            view_name, query
        );

        conn.execute(&create_view, params![])
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        Ok(())
    }
}

#[async_trait]
impl DatabaseBackend for DuckDbBackend {
    async fn connect(&mut self) -> Result<()> {
        // Already connected in new()
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        // Connection will be closed when dropped
        Ok(())
    }

    async fn create_table(&self, name: &str, schema: &Value) -> Result<()> {
        let conn = self.connection.lock()
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

        // Parse schema and create SQL
        let columns = schema.as_object()
            .ok_or_else(|| HanzoDbError::InvalidSchema("Schema must be object".to_string()))?;

        let column_defs: Vec<String> = columns.iter().map(|(name, type_def)| {
            let sql_type = match type_def.as_str() {
                Some("string") => "VARCHAR",
                Some("integer") => "INTEGER",
                Some("bigint") => "BIGINT",
                Some("float") => "REAL",
                Some("double") => "DOUBLE",
                Some("boolean") => "BOOLEAN",
                Some("timestamp") => "TIMESTAMP",
                Some("date") => "DATE",
                Some("json") => "JSON",
                Some("vector") => "DOUBLE[]",  // DuckDB array type for vectors
                _ => "VARCHAR",
            };
            format!("{} {}", name, sql_type)
        }).collect();

        let create_sql = format!(
            "CREATE TABLE IF NOT EXISTS {} ({})",
            name,
            column_defs.join(", ")
        );

        conn.execute(&create_sql, params![])
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        Ok(())
    }

    async fn drop_table(&self, name: &str) -> Result<()> {
        let conn = self.connection.lock()
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

        conn.execute(&format!("DROP TABLE IF EXISTS {}", name), params![])
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        Ok(())
    }

    async fn insert(&self, table: &str, records: &[Value]) -> Result<()> {
        let conn = self.connection.lock()
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

        // Begin transaction for bulk insert
        let tx = conn.unchecked_transaction()
            .map_err(|e| HanzoDbError::TransactionError(e.to_string()))?;

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

            // Convert values to DuckDB types
            let values: Vec<duckdb::types::Value> = obj.values().map(|v| {
                match v {
                    Value::String(s) => duckdb::types::Value::Text(s.clone()),
                    Value::Number(n) => {
                        if let Some(i) = n.as_i64() {
                            duckdb::types::Value::BigInt(i)
                        } else if let Some(f) = n.as_f64() {
                            duckdb::types::Value::Double(f)
                        } else {
                            duckdb::types::Value::Null
                        }
                    },
                    Value::Bool(b) => duckdb::types::Value::Boolean(*b),
                    Value::Null => duckdb::types::Value::Null,
                    Value::Array(a) => {
                        // Handle vector data
                        let floats: Vec<f64> = a.iter()
                            .filter_map(|v| v.as_f64())
                            .collect();
                        duckdb::types::Value::List(
                            floats.into_iter()
                                .map(duckdb::types::Value::Double)
                                .collect()
                        )
                    },
                    Value::Object(_) => {
                        // Store as JSON string
                        duckdb::types::Value::Text(v.to_string())
                    }
                }
            }).collect();

            tx.execute(&insert_sql, params_from_iter(values))
                .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;
        }

        tx.commit()
            .map_err(|e| HanzoDbError::TransactionError(e.to_string()))?;

        Ok(())
    }

    async fn query(&self, query: &str) -> Result<Vec<Value>> {
        self.analyze(query)
    }

    async fn update(&self, table: &str, id: &str, data: &Value) -> Result<()> {
        let conn = self.connection.lock()
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

        let obj = data.as_object()
            .ok_or_else(|| HanzoDbError::InvalidData("Update data must be object".to_string()))?;

        let set_clauses: Vec<String> = obj.keys()
            .enumerate()
            .map(|(i, key)| format!("{} = ${}", key, i + 2))
            .collect();

        let update_sql = format!(
            "UPDATE {} SET {} WHERE id = $1",
            table,
            set_clauses.join(", ")
        );

        let mut values = vec![duckdb::types::Value::Text(id.to_string())];
        for value in obj.values() {
            values.push(json_to_duckdb_value(value));
        }

        conn.execute(&update_sql, params_from_iter(values))
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        Ok(())
    }

    async fn delete(&self, table: &str, id: &str) -> Result<()> {
        let conn = self.connection.lock()
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

        conn.execute(
            &format!("DELETE FROM {} WHERE id = ?", table),
            params![id]
        ).map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        Ok(())
    }

    async fn vector_search(
        &self,
        table: &str,
        query_vector: &[f32],
        limit: usize,
        _filter: Option<&str>,
    ) -> Result<Vec<Value>> {
        // DuckDB doesn't have native vector similarity search
        // but we can implement it using array operations
        let conn = self.connection.lock()
            .map_err(|e| HanzoDbError::ConnectionError(e.to_string()))?;

        // Convert query vector to DuckDB array literal
        let vector_str = format!(
            "[{}]",
            query_vector.iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );

        // Calculate cosine similarity using array operations
        let query = format!(
            "SELECT *, 
             list_cosine_similarity(embedding, {}::DOUBLE[]) as similarity
             FROM {}
             ORDER BY similarity DESC
             LIMIT {}",
            vector_str, table, limit
        );

        let mut stmt = conn.prepare(&query)
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        let rows = stmt.query_map(params![], row_to_json)
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| HanzoDbError::QueryError(e.to_string()))
    }

    fn backend_type(&self) -> DatabaseBackend {
        DatabaseBackend::DuckDB
    }

    fn is_vector_capable(&self) -> bool {
        true // DuckDB can store vectors as arrays
    }

    fn is_analytical(&self) -> bool {
        true // DuckDB excels at analytics
    }
}

// Helper function to convert JSON to DuckDB value
fn json_to_duckdb_value(value: &Value) -> duckdb::types::Value {
    match value {
        Value::String(s) => duckdb::types::Value::Text(s.clone()),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                duckdb::types::Value::BigInt(i)
            } else if let Some(f) = n.as_f64() {
                duckdb::types::Value::Double(f)
            } else {
                duckdb::types::Value::Null
            }
        },
        Value::Bool(b) => duckdb::types::Value::Boolean(*b),
        Value::Null => duckdb::types::Value::Null,
        Value::Array(a) => {
            let values: Vec<duckdb::types::Value> = a.iter()
                .map(json_to_duckdb_value)
                .collect();
            duckdb::types::Value::List(values)
        },
        Value::Object(_) => duckdb::types::Value::Text(value.to_string()),
    }
}

// Helper function to convert row to JSON
fn row_to_json(row: &duckdb::Row) -> std::result::Result<Value, duckdb::Error> {
    let mut obj = serde_json::Map::new();
    
    let column_count = row.column_count();
    for i in 0..column_count {
        let column_name = row.column_name(i)
            .unwrap_or(&format!("column_{}", i))
            .to_string();
        
        // Try different types
        if let Ok(val) = row.get::<_, String>(i) {
            obj.insert(column_name, Value::String(val));
        } else if let Ok(val) = row.get::<_, i64>(i) {
            obj.insert(column_name, Value::Number(val.into()));
        } else if let Ok(val) = row.get::<_, f64>(i) {
            obj.insert(column_name, Value::from(val));
        } else if let Ok(val) = row.get::<_, bool>(i) {
            obj.insert(column_name, Value::Bool(val));
        } else {
            obj.insert(column_name, Value::Null);
        }
    }
    
    Ok(Value::Object(obj))
}

// Helper to create params from iterator
fn params_from_iter(values: Vec<duckdb::types::Value>) -> Vec<duckdb::types::Value> {
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_duckdb_analytics() {
        let config = HanzoDbConfig {
            backend: DatabaseBackend::DuckDB,
            path: Some(PathBuf::from(":memory:")),
            ..Default::default()
        };

        let backend = DuckDbBackend::new(config).unwrap();
        
        // Create table
        let schema = serde_json::json!({
            "id": "integer",
            "value": "double",
            "category": "string"
        });
        backend.create_table("metrics", &schema).await.unwrap();

        // Insert test data
        let records = vec![
            serde_json::json!({"id": 1, "value": 100.0, "category": "A"}),
            serde_json::json!({"id": 2, "value": 200.0, "category": "B"}),
            serde_json::json!({"id": 3, "value": 150.0, "category": "A"}),
        ];
        backend.insert("metrics", &records).await.unwrap();

        // Run analytical query
        let results = backend.analyze(
            "SELECT category, AVG(value) as avg_value FROM metrics GROUP BY category"
        ).unwrap();

        assert_eq!(results.len(), 2);
    }
}