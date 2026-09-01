use crate::{
    hanzo_message::hanzo_message::{MessageBody, HanzoMessage},
    hanzo_utils::hanzo_logging::{hanzo_log, HanzoLogLevel, HanzoLogOption},
    hanzo_utils::signatures::hash_signature_public_key,
};
use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::hash::Hash;
use std::{fmt, hash::Hasher};
use utoipa::ToSchema;

#[derive(Debug, Clone, Eq, ToSchema)]
pub struct HanzoName {
    pub full_name: String,
    pub node_name: String,
    pub profile_name: Option<String>,
    pub subidentity_type: Option<HanzoSubidentityType>,
    pub subidentity_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash, ToSchema)]
pub enum HanzoSubidentityType {
    Agent,
    Device,
}

impl fmt::Display for HanzoSubidentityType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HanzoSubidentityType::Agent => write!(f, "agent"),
            HanzoSubidentityType::Device => write!(f, "device"),
        }
    }
}

/// Alphanumeric or underscore, and never empty.
fn is_word(part: &str) -> bool {
    !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

// Valid Examples
// did:hanzo:c85950e4d9c2c24df9d44052c8f2298f69d4a9b82cb0266478f58f385ca2a679
// did:hanzo:c85950…/profileName
// did:hanzo:c85950…/profileName/agent/myChatGPTAgent
// did:hanzo:c85950…/profileName/device/myPhone

// Not valid examples
// did:hanzo:c85950…/profileName/myPhone
// did:hanzo:c859!50…
// did:hanzo:c85950…//
// alice

impl HanzoName {
    const PREFIX: &'static str = "did:hanzo:";

    /// The name a node answers to: the DID of its own signing key, addressed by
    /// the same hash that names its database.
    pub fn did(public_key: &VerifyingKey) -> String {
        format!("{}{}", Self::PREFIX, hash_signature_public_key(public_key))
    }

    pub fn new(raw_name: String) -> Result<Self, &'static str> {
        Self::validate_name(&raw_name)?;

        let parts: Vec<&str> = raw_name.split('/').collect();
        let node_name = parts[0].to_string();
        let profile_name = parts.get(1).map(|s| s.to_string());
        let subidentity_type = parts.get(2).map(|s| match *s {
            "agent" => HanzoSubidentityType::Agent,
            _ => HanzoSubidentityType::Device,
        });
        let subidentity_name = parts.get(3).map(|s| s.to_string());

        Ok(Self {
            full_name: raw_name.to_lowercase(),
            node_name: node_name.to_lowercase(),
            profile_name: profile_name.map(|s| s.to_lowercase()),
            subidentity_type,
            subidentity_name,
        })
    }

    pub fn is_fully_valid(hanzo_name: String) -> bool {
        match Self::validate_name(&hanzo_name) {
            Ok(_) => true,
            Err(err) => {
                hanzo_log(
                    HanzoLogOption::Identity,
                    HanzoLogLevel::Info,
                    &format!("Validation error: {}", err),
                );
                false
            }
        }
    }

    pub fn validate_name(raw_name: &str) -> Result<(), &'static str> {
        let parts: Vec<&str> = raw_name.split('/').collect();
        if parts.len() > 4 {
            return Err("Name should have one to four parts: node, profile, type (device or agent), and name.");
        }

        let address = parts[0]
            .strip_prefix(Self::PREFIX)
            .ok_or("Node name should be a DID: did:hanzo:<address>.")?;
        if !is_word(address) {
            return Err("The DID address should be alphanumeric or underscore.");
        }

        if let Some(profile) = parts.get(1) {
            if !is_word(profile) {
                return Err("The profile name should be alphanumeric or underscore.");
            }
        }

        match parts.len() {
            3 => Err("If type is 'agent' or 'device', a fourth part is expected."),
            4 if !matches!(parts[2], "agent" | "device") => {
                Err("The third part should either be 'agent' or 'device'.")
            }
            4 if !is_word(parts[3]) => {
                Err("The fourth part (name after 'agent' or 'device') should be alphanumeric or underscore.")
            }
            _ => Ok(()),
        }
    }

    #[allow(dead_code)]
    pub fn from_node_name(node_name: String) -> Result<Self, HanzoNameError> {
        // Ensure the node_name has no forward slashes
        if node_name.contains('/') {
            return Err(HanzoNameError::InvalidNameFormat(node_name.clone()));
        }
        let node_name_clone = node_name.clone();
        // Use the existing new() method to handle the rest of the formatting and checks
        match Self::new(node_name_clone) {
            Ok(name) => Ok(name),
            Err(_) => Err(HanzoNameError::InvalidNameFormat(node_name.clone())),
        }
    }

    pub fn from_node_and_profile_names(node_name: String, profile_name: String) -> Result<Self, &'static str> {
        Self::new(format!("{}/{}", node_name.to_lowercase(), profile_name.to_lowercase()))
    }

    #[allow(dead_code)]
    pub fn from_node_and_profile_names_and_type_and_name(
        node_name: String,
        profile_name: String,
        hanzo_type: HanzoSubidentityType,
        name: String,
    ) -> Result<Self, &'static str> {
        Self::new(format!(
            "{}/{}/{}/{}",
            node_name.to_lowercase(),
            profile_name.to_lowercase(),
            hanzo_type,
            name.to_lowercase()
        ))
    }

    #[allow(dead_code)]
    pub fn from_hanzo_message_using_sender_and_intra_sender(message: &HanzoMessage) -> Result<Self, &'static str> {
        let name = format!(
            "{}/{}",
            message.external_metadata.sender.clone(),
            message.external_metadata.intra_sender.clone()
        );
        Self::new(name)
    }

    #[allow(dead_code)]
    pub fn from_hanzo_message_only_using_sender_node_name(message: &HanzoMessage) -> Result<Self, &'static str> {
        Self::new(message.external_metadata.sender.clone())
    }

    #[allow(dead_code)]
    pub fn from_hanzo_message_only_using_recipient_node_name(message: &HanzoMessage) -> Result<Self, &'static str> {
        Self::new(message.external_metadata.recipient.clone())
    }

    #[allow(dead_code)]
    pub fn from_hanzo_message_using_sender_subidentity(message: &HanzoMessage) -> Result<Self, HanzoNameError> {
        // Check if outer encrypted and return error if so
        let body = match &message.body {
            MessageBody::Unencrypted(body) => body,
            _ => return Err(HanzoNameError::MessageBodyMissing),
        };

        let node = match Self::new(message.external_metadata.sender.clone()) {
            Ok(name) => name,
            Err(_) => {
                return Err(HanzoNameError::InvalidNameFormat(
                    message.external_metadata.sender.clone(),
                ))
            }
        };

        let sender_subidentity = if body.internal_metadata.sender_subidentity.is_empty() {
            String::from("")
        } else {
            format!("/{}", body.internal_metadata.sender_subidentity)
        };

        match Self::new(format!("{}{}", node, sender_subidentity)) {
            Ok(name) => Ok(name),
            Err(_) => Err(HanzoNameError::InvalidNameFormat(format!(
                "{}{}",
                node, sender_subidentity
            ))),
        }
    }

    pub fn from_hanzo_message_using_recipient_subidentity(
        message: &HanzoMessage,
    ) -> Result<Self, HanzoNameError> {
        // Check if the message is encrypted
        let body = match &message.body {
            MessageBody::Unencrypted(body) => body,
            _ => {
                return Err(HanzoNameError::InvalidOperation(
                    "Cannot process encrypted HanzoMessage".to_string(),
                ))
            }
        };

        let node = match Self::new(message.external_metadata.recipient.clone()) {
            Ok(name) => name,
            Err(_) => {
                return Err(HanzoNameError::InvalidNameFormat(
                    message.external_metadata.recipient.clone(),
                ))
            }
        };

        let recipient_subidentity = if body.internal_metadata.recipient_subidentity.is_empty() {
            String::from("")
        } else {
            format!("/{}", body.internal_metadata.recipient_subidentity)
        };

        match Self::new(format!("{}{}", node, recipient_subidentity)) {
            Ok(name) => Ok(name),
            Err(_) => Err(HanzoNameError::InvalidNameFormat(format!(
                "{}{}",
                node, recipient_subidentity
            ))),
        }
    }

    pub fn contains(&self, other: &HanzoName) -> bool {
        let self_parts: Vec<&str> = self.full_name.split('/').collect();
        let other_parts: Vec<&str> = other.full_name.split('/').collect();

        if self_parts.len() > other_parts.len() {
            return false;
        }

        self_parts
            .iter()
            .zip(other_parts.iter())
            .all(|(self_part, other_part)| self_part == other_part)
    }

    #[allow(dead_code)]
    pub fn has_profile(&self) -> bool {
        self.profile_name.is_some()
    }

    pub fn has_device(&self) -> bool {
        match self.subidentity_type {
            Some(HanzoSubidentityType::Device) => true,
            _ => false,
        }
    }

    pub fn has_agent(&self) -> bool {
        match self.subidentity_type {
            Some(HanzoSubidentityType::Agent) => true,
            _ => false,
        }
    }

    pub fn has_no_subidentities(&self) -> bool {
        self.profile_name.is_none() && self.subidentity_type.is_none()
    }

    #[allow(dead_code)]
    pub fn get_profile_name_string(&self) -> Option<String> {
        self.profile_name.clone()
    }

    #[allow(dead_code)]
    pub fn get_node_name_string(&self) -> String {
        self.node_name.clone()
    }

    #[allow(dead_code)]
    pub fn get_device_name_string(&self) -> Option<String> {
        if self.has_device() {
            self.subidentity_name.clone()
        } else {
            None
        }
    }

    #[allow(dead_code)]
    pub fn get_agent_name_string(&self) -> Option<String> {
        if self.has_agent() {
            self.subidentity_name.clone()
        } else {
            None
        }
    }

    #[allow(dead_code)]
    pub fn get_fullname_string_without_node_name(&self) -> Option<String> {
        let parts: Vec<&str> = self.full_name.splitn(2, '/').collect();
        parts.get(1).map(|s| s.to_string())
    }

    #[allow(dead_code)]
    pub fn extract_profile(&self) -> Result<Self, &'static str> {
        if self.has_no_subidentities() {
            return Err("This HanzoName does not include a profile.");
        }

        Ok(Self {
            full_name: format!("{}/{}", self.node_name, self.profile_name.as_ref().unwrap()),
            node_name: self.node_name.clone(),
            profile_name: self.profile_name.clone(),
            subidentity_type: None,
            subidentity_name: None,
        })
    }

    #[allow(dead_code)]
    pub fn extract_node(&self) -> Self {
        Self {
            full_name: self.node_name.clone(),
            node_name: self.node_name.clone(),
            profile_name: None,
            subidentity_type: None,
            subidentity_name: None,
        }
    }
}

impl fmt::Display for HanzoName {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.full_name)
    }
}

impl AsRef<str> for HanzoName {
    fn as_ref(&self) -> &str {
        &self.full_name
    }
}

impl Serialize for HanzoName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = self.full_name.clone();
        serializer.serialize_str(&s)
    }
}

impl<'de> Deserialize<'de> for HanzoName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        HanzoName::new(s).map_err(serde::de::Error::custom)
    }
}

impl PartialEq for HanzoName {
    fn eq(&self, other: &Self) -> bool {
        self.full_name.to_lowercase() == other.full_name.to_lowercase()
            && self.node_name.to_lowercase() == other.node_name.to_lowercase()
            && self.profile_name.as_ref().map(|s| s.to_lowercase())
                == other.profile_name.as_ref().map(|s| s.to_lowercase())
            && self.subidentity_type == other.subidentity_type
            && self.subidentity_name.as_ref().map(|s| s.to_lowercase())
                == other.subidentity_name.as_ref().map(|s| s.to_lowercase())
    }
}

impl Hash for HanzoName {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.full_name.to_lowercase().hash(state);
        self.node_name.to_lowercase().hash(state);
        self.profile_name.as_ref().map(|s| s.to_lowercase()).hash(state);
        self.subidentity_type.hash(state);
        self.subidentity_name.as_ref().map(|s| s.to_lowercase()).hash(state);
    }
}

#[derive(Debug, PartialEq)]
#[allow(dead_code)]
pub enum HanzoNameError {
    MissingBody(String),
    MissingInternalMetadata(String),
    MetadataMissing,
    MessageBodyMissing,
    InvalidGroupFormat(String),
    InvalidNameFormat(String),
    SomeError(String),
    InvalidOperation(String),
}

impl fmt::Display for HanzoNameError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            HanzoNameError::MissingBody(message) => {
                write!(f, "Missing body in HanzoMessage: {}", message)
            }
            HanzoNameError::MissingInternalMetadata(message) => {
                write!(f, "Missing internal metadata in HanzoMessage: {}", message)
            }
            HanzoNameError::MetadataMissing => write!(f, "Metadata missing"),
            HanzoNameError::MessageBodyMissing => write!(f, "Message body missing"),
            HanzoNameError::InvalidGroupFormat(message) => {
                write!(f, "Invalid group format: {}", message)
            }
            HanzoNameError::InvalidNameFormat(message) => {
                write!(f, "Invalid name format: {}", message)
            }
            HanzoNameError::SomeError(message) => write!(f, "Some error: {}", message),
            HanzoNameError::InvalidOperation(message) => {
                write!(f, "Invalid operation: {}", message)
            }
        }
    }
}

impl std::error::Error for HanzoNameError {}
