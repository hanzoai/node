use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{NonRustCodeRunnerFactory, NonRustRuntime, RunError};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrivateKeySource {
    pub private_key: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecoverySource {
    Mnemonic(String),
    PrivateKey(String),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    pub source: RecoverySource,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveredWallet {
    pub private_key: String,
    pub public_key: Option<String>,
    pub address: String,
    pub mnemonic: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Output {
    pub wallet: RecoveredWallet,
}

pub async fn recover_wallet(input: Input) -> Result<Output, RunError> {
    let code = include_str!("recoverWalletDenoImpl.ts");
    let runner = NonRustCodeRunnerFactory::new("recover_wallet", code, vec![])
        .with_runtime(NonRustRuntime::Deno)
        .create_runner(json!({}));
    runner.run::<_, Output>(input, None).await
}
