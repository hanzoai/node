use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{NonRustCodeRunnerFactory, NonRustRuntime, RunError};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedWallet {
    pub private_key: String,
    pub public_key: String,
    pub address: String,
    pub mnemonic: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Output {
    pub wallet: CreatedWallet,
}

pub async fn create_wallet(input: Input) -> Result<Output, RunError> {
    let code = include_str!("createWalletDenoImpl.ts");
    let runner = NonRustCodeRunnerFactory::new("create_wallet", code, vec![])
        .with_runtime(NonRustRuntime::Deno)
        .create_runner(json!({}));
    runner.run::<_, Output>(input, None).await
}
