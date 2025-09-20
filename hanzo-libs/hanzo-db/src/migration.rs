//! # SQLite to LanceDB Migration Module
//!
//! Provides tools to migrate existing SQLite databases to LanceDB.

use anyhow::{Context, Result};
use arrow_array::{
    BooleanArray, Float32Array, Int64Array, RecordBatch, StringArray, TimestampMillisecondArray,
    ArrayRef, FixedSizeListArray,
};
use arrow_schema::{DataType, Field, Schema as ArrowSchema};
use chrono::{DateTime, NaiveDateTime, Utc};
use lancedb::Connection;
use log::{debug, error, info, warn};
use rusqlite::{params, Connection as SqliteConnection, Row};
use serde_json;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;

use crate::models::*;
use crate::LanceDb;

/// Migration configuration
#[derive(Debug, Clone)]
pub struct MigrationConfig {
    /// Source SQLite database path
    pub sqlite_path: String,
    /// Target LanceDB instance
    pub target_db: Arc<LanceDb>,
    /// Batch size for migration
    pub batch_size: usize,
    /// Skip existing tables
    pub skip_existing: bool,
    /// Verify data after migration
    pub verify: bool,
}

impl Default for MigrationConfig {
    fn default() -> Self {
        Self {
            sqlite_path: "./storage/db.sqlite".to_string(),
            target_db: Arc::new(LanceDb::new().expect("Failed to create LanceDB")),
            batch_size: 1000,
            skip_existing: false,
            verify: true,
        }
    }
}

/// SQLite to LanceDB migrator
pub struct Migrator {
    config: MigrationConfig,
    sqlite_conn: SqliteConnection,
}

impl Migrator {
    /// Create new migrator instance
    pub fn new(config: MigrationConfig) -> Result<Self> {
        let sqlite_conn = SqliteConnection::open(&config.sqlite_path)
            .context("Failed to open SQLite database")?;
        
        Ok(Self {
            config,
            sqlite_conn,
        })
    }

    /// Run full migration
    pub async fn migrate_all(&mut self) -> Result<MigrationStats> {
        info!("Starting SQLite to LanceDB migration");
        let mut stats = MigrationStats::default();
        
        // Migrate users
        match self.migrate_users().await {
            Ok(count) => {
                info!("Migrated {} users", count);
                stats.users_migrated = count;
            }
            Err(e) => {
                error!("Failed to migrate users: {}", e);
                stats.errors.push(format!("Users: {}", e));
            }
        }
        
        // Migrate tools
        match self.migrate_tools().await {
            Ok(count) => {
                info!("Migrated {} tools", count);
                stats.tools_migrated = count;
            }
            Err(e) => {
                error!("Failed to migrate tools: {}", e);
                stats.errors.push(format!("Tools: {}", e));
            }
        }
        
        // Migrate agents
        match self.migrate_agents().await {
            Ok(count) => {
                info!("Migrated {} agents", count);
                stats.agents_migrated = count;
            }
            Err(e) => {
                error!("Failed to migrate agents: {}", e);
                stats.errors.push(format!("Agents: {}", e));
            }
        }
        
        // Migrate embeddings
        match self.migrate_embeddings().await {
            Ok(count) => {
                info!("Migrated {} embeddings", count);
                stats.embeddings_migrated = count;
            }
            Err(e) => {
                error!("Failed to migrate embeddings: {}", e);
                stats.errors.push(format!("Embeddings: {}", e));
            }
        }
        
        // Migrate jobs
        match self.migrate_jobs().await {
            Ok(count) => {
                info!("Migrated {} jobs", count);
                stats.jobs_migrated = count;
            }
            Err(e) => {
                error!("Failed to migrate jobs: {}", e);
                stats.errors.push(format!("Jobs: {}", e));
            }
        }
        
        // Verify if requested
        if self.config.verify {
            self.verify_migration(&stats).await?;
        }
        
        info!("Migration completed: {:?}", stats);
        Ok(stats)
    }

    /// Migrate users table
    async fn migrate_users(&mut self) -> Result<usize> {
        info!("Migrating users table");
        
        let mut stmt = self.sqlite_conn.prepare(
            "SELECT id, profile_name, identity_type, identity_public_key, 
             encryption_public_key, signature_public_key, 
             node_signature_public_key, node_encryption_public_key,
             permission_type, wallet_id, metadata, 
             created_at, updated_at, is_active
             FROM users"
        )?;
        
        let users = stmt.query_map([], |row| {
            Ok(UserRow {
                id: row.get(0)?,
                profile_name: row.get(1)?,
                identity_type: row.get(2)?,
                identity_public_key: row.get(3)?,
                encryption_public_key: row.get(4)?,
                signature_public_key: row.get(5)?,
                node_signature_public_key: row.get(6)?,
                node_encryption_public_key: row.get(7)?,
                permission_type: row.get(8)?,
                wallet_id: row.get(9)?,
                metadata: row.get(10)?,
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
                is_active: row.get(13)?,
            })
        })?;
        
        let mut count = 0;
        let mut batch = Vec::new();
        
        for user in users {
            let user = user?;
            batch.push(user);
            
            if batch.len() >= self.config.batch_size {
                self.insert_users_batch(&batch).await?;
                count += batch.len();
                batch.clear();
            }
        }
        
        // Insert remaining batch
        if !batch.is_empty() {
            self.insert_users_batch(&batch).await?;
            count += batch.len();
        }
        
        Ok(count)
    }

    /// Insert batch of users into LanceDB
    async fn insert_users_batch(&self, users: &[UserRow]) -> Result<()> {
        let schema = user_schema();
        
        // Build arrays for each column
        let mut id_builder = StringArray::builder(users.len());
        let mut profile_name_builder = StringArray::builder(users.len());
        let mut identity_type_builder = StringArray::builder(users.len());
        let mut identity_pk_builder = StringArray::builder(users.len());
        let mut encryption_pk_builder = StringArray::builder(users.len());
        let mut signature_pk_builder = StringArray::builder(users.len());
        let mut node_sig_pk_builder = StringArray::builder(users.len());
        let mut node_enc_pk_builder = StringArray::builder(users.len());
        let mut permission_builder = StringArray::builder(users.len());
        let mut wallet_builder = StringArray::builder(users.len());
        let mut metadata_builder = StringArray::builder(users.len());
        let mut created_at_builder = TimestampMillisecondArray::builder(users.len());
        let mut updated_at_builder = TimestampMillisecondArray::builder(users.len());
        let mut is_active_builder = BooleanArray::builder(users.len());
        
        for user in users {
            id_builder.append_value(&user.id);
            profile_name_builder.append_value(&user.profile_name);
            identity_type_builder.append_value(&user.identity_type);
            
            if let Some(v) = &user.identity_public_key {
                identity_pk_builder.append_value(v);
            } else {
                identity_pk_builder.append_null();
            }
            
            if let Some(v) = &user.encryption_public_key {
                encryption_pk_builder.append_value(v);
            } else {
                encryption_pk_builder.append_null();
            }
            
            if let Some(v) = &user.signature_public_key {
                signature_pk_builder.append_value(v);
            } else {
                signature_pk_builder.append_null();
            }
            
            if let Some(v) = &user.node_signature_public_key {
                node_sig_pk_builder.append_value(v);
            } else {
                node_sig_pk_builder.append_null();
            }
            
            if let Some(v) = &user.node_encryption_public_key {
                node_enc_pk_builder.append_value(v);
            } else {
                node_enc_pk_builder.append_null();
            }
            
            if let Some(v) = &user.permission_type {
                permission_builder.append_value(v);
            } else {
                permission_builder.append_null();
            }
            
            if let Some(v) = &user.wallet_id {
                wallet_builder.append_value(v);
            } else {
                wallet_builder.append_null();
            }
            
            if let Some(v) = &user.metadata {
                metadata_builder.append_value(v);
            } else {
                metadata_builder.append_null();
            }
            
            created_at_builder.append_value(user.created_at);
            updated_at_builder.append_value(user.updated_at);
            is_active_builder.append_value(user.is_active);
        }
        
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(id_builder.finish()) as ArrayRef,
                Arc::new(profile_name_builder.finish()) as ArrayRef,
                Arc::new(identity_type_builder.finish()) as ArrayRef,
                Arc::new(identity_pk_builder.finish()) as ArrayRef,
                Arc::new(encryption_pk_builder.finish()) as ArrayRef,
                Arc::new(signature_pk_builder.finish()) as ArrayRef,
                Arc::new(node_sig_pk_builder.finish()) as ArrayRef,
                Arc::new(node_enc_pk_builder.finish()) as ArrayRef,
                Arc::new(permission_builder.finish()) as ArrayRef,
                Arc::new(wallet_builder.finish()) as ArrayRef,
                Arc::new(metadata_builder.finish()) as ArrayRef,
                Arc::new(created_at_builder.finish()) as ArrayRef,
                Arc::new(updated_at_builder.finish()) as ArrayRef,
                Arc::new(is_active_builder.finish()) as ArrayRef,
            ],
        )?;
        
        // Insert into LanceDB
        let table = self.config.target_db.table("users").await?;
        table.add(&[batch]).execute().await?;
        
        Ok(())
    }

    /// Migrate tools table
    async fn migrate_tools(&mut self) -> Result<usize> {
        info!("Migrating tools table");
        
        // Check if tools table exists
        let table_exists: bool = self.sqlite_conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='tools'",
            [],
            |row| row.get(0),
        )?;
        
        if !table_exists {
            warn!("Tools table does not exist in SQLite database");
            return Ok(0);
        }
        
        // Similar implementation as migrate_users
        // TODO: Complete implementation
        Ok(0)
    }

    /// Migrate agents table
    async fn migrate_agents(&mut self) -> Result<usize> {
        info!("Migrating agents table");
        // TODO: Implement agent migration
        Ok(0)
    }

    /// Migrate embeddings table
    async fn migrate_embeddings(&mut self) -> Result<usize> {
        info!("Migrating embeddings table");
        // TODO: Implement embeddings migration with vector data
        Ok(0)
    }

    /// Migrate jobs table
    async fn migrate_jobs(&mut self) -> Result<usize> {
        info!("Migrating jobs table");
        // TODO: Implement jobs migration
        Ok(0)
    }

    /// Verify migration results
    async fn verify_migration(&self, stats: &MigrationStats) -> Result<()> {
        info!("Verifying migration results");
        
        let db_stats = self.config.target_db.stats().await?;
        
        for table_stat in &db_stats.table_stats {
            match table_stat.name.as_str() {
                "users" => {
                    if table_stat.row_count < stats.users_migrated {
                        warn!(
                            "User count mismatch: expected {}, found {}",
                            stats.users_migrated, table_stat.row_count
                        );
                    }
                }
                "tools" => {
                    if table_stat.row_count < stats.tools_migrated {
                        warn!(
                            "Tools count mismatch: expected {}, found {}",
                            stats.tools_migrated, table_stat.row_count
                        );
                    }
                }
                _ => {}
            }
        }
        
        info!("Verification completed");
        Ok(())
    }
}

/// Migration statistics
#[derive(Debug, Default, Clone)]
pub struct MigrationStats {
    pub users_migrated: usize,
    pub tools_migrated: usize,
    pub agents_migrated: usize,
    pub embeddings_migrated: usize,
    pub jobs_migrated: usize,
    pub errors: Vec<String>,
}

/// Temporary struct for SQLite user row
struct UserRow {
    id: String,
    profile_name: String,
    identity_type: String,
    identity_public_key: Option<String>,
    encryption_public_key: Option<String>,
    signature_public_key: Option<String>,
    node_signature_public_key: Option<String>,
    node_encryption_public_key: Option<String>,
    permission_type: Option<String>,
    wallet_id: Option<String>,
    metadata: Option<String>,
    created_at: i64,
    updated_at: i64,
    is_active: bool,
}

/// Migration CLI tool
pub async fn run_migration_cli(sqlite_path: &str, lance_path: &str) -> Result<()> {
    println!("SQLite to LanceDB Migration Tool");
    println!("=================================\n");
    
    // Create LanceDB instance
    let lance_config = crate::LanceDbConfig {
        path: lance_path.into(),
        ..Default::default()
    };
    let lance_db = Arc::new(LanceDb::with_config(lance_config).await?);
    
    // Create migration config
    let config = MigrationConfig {
        sqlite_path: sqlite_path.to_string(),
        target_db: lance_db,
        batch_size: 1000,
        skip_existing: false,
        verify: true,
    };
    
    // Run migration
    let mut migrator = Migrator::new(config)?;
    let stats = migrator.migrate_all().await?;
    
    // Print results
    println!("\nMigration Results:");
    println!("  Users migrated: {}", stats.users_migrated);
    println!("  Tools migrated: {}", stats.tools_migrated);
    println!("  Agents migrated: {}", stats.agents_migrated);
    println!("  Embeddings migrated: {}", stats.embeddings_migrated);
    println!("  Jobs migrated: {}", stats.jobs_migrated);
    
    if !stats.errors.is_empty() {
        println!("\nErrors encountered:");
        for error in &stats.errors {
            println!("  - {}", error);
        }
    }
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_migration_config() {
        let config = MigrationConfig::default();
        assert_eq!(config.batch_size, 1000);
        assert!(config.verify);
    }
}