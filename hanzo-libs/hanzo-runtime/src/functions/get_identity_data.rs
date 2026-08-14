use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{NonRustCodeRunnerFactory, NonRustRuntime, RunError};

#[derive(Debug, Serialize)]
pub struct Configurations {
    rpc_urls: Vec<String>,
    contract_address: String,
    contract_abi: String,
    timeout_rpc_request_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct Input {
    #[serde(rename = "identityId")]
    identity_id: String,
}

#[derive(Debug, Deserialize)]
pub struct IdentityData {
    #[serde(rename = "boundNft")]
    pub bound_nft: String,
    #[serde(rename = "stakedTokens")]
    pub staked_tokens: String,
    #[serde(rename = "encryptionKey")]
    pub encryption_key: String,
    #[serde(rename = "signatureKey")]
    pub signature_key: String,
    #[serde(rename = "routing")]
    pub routing: bool,
    #[serde(rename = "addressOrProxyNodes")]
    pub address_or_proxy_nodes: Vec<String>,
    #[serde(rename = "delegatedTokens")]
    pub delegated_tokens: String,
    #[serde(rename = "lastUpdated")]
    pub last_updated: u64,
}

#[derive(Debug, Deserialize)]
pub struct Output {
    #[serde(rename = "identityData")]
    pub identity_data: Option<IdentityData>,
}

pub async fn get_identity_data(
    rpc_urls: Vec<String>,
    contract_address: String,
    contract_abi: String,
    identity_id: String,
) -> Result<Output, RunError> {
    let code = include_str!("getIdentityDataImpl.ts");

    let per_rpc_timeout = Duration::from_secs(5);
    let configurations = Configurations {
        rpc_urls: rpc_urls.clone(),
        contract_address,
        contract_abi,
        timeout_rpc_request_ms: per_rpc_timeout.as_millis() as u64,
    };

    // The JsonRpcProvider has some issues https://github.com/ethers-io/ethers.js/issues/4377
    // and the are some casses where even with a real timeout on the network layer the node/deno process remains opened
    // so we need to set a custom timeout on top of the process
    let execution_timeout = Some(per_rpc_timeout * rpc_urls.len() as u32);
    let runner = NonRustCodeRunnerFactory::new("get_identity_data", code, vec![])
        .with_runtime(NonRustRuntime::Deno)
        .create_runner(configurations);
    runner
        .run::<Input, Output>(Input { identity_id }, execution_timeout)
        .await
}
