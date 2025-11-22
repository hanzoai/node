use std::sync::Arc;

use crate::llm_provider::execution::chains::inference_chain_trait::LLMInferenceResponse;
use crate::llm_provider::llm_stopper::LLMStopper;
use crate::managers::model_capabilities_manager::ModelCapabilitiesManager;
use hanzo_messages::schemas::job_config::JobConfig;
use hanzo_messages::schemas::prompts::Prompt;
use hanzo_messages::schemas::ws_types::WSUpdateHandler;
use hanzo_db_sqlite::SqliteManager;

use super::super::error::LLMProviderError;
use super::shared::togetherai::TogetherAPIResponse;
use super::LLMService;
use async_trait::async_trait;
use reqwest::Client;

use serde_json;
use serde_json::json;
use hanzo_messages::schemas::inbox_name::InboxName;
use hanzo_messages::schemas::llm_providers::serialized_llm_provider::{LLMProviderInterface, TogetherAI};
use hanzo_messages::hanzo_utils::hanzo_logging::{hanzo_log, HanzoLogLevel, HanzoLogOption};
use tokio::sync::Mutex;

#[async_trait]
impl LLMService for TogetherAI {
    async fn call_api(
        &self,
        client: &Client,
        url: Option<&String>,
        api_key: Option<&String>,
        prompt: Prompt,
        model: LLMProviderInterface,
        _inbox_name: Option<InboxName>,
        _ws_manager_trait: Option<Arc<Mutex<dyn WSUpdateHandler + Send>>>,
        _config: Option<JobConfig>,
        _llm_stopper: Arc<LLMStopper>,
        db: Arc<SqliteManager>,
        tracing_message_id: Option<String>,
    ) -> Result<LLMInferenceResponse, LLMProviderError> {
        if let Some(base_url) = url {
            if let Some(key) = api_key {
                let url = format!("{}{}", base_url, "/inference");

                let _max_tokens = ModelCapabilitiesManager::get_max_tokens(&model);
                let max_input_tokens = ModelCapabilitiesManager::get_max_input_tokens(&model);
                let max_output_tokens = ModelCapabilitiesManager::get_max_output_tokens(&model);
                let messages_string = prompt.generate_genericapi_messages(
                    Some(max_input_tokens),
                    &ModelCapabilitiesManager::num_tokens_from_llama3,
                )?;

                hanzo_log(
                    HanzoLogOption::JobExecution,
                    HanzoLogLevel::Info,
                    format!("Messages JSON: {:?}", messages_string).as_str(),
                );

                let payload = json!({
                    "model": self.model_type,
                    "max_tokens": max_output_tokens,
                    "prompt": messages_string,
                    "request_type": "language-model-inference",
                    "temperature": 0.7,
                    "top_p": 0.7,
                    "top_k": 50,
                    "repetition_penalty": 1,
                    "stream_tokens": false,
                    "stop": [
                        "<|eot_id|>",
                        "[/INST]",
                        "</s>",
                        "Sys:"
                    ],
                    "negative_prompt": "",
                    "safety_model": "",
                    "repetitive_penalty": 1,
                });

                hanzo_log(
                    HanzoLogOption::JobExecution,
                    HanzoLogLevel::Debug,
                    format!("Call API Body: {:?}", payload).as_str(),
                );

                let payload_log = payload.clone();
                if let Some(ref msg_id) = tracing_message_id {
                    if let Err(e) = db.add_tracing(
                        msg_id,
                        _inbox_name.as_ref().map(|i| i.get_value()).as_deref(),
                        "llm_payload",
                        &payload_log,
                    ) {
                        eprintln!("failed to add payload trace: {:?}", e);
                    }
                }

                let res = client
                    .post(url)
                    .bearer_auth(key)
                    .header("Content-Type", "application/json")
                    .json(&payload)
                    .send()
                    .await?;

                hanzo_log(
                    HanzoLogOption::JobExecution,
                    HanzoLogLevel::Debug,
                    format!("Call API Status: {:?}", res.status()).as_str(),
                );

                let response_text = res.text().await?;
                hanzo_log(
                    HanzoLogOption::JobExecution,
                    HanzoLogLevel::Info,
                    format!("Call API Response Text: {:?}", response_text).as_str(),
                );
                let data_resp: Result<TogetherAPIResponse, _> = serde_json::from_str(&response_text);

                match data_resp {
                    Ok(data) => {
                        // Comment(Nico): maybe we could go over all the choices and check for the ones that can convert
                        // to json with our format and from those the longest one. I haven't see
                        // multiple choices so far though.
                        let response_string: String = data
                            .output
                            .choices
                            .first()
                            .map(|choice| choice.text.clone())
                            .unwrap_or_else(String::new);

                        return Ok(LLMInferenceResponse::new(
                            response_string,
                            None,
                            json!({}),
                            vec![],
                            Vec::new(),
                            None,
                        ));
                    }
                    Err(e) => {
                        hanzo_log(
                            HanzoLogOption::JobExecution,
                            HanzoLogLevel::Error,
                            format!("Failed to parse response: {:?}", e).as_str(),
                        );
                        Err(LLMProviderError::SerdeError(e))
                    }
                }
            } else {
                Err(LLMProviderError::ApiKeyNotSet)
            }
        } else {
            Err(LLMProviderError::UrlNotSet)
        }
    }
}
