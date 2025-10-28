//! # Vector Search Implementation for LanceDB
//!
//! High-performance semantic search, KNN search, and hybrid search capabilities.

use anyhow::{Context, Result};
use arrow_array::{Float32Array, RecordBatch, StringArray};
use lancedb::{query::Query, Table};
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Search configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    /// Maximum number of results
    pub limit: usize,
    /// Similarity threshold (0.0 to 1.0)
    pub threshold: Option<f32>,
    /// Enable re-ranking
    pub rerank: bool,
    /// Metadata filters
    pub filters: Option<HashMap<String, serde_json::Value>>,
    /// Search mode
    pub mode: SearchMode,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            limit: 10,
            threshold: Some(0.7),
            rerank: false,
            filters: None,
            mode: SearchMode::Vector,
        }
    }
}

/// Search mode enum
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    /// Pure vector similarity search
    Vector,
    /// Full-text keyword search
    Keyword,
    /// Hybrid search combining vector and keyword
    Hybrid,
    /// Multi-modal search
    Multimodal,
}

/// Search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// Document ID
    pub id: String,
    /// Original content
    pub content: String,
    /// Similarity score (0.0 to 1.0)
    pub score: f32,
    /// Distance metric value
    pub distance: f32,
    /// Additional metadata
    pub metadata: Option<serde_json::Value>,
    /// Source information
    pub source: Option<String>,
}

/// Vector search engine
pub struct VectorSearch {
    table: Table,
}

impl VectorSearch {
    /// Create new vector search instance
    pub fn new(table: Table) -> Self {
        Self { table }
    }

    /// Perform semantic search using vector similarity
    pub async fn search(
        &self,
        query_vector: Vec<f32>,
        config: SearchConfig,
    ) -> Result<Vec<SearchResult>> {
        info!("Performing vector search with limit: {}", config.limit);
        
        let mut query = self.table
            .vector_search(&query_vector)
            .limit(config.limit);
        
        // Apply metadata filters if provided
        if let Some(filters) = &config.filters {
            for (key, value) in filters {
                query = apply_filter(query, key, value)?;
            }
        }
        
        let results = query
            .execute()
            .await
            .context("Failed to execute vector search")?;
        
        // Convert to search results
        let mut search_results = Vec::new();
        
        for batch in results.try_collect::<Vec<RecordBatch>>().await? {
            search_results.extend(batch_to_results(&batch, &config)?);        
        }
        
        // Apply threshold filtering
        if let Some(threshold) = config.threshold {
            search_results.retain(|r| r.score >= threshold);
        }
        
        // Re-rank if enabled
        if config.rerank {
            search_results = self.rerank_results(search_results).await?;
        }
        
        Ok(search_results)
    }

    /// Perform keyword-based search
    pub async fn keyword_search(
        &self,
        query: &str,
        config: SearchConfig,
    ) -> Result<Vec<SearchResult>> {
        info!("Performing keyword search for: {}", query);
        
        // Full-text search
        let results = self.table
            .query()
            .full_text_search("content", query)
            .limit(config.limit)
            .execute()
            .await
            .context("Failed to execute keyword search")?;
        
        let mut search_results = Vec::new();
        
        for batch in results.try_collect::<Vec<RecordBatch>>().await? {
            search_results.extend(batch_to_results(&batch, &config)?);        
        }
        
        Ok(search_results)
    }

    /// Perform hybrid search combining vector and keyword
    pub async fn hybrid_search(
        &self,
        query: &str,
        query_vector: Vec<f32>,
        config: SearchConfig,
    ) -> Result<Vec<SearchResult>> {
        info!("Performing hybrid search");
        
        // Get vector search results
        let mut vector_config = config.clone();
        vector_config.limit = config.limit * 2; // Get more candidates
        let vector_results = self.search(query_vector, vector_config).await?;
        
        // Get keyword search results
        let mut keyword_config = config.clone();
        keyword_config.limit = config.limit * 2;
        let keyword_results = self.keyword_search(query, keyword_config).await?;
        
        // Combine and deduplicate results
        let combined = self.combine_results(vector_results, keyword_results, &config)?;
        
        Ok(combined)
    }

    /// Find K nearest neighbors
    pub async fn knn_search(
        &self,
        query_vector: Vec<f32>,
        k: usize,
    ) -> Result<Vec<SearchResult>> {
        debug!("Performing KNN search with k={}", k);
        
        let results = self.table
            .vector_search(&query_vector)
            .limit(k)
            .execute()
            .await
            .context("Failed to execute KNN search")?;
        
        let mut search_results = Vec::new();
        let config = SearchConfig::default();
        
        for batch in results.try_collect::<Vec<RecordBatch>>().await? {
            search_results.extend(batch_to_results(&batch, &config)?);        
        }
        
        Ok(search_results)
    }

    /// Search with multiple query vectors (multi-query)
    pub async fn multi_query_search(
        &self,
        query_vectors: Vec<Vec<f32>>,
        config: SearchConfig,
    ) -> Result<Vec<SearchResult>> {
        info!("Performing multi-query search with {} queries", query_vectors.len());
        
        let mut all_results = Vec::new();
        let mut result_map: HashMap<String, (SearchResult, f32)> = HashMap::new();
        
        // Search with each query vector
        for (i, vector) in query_vectors.iter().enumerate() {
            debug!("Processing query vector {}/{}", i + 1, query_vectors.len());
            
            let results = self.search(vector.clone(), config.clone()).await?;
            
            // Aggregate scores
            for result in results {
                let entry = result_map
                    .entry(result.id.clone())
                    .or_insert((result.clone(), 0.0));
                entry.1 += result.score;
            }
        }
        
        // Average scores and convert to results
        let num_queries = query_vectors.len() as f32;
        for (mut result, total_score) in result_map.into_values() {
            result.score = total_score / num_queries;
            all_results.push(result);
        }
        
        // Sort by score
        all_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        all_results.truncate(config.limit);
        
        Ok(all_results)
    }

    /// Diversity-aware search to reduce redundancy
    pub async fn diverse_search(
        &self,
        query_vector: Vec<f32>,
        config: SearchConfig,
        diversity_threshold: f32,
    ) -> Result<Vec<SearchResult>> {
        info!("Performing diverse search with threshold: {}", diversity_threshold);
        
        // Get more candidates than needed
        let mut search_config = config.clone();
        search_config.limit = config.limit * 3;
        let candidates = self.search(query_vector, search_config).await?;
        
        // Select diverse results
        let mut selected = Vec::new();
        let mut selected_vectors: Vec<Vec<f32>> = Vec::new();
        
        for candidate in candidates {
            if selected.len() >= config.limit {
                break;
            }
            
            // Check similarity with already selected items
            let mut is_diverse = true;
            if !selected_vectors.is_empty() {
                // This is simplified - in production you'd fetch actual vectors
                // and compute cosine similarity
                for existing in &selected {
                    if candidate.content.chars().take(50).collect::<String>()
                        == existing.content.chars().take(50).collect::<String>() {
                        is_diverse = false;
                        break;
                    }
                }
            }
            
            if is_diverse {
                selected.push(candidate);
            }
        }
        
        Ok(selected)
    }

    /// Re-rank results using a more sophisticated model
    async fn rerank_results(
        &self,
        mut results: Vec<SearchResult>,
    ) -> Result<Vec<SearchResult>> {
        // In production, this would use a cross-encoder or other reranking model
        // For now, we'll just apply a simple boost based on metadata
        
        for result in &mut results {
            if let Some(metadata) = &result.metadata {
                // Boost based on recency, quality score, etc.
                if let Some(quality) = metadata.get("quality_score") {
                    if let Some(q) = quality.as_f64() {
                        result.score *= (1.0 + (q as f32 * 0.1));
                    }
                }
            }
        }
        
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        Ok(results)
    }

    /// Combine vector and keyword results with score fusion
    fn combine_results(
        &self,
        vector_results: Vec<SearchResult>,
        keyword_results: Vec<SearchResult>,
        config: &SearchConfig,
    ) -> Result<Vec<SearchResult>> {
        let mut combined_map: HashMap<String, SearchResult> = HashMap::new();
        
        // Add vector results with weight
        let vector_weight = 0.7;
        for mut result in vector_results {
            result.score *= vector_weight;
            combined_map.insert(result.id.clone(), result);
        }
        
        // Add or update with keyword results
        let keyword_weight = 0.3;
        for mut result in keyword_results {
            result.score *= keyword_weight;
            
            combined_map
                .entry(result.id.clone())
                .and_modify(|r| r.score += result.score)
                .or_insert(result);
        }
        
        // Convert to vector and sort
        let mut combined: Vec<SearchResult> = combined_map.into_values().collect();
        combined.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        combined.truncate(config.limit);
        
        Ok(combined)
    }
}

/// Apply filter to query
fn apply_filter(
    mut query: Query,
    key: &str,
    value: &serde_json::Value,
) -> Result<Query> {
    match value {
        serde_json::Value::String(s) => {
            query = query.filter(format!("{} = '{}'", key, s));
        }
        serde_json::Value::Number(n) => {
            query = query.filter(format!("{} = {}", key, n));
        }
        serde_json::Value::Bool(b) => {
            query = query.filter(format!("{} = {}", key, b));
        }
        _ => {
            return Err(anyhow::anyhow!("Unsupported filter value type"));
        }
    }
    Ok(query)
}

/// Convert RecordBatch to SearchResults
fn batch_to_results(batch: &RecordBatch, config: &SearchConfig) -> Result<Vec<SearchResult>> {
    let mut results = Vec::new();
    
    let id_array = batch
        .column_by_name("id")
        .context("Missing id column")?
        .as_any()
        .downcast_ref::<StringArray>()
        .context("Invalid id column type")?;
    
    let content_array = batch
        .column_by_name("content")
        .context("Missing content column")?
        .as_any()
        .downcast_ref::<StringArray>()
        .context("Invalid content column type")?;
    
    let distance_array = batch
        .column_by_name("_distance")
        .map(|col| {
            col.as_any()
                .downcast_ref::<Float32Array>()
        })
        .unwrap_or(None);
    
    for i in 0..batch.num_rows() {
        let id = id_array.value(i).to_string();
        let content = content_array.value(i).to_string();
        
        // Convert distance to similarity score (1 - normalized_distance)
        let distance = distance_array
            .and_then(|arr| Some(arr.value(i)))
            .unwrap_or(0.0);
        let score = 1.0 / (1.0 + distance); // Convert distance to similarity
        
        results.push(SearchResult {
            id,
            content,
            score,
            distance,
            metadata: None,
            source: None,
        });
    }
    
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_config_default() {
        let config = SearchConfig::default();
        assert_eq!(config.limit, 10);
        assert_eq!(config.threshold, Some(0.7));
        assert!(!config.rerank);
        assert!(matches!(config.mode, SearchMode::Vector));
    }

    #[test]
    fn test_search_mode_serialization() {
        let mode = SearchMode::Hybrid;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, r#""hybrid""#);
        
        let deserialized: SearchMode = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, SearchMode::Hybrid));
    }
}