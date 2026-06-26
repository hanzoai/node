use reqwest::Client;
use rusqlite::Result;
use serde::Deserialize;
use hanzo_embed::model_type::EmbeddingModelType;

// Native hanzo-engine OpenAI-compatible embeddings response (`/v1/embeddings`).
#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

pub struct EmbeddingFunction {
    client: Client,
    api_url: String,
    model_type: EmbeddingModelType,
}

impl EmbeddingFunction {
    pub fn new(api_url: &str, model_type: EmbeddingModelType) -> Self {
        Self {
            client: Client::new(),
            api_url: api_url.to_string(),
            model_type,
        }
    }

    pub async fn request_embeddings(&self, prompt: &str) -> Result<Vec<f32>, rusqlite::Error> {
        let model_str = match &self.model_type {
            EmbeddingModelType::OllamaTextEmbeddingsInference(model) => model.to_string(),
        };

        let max_tokens = self.model_type.max_input_token_count();
        let truncated_prompt = if prompt.len() > max_tokens {
            &prompt[..max_tokens]
        } else {
            prompt
        };

        let request_body = serde_json::json!({
            "model": model_str,
            "input": truncated_prompt
        });

        let full_url = if self.api_url.ends_with('/') {
            format!("{}v1/embeddings", self.api_url)
        } else {
            format!("{}/v1/embeddings", self.api_url)
        };

        let response = self.client.post(&full_url).json(&request_body).send().await;

        match response {
            Ok(response) => {
                if !response.status().is_success() {
                    println!("Failed to send request to embedding API: {}", response.status());
                    return Err(rusqlite::Error::InvalidQuery);
                }
                let embedding_response = response.json::<EmbeddingResponse>().await.map_err(|e| {
                    println!("Failed to convert response to EmbeddingResponse: {}", e);
                    rusqlite::Error::InvalidQuery
                })?;

                embedding_response
                    .data
                    .into_iter()
                    .next()
                    .map(|d| d.embedding)
                    .ok_or(rusqlite::Error::InvalidQuery)
            }
            Err(e) => {
                println!("Failed to send request to embedding API: {}", e);
                return Err(rusqlite::Error::InvalidParameterName(e.to_string()));
            }
        }
    }
}
