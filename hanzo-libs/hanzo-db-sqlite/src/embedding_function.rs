use reqwest::Client;
use rusqlite::Result;
use serde::Deserialize;
use hanzo_embed::model_type::EmbeddingModelType;

#[derive(Deserialize)]
struct EmbeddingResponseData {
    embedding: Vec<f32>,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingResponseData>,
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
        // The local Hanzo engine accepts the sentinel `"default"` to mean "whatever
        // embedding model is loaded" (it also accepts the model's full id). We send
        // `"default"` so the node adapts to whatever the engine serves — no embedding
        // model name is hardcoded; the vector dimension is auto-detected separately.
        let model_str = "default";

        let max_tokens = self.model_type.max_input_token_count();
        let truncated_prompt = if prompt.len() > max_tokens {
            &prompt[..max_tokens]
        } else {
            prompt
        };

        // OpenAI `/v1/embeddings` request schema: {"model": <model>, "input": <text>}
        let request_body = serde_json::json!({
            "model": model_str,
            "input": truncated_prompt
        });

        // The node talks to the local Hanzo engine over its `/v1/engine/embeddings`
        // path (the engine namespaces inference under `/v1/engine/`). Ollama is fully
        // retired; never use `/api/embeddings`.
        let full_url = if self.api_url.ends_with('/') {
            format!("{}v1/engine/embeddings", self.api_url)
        } else {
            format!("{}/v1/engine/embeddings", self.api_url)
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

                match embedding_response.data.into_iter().next() {
                    Some(first) => Ok(first.embedding),
                    None => {
                        println!("Embeddings response contained no data");
                        Err(rusqlite::Error::InvalidQuery)
                    }
                }
            }
            Err(e) => {
                println!("Failed to send request to embedding API: {}", e);
                return Err(rusqlite::Error::InvalidParameterName(e.to_string()));
            }
        }
    }
}
