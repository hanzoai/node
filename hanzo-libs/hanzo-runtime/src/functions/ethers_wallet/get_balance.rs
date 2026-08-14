use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{NonRustCodeRunnerFactory, NonRustRuntime, RunError};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    pub token_address: Option<String>,
    pub wallet_address: String,
    pub rpc_url: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenInfo {
    pub name: String,
    pub symbol: String,
    pub decimals: u8,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Output {
    pub balance: String,
    pub formatted_balance: String,
    pub token_info: TokenInfo,
}

pub async fn get_balance(input: Input) -> Result<Output, RunError> {
    let code = include_str!("getBalanceDenoImpl.ts");
    let runner = NonRustCodeRunnerFactory::new("get_balance", code, vec![])
        .with_runtime(NonRustRuntime::Deno)
        .create_runner(json!({}));
    runner.run::<_, Output>(input, None).await
}
