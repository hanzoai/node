use super::hanzo_message_schemas::MessageSchemaType;
use crate::hanzo_utils::encryption::EncryptionMethod;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct HanzoMessage {
    pub body: MessageBody,
    pub external_metadata: ExternalMetadata,
    pub encryption: EncryptionMethod,
    pub version: HanzoVersion,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct HanzoBody {
    pub message_data: MessageData,
    pub internal_metadata: InternalMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct InternalMetadata {
    pub sender_subidentity: String,
    pub recipient_subidentity: String,
    pub inbox: String,
    pub signature: String,
    pub encryption: EncryptionMethod,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_api_data: Option<NodeApiData>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct ExternalMetadata {
    pub sender: String,
    pub recipient: String,
    pub scheduled_time: String,
    pub signature: String,
    pub intra_sender: String,
    pub other: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub struct NodeApiData {
    pub parent_hash: String,
    pub node_message_hash: String,
    pub node_timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct EncryptedHanzoBody {
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct EncryptedHanzoData {
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct HanzoData {
    pub message_raw_content: String,
    pub message_content_schema: MessageSchemaType,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub enum MessageBody {
    #[serde(rename = "encrypted")]
    Encrypted(EncryptedHanzoBody),
    #[serde(rename = "unencrypted")]
    Unencrypted(HanzoBody),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub enum MessageData {
    #[serde(rename = "encrypted")]
    Encrypted(EncryptedHanzoData),
    #[serde(rename = "unencrypted")]
    Unencrypted(HanzoData),
}

#[derive(Debug, Clone, PartialEq, ToSchema)]
pub enum HanzoVersion {
    V1_0,
    Unsupported,
}
