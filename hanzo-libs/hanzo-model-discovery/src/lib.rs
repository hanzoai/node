use async_trait::async_trait;
use chrono::{DateTime, Utc};
use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Represents a model available for download
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub author: String,
    pub tags: Vec<String>,
    pub downloads: u64,
    pub likes: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub model_size_gb: Option<f32>,
    pub quantization: Option<String>,
    pub base_model: Option<String>,
    pub context_length: Option<u32>,
    pub parameters: Option<String>,
    pub license: Option<String>,
    pub language: Vec<String>,
    pub library: Option<String>,
    pub pipeline_tag: Option<String>,
    pub trusted_source: bool,
    pub download_url: String,
}

/// Model sources
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ModelSource {
    HanzoLM,           // hanzo-lm org
    HanzoMLX,          // hanzo-mlx org
    HanzoCommunity,    // hanzo-community (mirror of lmstudio)
    HanzoEmbeddings,   // hanzo-embeddings org
    HanzoTools,        // hanzo-tools org
    LMStudio,          // lmstudio-community
    MLXCommunity,      // mlx-community
    HuggingFace(String), // Any HF model
}

/// Filter options for model search
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelFilter {
    pub source: Option<Vec<ModelSource>>,
    pub quantization: Option<Vec<String>>,
    pub min_downloads: Option<u64>,
    pub max_size_gb: Option<f32>,
    pub min_context: Option<u32>,
    pub language: Option<Vec<String>>,
    pub library: Option<Vec<String>>,
    pub pipeline_tag: Option<Vec<String>>,
    pub trusted_only: bool,
    pub search_query: Option<String>,
}

/// Sort options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SortBy {
    Downloads,
    Likes,
    UpdatedRecently,
    CreatedRecently,
    Name,
    Size,
}

#[async_trait]
pub trait ModelDiscovery {
    /// Search for models with filters
    async fn search_models(&self, filter: &ModelFilter, sort: SortBy, limit: usize) -> Result<Vec<ModelInfo>, String>;

    /// Get detailed info about a specific model
    async fn get_model_info(&self, model_id: &str) -> Result<ModelInfo, String>;

    /// List all models from a specific source
    async fn list_source_models(&self, source: ModelSource) -> Result<Vec<ModelInfo>, String>;

    /// Get recommended models for a use case
    async fn get_recommended(&self, use_case: &str) -> Result<Vec<ModelInfo>, String>;
}

pub struct HanzoModelDiscovery {
    client: Client,
    cache: HashMap<String, Vec<ModelInfo>>,
}

impl Default for HanzoModelDiscovery {
    fn default() -> Self {
        Self::new()
    }
}

impl HanzoModelDiscovery {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            cache: HashMap::new(),
        }
    }

    /// Parse model ID to extract metadata
    fn parse_model_id(&self, model_id: &str) -> ModelInfo {
        let parts: Vec<&str> = model_id.split('/').collect();
        let (author, name) = if parts.len() == 2 {
            (parts[0].to_string(), parts[1].to_string())
        } else {
            ("unknown".to_string(), model_id.to_string())
        };

        // Extract metadata from name
        let mut quantization = None;
        let mut parameters = None;
        let mut base_model = None;
        let mut model_size_gb = None;

        // Check for quantization (4bit, 8bit, etc.)
        let quant_regex = Regex::new(r"(\d+)bit").unwrap();
        if let Some(caps) = quant_regex.captures(&name) {
            quantization = Some(format!("{}bit", &caps[1]));
        }

        // Check for GGUF formats
        if name.contains("Q4_K_M") || name.contains("Q5_K_M") || name.contains("Q8_0") {
            quantization = Some(name.split('-').next_back().unwrap_or("GGUF").to_string());
        }

        // Extract parameter count (7B, 70B, etc.)
        let param_regex = Regex::new(r"(\d+\.?\d*)([BM])").unwrap();
        if let Some(caps) = param_regex.captures(&name) {
            parameters = Some(format!("{}{}", &caps[1], &caps[2]));

            // Estimate size based on parameters and quantization
            let param_num: f32 = caps[1].parse().unwrap_or(0.0);
            let multiplier = match &caps[2] {
                "B" => 1.0,
                "M" => 0.001,
                _ => 1.0,
            };
            let base_size = param_num * multiplier * 2.0; // 2 bytes per param (fp16)

            model_size_gb = Some(match quantization.as_deref() {
                Some("4bit") => base_size * 0.5,
                Some("8bit") => base_size * 1.0,
                _ => base_size,
            });
        }

        // Extract base model name
        if name.contains("Llama") {
            base_model = Some("Llama".to_string());
        } else if name.contains("Qwen") {
            base_model = Some("Qwen".to_string());
        } else if name.contains("Mistral") {
            base_model = Some("Mistral".to_string());
        } else if name.contains("Gemma") {
            base_model = Some("Gemma".to_string());
        }

        // Determine if trusted source
        let trusted_source = matches!(
            author.as_str(),
            "hanzo-lm" | "hanzo-mlx" | "hanzo-community" | "hanzo-embeddings" | "hanzo-tools" |
            "lmstudio-community" | "mlx-community" | "meta-llama" | "mistralai" | "google" |
            "microsoft" | "Qwen" | "deepseek-ai" | "NousResearch"
        );

        ModelInfo {
            id: model_id.to_string(),
            name: name.clone(),
            author: author.clone(),
            tags: vec![],
            downloads: 0,
            likes: 0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            model_size_gb,
            quantization,
            base_model,
            context_length: Some(if name.contains("128k") {
                131072
            } else if name.contains("32k") {
                32768
            } else {
                8192
            }),
            parameters,
            license: None,
            language: vec!["en".to_string()],
            library: if author.contains("mlx") {
                Some("mlx".to_string())
            } else {
                Some("transformers".to_string())
            },
            pipeline_tag: Some("text-generation".to_string()),
            trusted_source,
            download_url: format!("https://huggingface.co/{model_id}"),
        }
    }
}

#[async_trait]
impl ModelDiscovery for HanzoModelDiscovery {
    async fn search_models(&self, filter: &ModelFilter, sort: SortBy, limit: usize) -> Result<Vec<ModelInfo>, String> {
        // Use HuggingFace API to search
        let mut url = "https://huggingface.co/api/models".to_string();
        let mut params = vec![];

        // Add search query
        if let Some(query) = &filter.search_query {
            params.push(format!("search={}", urlencoding::encode(query)));
        }

        // Add author filter for sources
        if let Some(sources) = &filter.source {
            let authors: Vec<String> = sources.iter().map(|s| match s {
                ModelSource::HanzoLM => "hanzo-lm".to_string(),
                ModelSource::HanzoMLX => "hanzo-mlx".to_string(),
                ModelSource::HanzoCommunity => "hanzo-community".to_string(),
                ModelSource::HanzoEmbeddings => "hanzo-embeddings".to_string(),
                ModelSource::HanzoTools => "hanzo-tools".to_string(),
                ModelSource::LMStudio => "lmstudio-community".to_string(),
                ModelSource::MLXCommunity => "mlx-community".to_string(),
                ModelSource::HuggingFace(org) => org.clone(),
            }).collect();

            for author in authors {
                params.push(format!("author={author}"));
            }
        }

        // Add library filter
        if let Some(libs) = &filter.library {
            for lib in libs {
                params.push(format!("library={lib}"));
            }
        }

        // Add pipeline filter
        if let Some(tags) = &filter.pipeline_tag {
            for tag in tags {
                params.push(format!("pipeline_tag={tag}"));
            }
        }

        // Add sort
        let sort_param = match sort {
            SortBy::Downloads => "downloads",
            SortBy::Likes => "likes",
            SortBy::UpdatedRecently => "lastModified",
            SortBy::CreatedRecently => "createdAt",
            SortBy::Name => "id",
            SortBy::Size => "downloads", // No direct size sort
        };
        params.push(format!("sort={sort_param}"));
        params.push(format!("limit={limit}"));

        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }

        // Make request
        let response = self.client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Failed to fetch models: {e}"))?;

        let models_json: Vec<serde_json::Value> = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {e}"))?;

        // Parse models
        let mut models: Vec<ModelInfo> = models_json
            .iter()
            .map(|m| {
                let model_id = m["modelId"].as_str().unwrap_or("").to_string();
                let mut info = self.parse_model_id(&model_id);

                // Update with actual data from API
                if let Some(downloads) = m["downloads"].as_u64() {
                    info.downloads = downloads;
                }
                if let Some(likes) = m["likes"].as_u64() {
                    info.likes = likes;
                }
                if let Some(tags) = m["tags"].as_array() {
                    info.tags = tags.iter()
                        .filter_map(|t| t.as_str())
                        .map(|s| s.to_string())
                        .collect();
                }

                info
            })
            .collect();

        // Apply additional filters
        if filter.trusted_only {
            models.retain(|m| m.trusted_source);
        }

        if let Some(max_size) = filter.max_size_gb {
            models.retain(|m| m.model_size_gb.unwrap_or(f32::MAX) <= max_size);
        }

        if let Some(min_context) = filter.min_context {
            models.retain(|m| m.context_length.unwrap_or(0) >= min_context);
        }

        if let Some(quants) = &filter.quantization {
            models.retain(|m| {
                m.quantization.as_ref()
                    .map(|q| quants.contains(q))
                    .unwrap_or(false)
            });
        }

        models.truncate(limit);
        Ok(models)
    }

    async fn get_model_info(&self, model_id: &str) -> Result<ModelInfo, String> {
        let url = format!("https://huggingface.co/api/models/{model_id}");

        let response = self.client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Failed to fetch model: {e}"))?;

        let model_json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {e}"))?;

        let mut info = self.parse_model_id(model_id);

        // Update with actual data
        if let Some(downloads) = model_json["downloads"].as_u64() {
            info.downloads = downloads;
        }
        if let Some(likes) = model_json["likes"].as_u64() {
            info.likes = likes;
        }

        Ok(info)
    }

    async fn list_source_models(&self, source: ModelSource) -> Result<Vec<ModelInfo>, String> {
        let filter = ModelFilter {
            source: Some(vec![source]),
            ..Default::default()
        };

        self.search_models(&filter, SortBy::Downloads, 100).await
    }

    async fn get_recommended(&self, use_case: &str) -> Result<Vec<ModelInfo>, String> {
        let filter = match use_case {
            "chat" => ModelFilter {
                source: Some(vec![
                    ModelSource::HanzoLM,
                    ModelSource::HanzoCommunity,
                ]),
                search_query: Some("instruct chat".to_string()),
                trusted_only: true,
                ..Default::default()
            },
            "code" => ModelFilter {
                search_query: Some("code coder deepseek codellama".to_string()),
                trusted_only: true,
                ..Default::default()
            },
            "embedding" => ModelFilter {
                source: Some(vec![ModelSource::HanzoEmbeddings]),
                pipeline_tag: Some(vec!["sentence-similarity".to_string()]),
                ..Default::default()
            },
            "vision" => ModelFilter {
                search_query: Some("vision llava pixtral".to_string()),
                trusted_only: true,
                ..Default::default()
            },
            "mlx" => ModelFilter {
                source: Some(vec![ModelSource::HanzoMLX, ModelSource::MLXCommunity]),
                library: Some(vec!["mlx".to_string()]),
                ..Default::default()
            },
            _ => ModelFilter {
                trusted_only: true,
                ..Default::default()
            }
        };

        self.search_models(&filter, SortBy::Downloads, 10).await
    }
}

// Helper for URL encoding
mod urlencoding {
    pub fn encode(s: &str) -> String {
        percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_model_id() {
        let discovery = HanzoModelDiscovery::new();

        let info = discovery.parse_model_id("hanzo-lm/Llama-3.3-70B-Instruct-4bit");
        assert_eq!(info.author, "hanzo-lm");
        assert_eq!(info.quantization, Some("4bit".to_string()));
        assert_eq!(info.parameters, Some("70B".to_string()));
        assert_eq!(info.base_model, Some("Llama".to_string()));
        assert!(info.trusted_source);
    }

    #[tokio::test]
    async fn test_search_models() {
        let discovery = HanzoModelDiscovery::new();

        let filter = ModelFilter {
            source: Some(vec![ModelSource::HanzoCommunity]),
            trusted_only: true,
            ..Default::default()
        };

        let results = discovery.search_models(&filter, SortBy::Downloads, 5).await;
        assert!(results.is_ok());
    }
}