//! SQL Brain - Local-first memory store with vector search
//!
//! Uses SQLite with sqlite-vec for in-database vector operations

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{sqlite::SqlitePoolOptions, Pool, Sqlite};
use std::path::Path;

use crate::SimulationResult;

/// User context loaded from SQL brain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserContext {
    pub user_id: String,
    pub episodes: Vec<Episode>,
    pub scene_memories: Vec<SceneMemory>,
    pub preferences: UserPreferences,
    pub delta_weights: Option<DeltaWeights>,
}

/// Episode memory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Episode {
    pub id: String,
    pub timestamp: i64,
    pub question: String,
    pub answer: String,
    pub confidence: f64,
    pub trajectory: Vec<StateTransition>,
    pub reward: f64,
}

/// State transition in episode
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateTransition {
    pub state: serde_json::Value,
    pub action: serde_json::Value,
    pub next_state: serde_json::Value,
    pub reward: f64,
}

/// Scene memory with embeddings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneMemory {
    pub scene_id: String,
    pub description: String,
    pub embedding: Vec<f32>,
    pub objects: Vec<ObjectMemory>,
    pub affordances: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectMemory {
    pub object_id: String,
    pub class: String,
    pub properties: serde_json::Value,
    pub embedding: Vec<f32>,
}

/// User preferences
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPreferences {
    pub risk_tolerance: f64,
    pub exploration_rate: f64,
    pub preferred_tier: Option<String>,
}

/// BitDelta 1-bit weight deltas for per-user adaptation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaWeights {
    pub base_model: String,
    pub delta_bits: Vec<u8>,     // 1-bit deltas packed
    pub scale: f32,               // Learned scale factor
    pub sparsity_mask: Vec<bool>, // Which weights to update
}

/// SQL Brain implementation
pub struct SqlBrain {
    pool: Pool<Sqlite>,
}

impl SqlBrain {
    pub async fn new(path: &str) -> Result<Self> {
        // Create database if it doesn't exist
        if !Path::new(path).exists() {
            tokio::fs::File::create(path).await?;
        }

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&format!("sqlite:{}", path))
            .await?;

        // Initialize schema
        Self::init_schema(&pool).await?;

        Ok(Self { pool })
    }

    async fn init_schema(pool: &Pool<Sqlite>) -> Result<()> {
        // Create tables
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS users (
                user_id TEXT PRIMARY KEY,
                created_at INTEGER NOT NULL,
                preferences TEXT NOT NULL
            )
            "#,
        )
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS episodes (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                question TEXT NOT NULL,
                answer TEXT NOT NULL,
                confidence REAL NOT NULL,
                trajectory TEXT NOT NULL,
                reward REAL NOT NULL,
                FOREIGN KEY (user_id) REFERENCES users(user_id)
            )
            "#,
        )
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS scenes (
                scene_id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                description TEXT NOT NULL,
                embedding BLOB NOT NULL,
                objects TEXT NOT NULL,
                affordances TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                FOREIGN KEY (user_id) REFERENCES users(user_id)
            )
            "#,
        )
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS objects (
                object_id TEXT PRIMARY KEY,
                scene_id TEXT NOT NULL,
                class TEXT NOT NULL,
                properties TEXT NOT NULL,
                embedding BLOB NOT NULL,
                FOREIGN KEY (scene_id) REFERENCES scenes(scene_id)
            )
            "#,
        )
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS delta_weights (
                user_id TEXT PRIMARY KEY,
                base_model TEXT NOT NULL,
                delta_bits BLOB NOT NULL,
                scale REAL NOT NULL,
                sparsity_mask BLOB NOT NULL,
                updated_at INTEGER NOT NULL,
                FOREIGN KEY (user_id) REFERENCES users(user_id)
            )
            "#,
        )
        .execute(pool)
        .await?;

        // Create vector index using sqlite-vec virtual table
        sqlx::query(
            r#"
            CREATE VIRTUAL TABLE IF NOT EXISTS scene_embeddings USING vec0(
                scene_id TEXT PRIMARY KEY,
                embedding FLOAT[768]
            )
            "#,
        )
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            CREATE VIRTUAL TABLE IF NOT EXISTS object_embeddings USING vec0(
                object_id TEXT PRIMARY KEY,
                embedding FLOAT[768]
            )
            "#,
        )
        .execute(pool)
        .await?;

        Ok(())
    }

    /// Load user context
    pub async fn load_user_context(&self, user_id: &str) -> Result<UserContext> {
        // Load user preferences
        let preferences: UserPreferences = sqlx::query_as!(
            UserPreferencesRow,
            "SELECT preferences FROM users WHERE user_id = ?",
            user_id
        )
        .fetch_optional(&self.pool)
        .await?
        .map(|row| serde_json::from_str(&row.preferences).unwrap())
        .unwrap_or_default();

        // Load recent episodes
        let episodes = sqlx::query!(
            r#"
            SELECT id, timestamp, question, answer, confidence, trajectory, reward
            FROM episodes
            WHERE user_id = ?
            ORDER BY timestamp DESC
            LIMIT 100
            "#,
            user_id
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| Episode {
            id: row.id,
            timestamp: row.timestamp,
            question: row.question,
            answer: row.answer,
            confidence: row.confidence,
            trajectory: serde_json::from_str(&row.trajectory).unwrap(),
            reward: row.reward,
        })
        .collect();

        // Load scene memories
        let scene_memories = sqlx::query!(
            r#"
            SELECT scene_id, description, embedding, objects, affordances
            FROM scenes
            WHERE user_id = ?
            ORDER BY created_at DESC
            LIMIT 10
            "#,
            user_id
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| SceneMemory {
            scene_id: row.scene_id,
            description: row.description,
            embedding: bincode::deserialize(&row.embedding).unwrap(),
            objects: serde_json::from_str(&row.objects).unwrap(),
            affordances: serde_json::from_str(&row.affordances).unwrap(),
        })
        .collect();

        // Load delta weights if they exist
        let delta_weights = sqlx::query!(
            r#"
            SELECT base_model, delta_bits, scale, sparsity_mask
            FROM delta_weights
            WHERE user_id = ?
            "#,
            user_id
        )
        .fetch_optional(&self.pool)
        .await?
        .map(|row| DeltaWeights {
            base_model: row.base_model,
            delta_bits: row.delta_bits,
            scale: row.scale,
            sparsity_mask: bincode::deserialize(&row.sparsity_mask).unwrap(),
        });

        Ok(UserContext {
            user_id: user_id.to_string(),
            episodes,
            scene_memories,
            preferences,
            delta_weights,
        })
    }

    /// Store simulation episode
    pub async fn store_episode(&self, user_id: &str, result: &SimulationResult) -> Result<()> {
        let episode_id = format!("ep_{}", uuid::Uuid::new_v4());
        let timestamp = chrono::Utc::now().timestamp();

        let trajectory = serde_json::to_string(&Vec::<StateTransition>::new())?; // Placeholder

        sqlx::query!(
            r#"
            INSERT INTO episodes (id, user_id, timestamp, question, answer, confidence, trajectory, reward)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            episode_id,
            user_id,
            timestamp,
            "",  // Question from request
            result.answer,
            result.confidence,
            trajectory,
            result.metrics.avg_reward
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Vector search for similar scenes
    pub async fn search_similar_scenes(
        &self,
        embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<SceneMemory>> {
        // Use sqlite-vec for efficient vector search
        let query = format!(
            r#"
            SELECT s.scene_id, s.description, s.embedding, s.objects, s.affordances
            FROM scenes s
            JOIN scene_embeddings se ON s.scene_id = se.scene_id
            WHERE se.embedding MATCH ?
            ORDER BY distance
            LIMIT {}
            "#,
            limit
        );

        let results = sqlx::query(&query)
            .bind(bincode::serialize(embedding)?)
            .fetch_all(&self.pool)
            .await?;

        Ok(results
            .into_iter()
            .map(|row| SceneMemory {
                scene_id: row.get("scene_id"),
                description: row.get("description"),
                embedding: bincode::deserialize(row.get("embedding")).unwrap(),
                objects: serde_json::from_str(row.get("objects")).unwrap(),
                affordances: serde_json::from_str(row.get("affordances")).unwrap(),
            })
            .collect())
    }

    /// Update BitDelta weights for user
    pub async fn update_delta_weights(&self, user_id: &str, delta: DeltaWeights) -> Result<()> {
        let timestamp = chrono::Utc::now().timestamp();
        let sparsity_mask_bytes = bincode::serialize(&delta.sparsity_mask)?;

        sqlx::query!(
            r#"
            INSERT OR REPLACE INTO delta_weights
            (user_id, base_model, delta_bits, scale, sparsity_mask, updated_at)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
            user_id,
            delta.base_model,
            delta.delta_bits,
            delta.scale,
            sparsity_mask_bytes,
            timestamp
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Compute BitDelta from recent episodes
    pub async fn compute_bitdelta(&self, user_id: &str) -> Result<DeltaWeights> {
        // Load recent episodes
        let episodes = sqlx::query!(
            r#"
            SELECT trajectory, reward
            FROM episodes
            WHERE user_id = ?
            ORDER BY timestamp DESC
            LIMIT 50
            "#,
            user_id
        )
        .fetch_all(&self.pool)
        .await?;

        // Convert trajectories to training data
        let mut training_data = Vec::new();
        for episode in episodes {
            let trajectory: Vec<StateTransition> = serde_json::from_str(&episode.trajectory)?;
            training_data.push((trajectory, episode.reward));
        }

        // Compute 1-bit deltas (placeholder implementation)
        // In reality, this would:
        // 1. Fine-tune on the training data
        // 2. Compute weight differences
        // 3. Quantize to 1-bit with learned scale
        // 4. Apply sparsity mask

        let delta_weights = DeltaWeights {
            base_model: "qwen3-30b-a3b".to_string(),
            delta_bits: vec![0xFF; 1024], // Placeholder
            scale: 0.01,
            sparsity_mask: vec![true; 8192], // Placeholder
        };

        Ok(delta_weights)
    }
}

// Helper structs for SQLx
struct UserPreferencesRow {
    preferences: String,
}

impl Default for UserPreferences {
    fn default() -> Self {
        Self {
            risk_tolerance: 0.5,
            exploration_rate: 0.1,
            preferred_tier: None,
        }
    }
}

// UUID placeholder
mod uuid {
    pub struct Uuid;
    impl Uuid {
        pub fn new_v4() -> String {
            format!("{:x}", rand::random::<u128>())
        }
    }
}

// Random placeholder
mod rand {
    pub fn random<T>() -> T
    where
        T: Default,
    {
        T::default()
    }
}