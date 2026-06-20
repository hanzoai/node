use async_channel::Sender;
use ed25519_dalek::SigningKey;
use hanzo_http_api::node_api_router::APIError;
use hanzo_messages::schemas::hanzo_name::HanzoName;
use k256::ecdsa::SigningKey as Secp256k1SigningKey;
use serde_json::{json, Value};

use crate::{
    network::{node_error::NodeError, Node},
    utils::keys::{derive_secp256k1_signing_key, evm_address_from_secp256k1},
};

impl Node {
    /// Return the node's unified wallet/identity:
    /// the secp256k1 private key (hex), its EVM address, and the node's DID.
    ///
    /// The wallet is deterministically derived from the node's ed25519 identity
    /// secret seed, so `address == did's 0x id == mining-payout address`. This is a
    /// local-only convenience endpoint that exposes the secret key so the desktop
    /// can sign on the node's behalf.
    pub async fn v2_api_node_wallet(
        identity_secret_key: SigningKey,
        node_name: HanzoName,
        res: Sender<Result<Value, APIError>>,
    ) -> Result<(), NodeError> {
        let secp: Secp256k1SigningKey = derive_secp256k1_signing_key(&identity_secret_key);
        let address = evm_address_from_secp256k1(&secp);
        let private_key = format!("0x{}", hex::encode(secp.to_bytes()));
        let did = node_name.to_string();

        let body = json!({
            "address": address,
            "private_key": private_key,
            "did": did,
        });

        let _ = res.send(Ok(body)).await;
        Ok(())
    }
}
