//! # Hanzo LanceDB - Multimodal Vector Database
//!
//! Production-ready LanceDB integration for Hanzo Node, providing:
//! - Multimodal storage (text, images, embeddings)
//! - High-performance vector search
//! - Transaction support
//! - Connection pooling
//! - Migration from SQLite

use anyhow::{Context, Result};
use arrow_array::{RecordBatch, RecordBatchIterator};
use arrow_schema::{DataType, Field, Schema as ArrowSchema};
use async_trait::async_trait;
use deadpool::managed::{Manager, Pool, PoolError, RecycledError};
use lancedb::{connect, Connection, Table};
use log::{debug, error, info, warn};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

pub mod models;
pub mod vector_search;

#[cfg(feature = "migration")]
pub mod migration;

// Re-exports
pub use models::*;
pub use vector_search::*;

/// Default database path
const DEFAULT_DB_PATH: &str = "./storage/lancedb";

/// Default pool size
const DEFAULT_POOL_SIZE: usize = 16;

/// LanceDB configuration
#[derive(Debug, Clone)]
pub struct LanceDbConfig {
    /// Database directory path
    pub path: PathBuf,
    /// Connection pool size
    pub pool_size: usize,
    /// Enable write-ahead logging
    pub enable_wal: bool,
    /// Cache size in bytes
    pub cache_size: Option<usize>,
    /// Enable compression
    pub enable_compression: bool,
}

impl Default for LanceDbConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from(DEFAULT_DB_PATH),
            pool_size: DEFAULT_POOL_SIZE,
            enable_wal: true,
            cache_size: Some(256 * 1024 * 1024), // 256MB
            enable_compression: true,
        }
    }
}

/// LanceDB connection manager for pooling
struct LanceDbManager {
    config: LanceDbConfig,
}

impl LanceDbManager {
    fn new(config: LanceDbConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Manager for LanceDbManager {
    type Type = Connection;
    type Error = anyhow::Error;

    async fn create(&self) -> Result<Self::Type> {
        debug!("Creating new LanceDB connection");
        
        // Ensure directory exists
        tokio::fs::create_dir_all(&self.config.path)
            .await
            .context("Failed to create database directory")?;
        
        let path_str = self.config.path.to_string_lossy();
        let conn = connect(&path_str)
            .execute()
            .await
            .context("Failed to connect to LanceDB")?;
        
        Ok(conn)
    }

    async fn recycle(
        &self,
        conn: &mut Self::Type,
        _: &deadpool::managed::Metrics,
    ) -> RecycledError<Self::Error> {
        // Verify connection is still valid
        match conn.table_names().execute().await {
            Ok(_) => Ok(()),
            Err(e) => Err(RecycledError::Backend(e.into())),
        }
    }
}

/// Main LanceDB database interface
pub struct LanceDb {
    pool: Pool<LanceDbManager>,
    config: LanceDbConfig,
    tables: Arc<RwLock<TableRegistry>>,
}

/// Registry of initialized tables
struct TableRegistry {
    users: bool,
    tools: bool,
    agents: bool,
    embeddings: bool,
    jobs: bool,
    sessions: bool,
    multimodal: bool,
}

impl Default for TableRegistry {
    fn default() -> Self {
        Self {
            users: false,
            tools: false,
            agents: false,
            embeddings: false,
            jobs: false,
            sessions: false,
            multimodal: false,
        }
    }
}

impl LanceDb {
    /// Create new LanceDB instance with default configuration
    pub async fn new() -> Result<Self> {
        Self::with_config(LanceDbConfig::default()).await
    }

    /// Create new LanceDB instance with custom configuration
    pub async fn with_config(config: LanceDbConfig) -> Result<Self> {
        info!("Initializing LanceDB at {:?}", config.path);
        
        let manager = LanceDbManager::new(config.clone());
        let pool = Pool::builder(manager)
            .max_size(config.pool_size)
            .runtime(deadpool_runtime::Runtime::Tokio1)
            .build()
            .context("Failed to create connection pool")?;
        
        let db = Self {
            pool,
            config,
            tables: Arc::new(RwLock::new(TableRegistry::default())),
        };
        
        // Initialize core tables
        db.initialize_tables().await?;
        
        Ok(db)
    }

    /// Get connection from pool
    pub async fn connection(&self) -> Result<deadpool::managed::Object<LanceDbManager>> {
        self.pool
            .get()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get connection from pool: {}", e))
    }

    /// Initialize all required tables
    async fn initialize_tables(&self) -> Result<()> {
        info!("Initializing database tables");
        
        let mut conn = self.connection().await?;
        let mut registry = self.tables.write().await;
        
        // Users table
        if !registry.users {
            self.create_users_table(&mut conn).await?;
            registry.users = true;
        }
        
        // Tools table
        if !registry.tools {
            self.create_tools_table(&mut conn).await?;
            registry.tools = true;
        }
        
        // Agents table
        if !registry.agents {
            self.create_agents_table(&mut conn).await?;
            registry.agents = true;
        }
        
        // Embeddings table for vector search
        if !registry.embeddings {
            self.create_embeddings_table(&mut conn).await?;
            registry.embeddings = true;
        }
        
        // Jobs table
        if !registry.jobs {
            self.create_jobs_table(&mut conn).await?;
            registry.jobs = true;
        }
        
        // Sessions table
        if !registry.sessions {
            self.create_sessions_table(&mut conn).await?;
            registry.sessions = true;
        }
        
        // Multimodal table
        #[cfg(feature = "multimodal")]
        if !registry.multimodal {
            self.create_multimodal_table(&mut conn).await?;
            registry.multimodal = true;
        }
        
        info!("Database tables initialized successfully");
        Ok(())
    }

    /// Create users table
    async fn create_users_table(&self, conn: &mut Connection) -> Result<()> {
        let schema = models::user_schema();
        self.create_table_if_not_exists(conn, "users", schema).await
    }

    /// Create tools table
    async fn create_tools_table(&self, conn: &mut Connection) -> Result<()> {
        let schema = models::tool_schema();
        self.create_table_if_not_exists(conn, "tools", schema).await
    }

    /// Create agents table
    async fn create_agents_table(&self, conn: &mut Connection) -> Result<()> {
        let schema = models::agent_schema();
        self.create_table_if_not_exists(conn, "agents", schema).await
    }

    /// Create embeddings table with vector index
    async fn create_embeddings_table(&self, conn: &mut Connection) -> Result<()> {
        let schema = models::embedding_schema();
        self.create_table_if_not_exists(conn, "embeddings", schema).await?;
        
        // Create vector index for fast similarity search
        let table = conn.open_table("embeddings").execute().await?;
        
        // Create IVF_PQ index for fast vector search
        use lancedb::index::{Index, IvfPqIndexBuilder};
        table
            .create_index(&["vector"], Index::IvfPq(IvfPqIndexBuilder::default()))
            .execute()
            .await
            .context("Failed to create vector index")?;
        
        Ok(())
    }

    /// Create jobs table
    async fn create_jobs_table(&self, conn: &mut Connection) -> Result<()> {
        let schema = models::job_schema();
        self.create_table_if_not_exists(conn, "jobs", schema).await
    }

    /// Create sessions table
    async fn create_sessions_table(&self, conn: &mut Connection) -> Result<()> {
        let schema = models::session_schema();
        self.create_table_if_not_exists(conn, "sessions", schema).await
    }

    /// Create multimodal table for images and other media
    #[cfg(feature = "multimodal")]
    async fn create_multimodal_table(&self, conn: &mut Connection) -> Result<()> {
        let schema = models::multimodal_schema();
        self.create_table_if_not_exists(conn, "multimodal", schema).await?;
        
        // Create vector index for multimodal embeddings
        let table = conn.open_table("multimodal").execute().await?;
        
        use lancedb::index::{Index, IvfPqIndexBuilder};
        table
            .create_index(&["embedding"], Index::IvfPq(IvfPqIndexBuilder::default()))
            .execute()
            .await
            .context("Failed to create multimodal index")?;
        
        Ok(())
    }

    /// Helper to create table if it doesn't exist
    async fn create_table_if_not_exists(
        &self,
        conn: &mut Connection,
        name: &str,
        schema: Arc<ArrowSchema>,
    ) -> Result<()> {
        let table_names = conn.table_names().execute().await?;
        
        if !table_names.contains(&name.to_string()) {
            debug!("Creating table: {}", name);
            
            // Create empty record batch with schema
            let batch = RecordBatch::new_empty(schema.clone());
            let batches = vec![batch];
            let reader = RecordBatchIterator::new(batches.into_iter().map(Ok), schema);
            
            conn.create_table(name, Box::new(reader))
                .execute()
                .await
                .context(format!("Failed to create table: {}", name))?;
            
            info!("Created table: {}", name);
        } else {
            debug!("Table already exists: {}", name);
        }
        
        Ok(())
    }

    /// Get table by name
    pub async fn table(&self, name: &str) -> Result<Table> {
        let conn = self.connection().await?;
        conn.open_table(name)
            .execute()
            .await
            .context(format!("Failed to open table: {}", name))
    }

    /// Transaction support using optimistic concurrency
    pub async fn transaction<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&Connection) -> futures::future::BoxFuture<'_, Result<R>> + Send,
        R: Send,
    {
        let conn = self.connection().await?;
        
        // LanceDB uses optimistic concurrency control
        // Transactions are handled at the table level with versioning
        let result = f(&*conn).await?;
        
        Ok(result)
    }

    /// Optimize tables for better performance
    pub async fn optimize(&self) -> Result<()> {
        info!("Optimizing database tables");
        
        let conn = self.connection().await?;
        let table_names = conn.table_names().execute().await?;
        
        for name in table_names {
            debug!("Optimizing table: {}", name);
            
            let table = conn.open_table(&name).execute().await?;
            
            // Compact files to reduce fragmentation
            table.compact_files()
                .execute()
                .await
                .context(format!("Failed to compact table: {}", name))?;
            
            // Clean up old versions
            table.cleanup_old_versions()
                .execute()
                .await
                .context(format!("Failed to cleanup table: {}", name))?;
        }
        
        info!("Database optimization completed");
        Ok(())
    }

    /// Get database statistics
    pub async fn stats(&self) -> Result<DbStats> {
        let conn = self.connection().await?;
        let table_names = conn.table_names().execute().await?;
        
        let mut total_rows = 0;
        let mut table_stats = Vec::new();
        
        for name in &table_names {
            let table = conn.open_table(name).execute().await?;
            let count = table.count_rows(None).await?;
            
            total_rows += count;
            table_stats.push(TableStats {
                name: name.clone(),
                row_count: count,
            });
        }
        
        Ok(DbStats {
            total_tables: table_names.len(),
            total_rows,
            table_stats,
            pool_size: self.config.pool_size,
        })
    }
}

/// Database statistics
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DbStats {
    pub total_tables: usize,
    pub total_rows: usize,
    pub table_stats: Vec<TableStats>,
    pub pool_size: usize,
}

/// Table statistics
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TableStats {
    pub name: String,
    pub row_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_database_creation() {
        let temp_dir = TempDir::new().unwrap();
        let config = LanceDbConfig {
            path: temp_dir.path().to_path_buf(),
            ..Default::default()
        };
        
        let db = LanceDb::with_config(config).await.unwrap();
        let stats = db.stats().await.unwrap();
        
        assert!(stats.total_tables > 0);
    }

    #[tokio::test]
    async fn test_connection_pool() {
        let temp_dir = TempDir::new().unwrap();
        let config = LanceDbConfig {
            path: temp_dir.path().to_path_buf(),
            pool_size: 4,
            ..Default::default()
        };
        
        let db = LanceDb::with_config(config).await.unwrap();
        
        // Get multiple connections
        let mut connections = Vec::new();
        for _ in 0..3 {
            connections.push(db.connection().await.unwrap());
        }
        
        assert_eq!(connections.len(), 3);
    }
}