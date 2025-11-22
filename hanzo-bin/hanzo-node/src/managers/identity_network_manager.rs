use hanzo_identity::{OnchainIdentity, HanzoRegistry};
use hanzo_messages::hanzo_utils::hanzo_logging::{hanzo_log, HanzoLogLevel, HanzoLogOption};
use std::{env, sync::Arc};
use tokio::sync::Mutex;

pub struct IdentityNetworkManager {
    registry: Arc<Mutex<HanzoRegistry>>,
}

impl IdentityNetworkManager {
    pub async fn new() -> Self {
        // TODO: Update with mainnet values (eventually)
        let rpc_url = env::var("RPC_URL").unwrap_or("https://sepolia.base.org".to_string());
        let contract_address =
            env::var("CONTRACT_ADDRESS").unwrap_or("0x425fb20ba3874e887336aaa7f3fab32d08135ba9".to_string());
        let abi_path = env::var("ABI_PATH").ok();
        hanzo_log(
            HanzoLogOption::IdentityNetwork,
            HanzoLogLevel::Info,
            &format!("Identity Network Manager initialized with ABI path: {:?}", abi_path),
        );

        let registry = HanzoRegistry::new(&rpc_url, &contract_address, abi_path)
            .await
            .unwrap();

        let registry = Arc::new(Mutex::new(registry));

        IdentityNetworkManager { registry }
    }

    pub async fn external_identity_to_profile_data(
        &self,
        global_identity: String,
        force_refresh: Option<bool>,
    ) -> Result<OnchainIdentity, &'static str> {
        let record = {
            let identity = global_identity.trim_start_matches("@@");
            let registry = self.registry.lock().await;
            match registry.get_identity_record(identity.to_string(), force_refresh).await {
                Ok(record) => record,
                Err(_) => return Err("Unrecognized global identity"),
            }
        };

        // Check if any of the address_or_proxy_nodes ends with .sepolia-hanzo
        if record.address_or_proxy_nodes.iter().any(|node| {
            let node_base = node.split(':').next().unwrap_or(node);
            node_base.ends_with(".sepolia-hanzo")
                || node_base.ends_with(".hanzo")
                || node_base.ends_with(".sep-hanzo")
        }) {
            // Call the proxy node to get the actual data
            let proxy_identity = record.address_or_proxy_nodes.clone();
            let proxy_record = {
                let registry = self.registry.lock().await;
                match registry
                    .get_identity_record(proxy_identity.join(","), force_refresh)
                    .await
                {
                    Ok(record) => record,
                    Err(_) => return Err("Failed to fetch proxy node data"),
                }
            };

            // Return the same record but with the updated address_or_proxy_nodes field
            let updated_record = OnchainIdentity {
                address_or_proxy_nodes: proxy_record.address_or_proxy_nodes,
                ..record
            };
            eprintln!(
                "external_identity_to_profile_data> Found record with proxy: {:?}",
                updated_record
            );

            return Ok(updated_record);
        }

        eprintln!("external_identity_to_profile_data> Found record: {:?}", record);
        Ok(record)
    }
}
