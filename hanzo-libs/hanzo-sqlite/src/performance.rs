//! Performance optimizations for SQLite operations
//!
//! This module provides optimized database configurations and query patterns
//! to eliminate bottlenecks in the Hanzo node database layer.

use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{Connection, Result, Row, ToSql, params};
use std::sync::Arc;
use std::time::Duration;
use log::{debug, warn};

/// Optimized pool configuration for maximum performance
pub struct OptimizedPoolConfig {
    /// Maximum number of connections in the pool
    pub max_size: u32,
    /// Minimum number of idle connections to maintain
    pub min_idle: Option<u32>,
    /// Maximum lifetime of a connection
    pub max_lifetime: Option<Duration>,
    /// Time to wait for a connection before timing out
    pub connection_timeout: Duration,
    /// How often to run the idle connection reaper
    pub idle_timeout: Option<Duration>,
    /// Test connections on checkout
    pub test_on_check_out: bool,
}

impl Default for OptimizedPoolConfig {
    fn default() -> Self {
        Self {
            // MATRIX MODE: Aggressive pooling for maximum throughput
            max_size: 32,  // Increased from 10 for better parallelism
            min_idle: Some(8),  // Keep warm connections ready
            max_lifetime: Some(Duration::from_secs(300)),  // 5 minutes
            connection_timeout: Duration::from_secs(10),  // Reduced from 60
            idle_timeout: Some(Duration::from_secs(60)),  // Clean up idle connections
            test_on_check_out: false,  // Skip test for speed (rely on retries)
        }
    }
}

/// Create an optimized connection pool
pub fn create_optimized_pool(
    db_path: &str,
    config: OptimizedPoolConfig,
) -> Result<Pool<SqliteConnectionManager>, Box<dyn std::error::Error>> {
    let manager = SqliteConnectionManager::file(db_path);

    let mut builder = Pool::builder()
        .max_size(config.max_size)
        .connection_timeout(config.connection_timeout)
        .test_on_check_out(config.test_on_check_out);

    if let Some(min_idle) = config.min_idle {
        builder = builder.min_idle(Some(min_idle));
    }

    if let Some(max_lifetime) = config.max_lifetime {
        builder = builder.max_lifetime(Some(max_lifetime));
    }

    if let Some(idle_timeout) = config.idle_timeout {
        builder = builder.idle_timeout(Some(idle_timeout));
    }

    let pool = builder.build(manager)?;

    // Pre-warm the pool by creating minimum idle connections
    if let Some(min_idle) = config.min_idle {
        for _ in 0..min_idle {
            let conn = pool.get()?;
            optimize_connection(&conn)?;
            drop(conn); // Return to pool
        }
    }

    Ok(pool)
}

/// Optimize a SQLite connection for maximum performance
pub fn optimize_connection(conn: &Connection) -> Result<()> {
    // MATRIX MODE: Bend reality for speed
    conn.execute_batch(
        "
        -- Write-Ahead Logging for concurrent reads
        PRAGMA journal_mode = WAL;

        -- Synchronous mode: NORMAL is faster than FULL but still safe
        PRAGMA synchronous = NORMAL;

        -- Use memory for temporary tables
        PRAGMA temp_store = MEMORY;

        -- Increase cache size to 100MB (negative = KB)
        PRAGMA cache_size = -100000;

        -- Memory-mapped I/O for 512MB
        PRAGMA mmap_size = 536870912;

        -- Page size optimization (must be set before any tables are created)
        -- PRAGMA page_size = 8192;

        -- WAL autocheckpoint every 1000 pages (4MB with 4KB pages)
        PRAGMA wal_autocheckpoint = 1000;

        -- Busy timeout to handle contention
        PRAGMA busy_timeout = 5000;

        -- Foreign keys for integrity
        PRAGMA foreign_keys = ON;

        -- Query planner optimizations
        PRAGMA optimize;
        "
    )?;

    debug!("⚡ SQLite connection optimized for MATRIX-level performance");
    Ok(())
}

/// Batch insert optimization using prepared statements
pub struct BatchInserter<'a> {
    conn: &'a mut Connection,
    batch_size: usize,
}

impl<'a> BatchInserter<'a> {
    pub fn new(conn: &'a mut Connection, batch_size: usize) -> Self {
        Self { conn, batch_size }
    }

    /// Perform batch insert with optimal transaction handling
    pub fn insert<T, F>(
        &mut self,
        table: &str,
        columns: &[&str],
        data: Vec<T>,
        mut bind_fn: F,
    ) -> Result<usize>
    where
        F: FnMut(&T) -> Vec<Box<dyn ToSql>>,
    {
        if data.is_empty() {
            return Ok(0);
        }

        let placeholders = (1..=columns.len())
            .map(|i| format!("?{}", i))
            .collect::<Vec<_>>()
            .join(", ");

        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            table,
            columns.join(", "),
            placeholders
        );

        let mut total_inserted = 0;

        // Process in batches with transactions
        for chunk in data.chunks(self.batch_size) {
            let tx = self.conn.transaction()?;
            {
                let mut stmt = tx.prepare_cached(&sql)?;

                for item in chunk {
                    let params = bind_fn(item);
                    let param_refs: Vec<&dyn ToSql> = params
                        .iter()
                        .map(|p| p.as_ref() as &dyn ToSql)
                        .collect();
                    stmt.execute(&param_refs[..])?;
                    total_inserted += 1;
                }
            }
            tx.commit()?;
        }

        Ok(total_inserted)
    }
}

/// Optimized query builder to reduce allocations
pub struct QueryBuilder {
    base_query: String,
    conditions: Vec<String>,
    parameters: Vec<Box<dyn ToSql>>,
    order_by: Option<String>,
    limit: Option<usize>,
}

impl QueryBuilder {
    pub fn new(base_query: impl Into<String>) -> Self {
        Self {
            base_query: base_query.into(),
            conditions: Vec::new(),
            parameters: Vec::new(),
            order_by: None,
            limit: None,
        }
    }

    pub fn add_condition(mut self, condition: impl Into<String>) -> Self {
        self.conditions.push(condition.into());
        self
    }

    pub fn add_parameter<T: ToSql + 'static>(mut self, param: T) -> Self {
        self.parameters.push(Box::new(param));
        self
    }

    pub fn order_by(mut self, order: impl Into<String>) -> Self {
        self.order_by = Some(order.into());
        self
    }

    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn build(self) -> (String, Vec<Box<dyn ToSql>>) {
        let mut query = self.base_query;

        if !self.conditions.is_empty() {
            query.push_str(" WHERE ");
            query.push_str(&self.conditions.join(" AND "));
        }

        if let Some(order) = self.order_by {
            query.push_str(" ORDER BY ");
            query.push_str(&order);
        }

        if let Some(limit) = self.limit {
            query.push_str(&format!(" LIMIT {}", limit));
        }

        (query, self.parameters)
    }
}

/// Connection pool with automatic retry logic
pub struct ResilientPool {
    pool: Arc<Pool<SqliteConnectionManager>>,
    max_retries: u32,
    retry_delay: Duration,
}

impl ResilientPool {
    pub fn new(pool: Pool<SqliteConnectionManager>) -> Self {
        Self {
            pool: Arc::new(pool),
            max_retries: 3,
            retry_delay: Duration::from_millis(100),
        }
    }

    /// Get a connection with automatic retry on failure
    pub async fn get(&self) -> Result<PooledConnection<SqliteConnectionManager>, Box<dyn std::error::Error>> {
        let mut attempts = 0;

        loop {
            match self.pool.get() {
                Ok(conn) => return Ok(conn),
                Err(e) if attempts < self.max_retries => {
                    warn!("Failed to get connection (attempt {}): {}", attempts + 1, e);
                    tokio::time::sleep(self.retry_delay).await;
                    attempts += 1;
                }
                Err(e) => return Err(Box::new(e)),
            }
        }
    }

    /// Execute a query with automatic retry
    pub async fn execute_with_retry<F, R>(&self, f: F) -> Result<R, Box<dyn std::error::Error>>
    where
        F: Fn(&Connection) -> Result<R>,
    {
        let conn = self.get().await?;

        let mut attempts = 0;
        loop {
            match f(&conn) {
                Ok(result) => return Ok(result),
                Err(e) if attempts < self.max_retries => {
                    warn!("Query failed (attempt {}): {}", attempts + 1, e);
                    tokio::time::sleep(self.retry_delay).await;
                    attempts += 1;
                }
                Err(e) => return Err(Box::new(e)),
            }
        }
    }
}

/// Index optimizer - analyzes query patterns and suggests indexes
pub struct IndexOptimizer;

impl IndexOptimizer {
    /// Analyze slow queries and suggest indexes
    pub fn analyze_and_suggest(conn: &Connection) -> Result<Vec<String>> {
        let mut suggestions = Vec::new();

        // Check for missing indexes on foreign keys
        let mut stmt = conn.prepare(
            "
            SELECT
                m.name AS table_name,
                p.name AS column_name
            FROM
                sqlite_master AS m,
                pragma_table_info(m.name) AS p
            WHERE
                m.type = 'table'
                AND p.pk = 0
                AND p.name LIKE '%_id'
                AND NOT EXISTS (
                    SELECT 1 FROM sqlite_master AS idx
                    WHERE idx.type = 'index'
                    AND idx.tbl_name = m.name
                    AND idx.sql LIKE '%' || p.name || '%'
                )
            "
        )?;

        let missing_indexes = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
            ))
        })?;

        for index in missing_indexes {
            if let Ok((table, column)) = index {
                suggestions.push(format!(
                    "CREATE INDEX idx_{}_{} ON {} ({});",
                    table, column, table, column
                ));
            }
        }

        // Check for tables without any indexes
        let mut stmt = conn.prepare(
            "
            SELECT name
            FROM sqlite_master
            WHERE type = 'table'
            AND name NOT LIKE 'sqlite_%'
            AND NOT EXISTS (
                SELECT 1 FROM sqlite_master
                WHERE type = 'index'
                AND tbl_name = sqlite_master.name
            )
            "
        )?;

        let unindexed_tables = stmt.query_map([], |row| {
            row.get::<_, String>(0)
        })?;

        for table in unindexed_tables {
            if let Ok(table_name) = table {
                suggestions.push(format!(
                    "-- Consider adding indexes to table: {}",
                    table_name
                ));
            }
        }

        Ok(suggestions)
    }

    /// Create recommended indexes
    pub fn create_standard_indexes(conn: &Connection) -> Result<()> {
        // Create indexes for common query patterns
        let indexes = vec![
            // Job management
            "CREATE INDEX IF NOT EXISTS idx_jobs_status ON jobs (job_status)",
            "CREATE INDEX IF NOT EXISTS idx_jobs_created ON jobs (created_at)",
            "CREATE INDEX IF NOT EXISTS idx_jobs_agent ON jobs (agent_id)",

            // Tool execution
            "CREATE INDEX IF NOT EXISTS idx_tools_key ON hanzo_tools (tool_router_key)",
            "CREATE INDEX IF NOT EXISTS idx_tools_type ON hanzo_tools (tool_type)",

            // Messages and inbox
            "CREATE INDEX IF NOT EXISTS idx_inbox_hash ON inbox (hash_key)",
            "CREATE INDEX IF NOT EXISTS idx_inbox_parent ON inbox (parent_hash_key)",

            // Cron tasks
            "CREATE INDEX IF NOT EXISTS idx_cron_next ON cron_tasks (next_execution)",
            "CREATE INDEX IF NOT EXISTS idx_cron_enabled ON cron_tasks (enabled)",

            // Vector embeddings
            "CREATE INDEX IF NOT EXISTS idx_embeddings_created ON vector_embeddings (created_at)",

            // OAuth tokens
            "CREATE INDEX IF NOT EXISTS idx_oauth_tool ON oauth_tokens (tool_key)",
        ];

        for index_sql in indexes {
            conn.execute(index_sql, [])?;
        }

        debug!("✅ Standard indexes created for optimal query performance");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_optimized_pool_creation() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_str().unwrap().to_string();

        let config = OptimizedPoolConfig::default();
        let pool = create_optimized_pool(&db_path, config).unwrap();

        // Test that we can get connections
        let conn = pool.get().unwrap();
        conn.execute("SELECT 1", []).unwrap();
    }

    #[test]
    fn test_query_builder() {
        let (query, _params) = QueryBuilder::new("SELECT * FROM users")
            .add_condition("age > ?1")
            .add_parameter(18)
            .add_condition("status = ?2")
            .add_parameter("active")
            .order_by("created_at DESC")
            .limit(10)
            .build();

        assert_eq!(
            query,
            "SELECT * FROM users WHERE age > ?1 AND status = ?2 ORDER BY created_at DESC LIMIT 10"
        );
    }

    #[tokio::test]
    async fn test_resilient_pool() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db").to_str().unwrap().to_string();

        let config = OptimizedPoolConfig::default();
        let pool = create_optimized_pool(&db_path, config).unwrap();
        let resilient = ResilientPool::new(pool);

        let result = resilient.execute_with_retry(|conn| {
            conn.execute("SELECT 1", [])
        }).await;

        assert!(result.is_ok());
    }
}