use crate::{
    hanzo_message::hanzo_message::{MessageBody, HanzoMessage},
    hanzo_utils::hanzo_logging::{hanzo_log, HanzoLogLevel, HanzoLogOption},
};
use regex::Regex;
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

// Valid Examples
// @@alice.hanzo
// @@alice.hanzo/profileName
// @@alice.hanzo/profileName/agent/myChatGPTAgent
// @@alice.hanzo/profileName/device/myPhone
// @@alice.sep-hanzo
// @@alice.sep-hanzo/profileName

// Not valid examples
// @@alice.hanzo/profileName/myPhone
// @@al!ce.hanzo
// @@alice.hanzo//
// @@node1.hanzo/profile_1.hanzo
// @@alice.sepolia--hanzo

impl HanzoName {
    // Define a list of valid endings
    const VALID_ENDINGS: [&'static str; 4] = [".hanzo", ".sepolia-hanzo", ".arb-sep-hanzo", ".sep-hanzo"];

    pub fn new(raw_name: String) -> Result<Self, &'static str> {
        let raw_name = Self::correct_node_name(raw_name);
        Self::validate_name(&raw_name)?;

        let parts: Vec<&str> = raw_name.split('/').collect();
        let node_name = parts[0].to_string();
        let profile_name = parts.get(1).map(|s| s.to_string());
        let subidentity_type = parts.get(2).map(|s| {
            if *s == "agent" {
                HanzoSubidentityType::Agent
            } else if *s == "device" {
                HanzoSubidentityType::Device
            } else {
                hanzo_log(
                    HanzoLogOption::Identity,
                    HanzoLogLevel::Error,
                    &format!("Invalid subidentity type: {}", s),
                );
                panic!("Invalid subidentity type");
            }
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

        if !(!parts.is_empty() && parts.len() <= 4) {
            hanzo_log(
                HanzoLogOption::Identity,
                HanzoLogLevel::Info,
                &format!(
                    "Name should have one to four parts: node, profile, type (device or agent), and name: {}",
                    raw_name
                ),
            );
            return Err("Name should have one to four parts: node, profile, type (device or agent), and name.");
        }

        if !parts[0].starts_with("@@") || !Self::VALID_ENDINGS.iter().any(|&ending| parts[0].ends_with(ending)) {
            hanzo_log(
                HanzoLogOption::Identity,
                HanzoLogLevel::Info,
                &format!("Validation error: {}", raw_name),
            );
            return Err("Node part of the name should start with '@@' and end with a valid ending ('.hanzo', '.arb-sep-hanzo', '.sep-hanzo', etc.).");
        }

        let node_name_regex = r"^@@[a-zA-Z0-9\_\.]+(\.hanzo|\.arb-sep-hanzo|\.sepolia-hanzo|\.sep-hanzo)$";
        if !Regex::new(node_name_regex).unwrap().is_match(parts[0]) {
            hanzo_log(
                HanzoLogOption::Identity,
                HanzoLogLevel::Info,
                &format!("Node part of the name contains invalid characters: {}", raw_name),
            );
            return Err("Node part of the name contains invalid characters.");
        }

        let re = Regex::new(r"^[a-zA-Z0-9_]*$").unwrap();

        for (index, part) in parts.iter().enumerate() {
            if index == 0 {
                if part.contains('/') {
                    hanzo_log(
                        HanzoLogOption::Identity,
                        HanzoLogLevel::Info,
                        &format!("Root node name cannot contain '/': {}", raw_name),
                    );
                    return Err("Root node name cannot contain '/'.");
                }
                continue;
            }

            if index == 2
                && !(part == &HanzoSubidentityType::Agent.to_string()
                    || part == &HanzoSubidentityType::Device.to_string())
            {
                hanzo_log(
                    HanzoLogOption::Identity,
                    HanzoLogLevel::Info,
                    &format!("The third part should either be 'agent' or 'device': {}", raw_name),
                );
                return Err("The third part should either be 'agent' or 'device'.");
            }

            if index == 3 && !re.is_match(part) {
                hanzo_log(
                    HanzoLogOption::Identity,
                    HanzoLogLevel::Info,
                    &format!(
                        "The fourth part (name after 'agent' or 'device') should be alphanumeric or underscore: {}",
                        raw_name
                    ),
                );
                return Err("The fourth part (name after 'agent' or 'device') should be alphanumeric or underscore.");
            }

            if index != 0 && index != 2 && (!re.is_match(part) || part.contains(".hanzo")) {
                hanzo_log(
                    HanzoLogOption::Identity,
                    HanzoLogLevel::Info,
                    &format!(
                        "Name parts should be alphanumeric or underscore and not contain '.hanzo': {}",
                        raw_name
                    ),
                );
                return Err("Name parts should be alphanumeric or underscore and not contain '.hanzo'.");
            }
        }

        if parts.len() == 3
            && (parts[2] == &HanzoSubidentityType::Agent.to_string()
                || parts[2] == &HanzoSubidentityType::Device.to_string())
        {
            hanzo_log(
                HanzoLogOption::Identity,
                HanzoLogLevel::Info,
                &format!(
                    "If type is 'agent' or 'device', a fourth part is expected: {}",
                    raw_name
                ),
            );
            return Err("If type is 'agent' or 'device', a fourth part is expected.");
        }

        Ok(())
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
        // Validate and format the node_name
        let node_name = Self::correct_node_name(node_name);

        // Construct the full_identity_name
        let full_identity_name = format!("{}/{}", node_name.to_lowercase(), profile_name.to_lowercase());

        // Create a new HanzoName
        Self::new(full_identity_name)
    }

    #[allow(dead_code)]
    pub fn from_node_and_profile_names_and_type_and_name(
        node_name: String,
        profile_name: String,
        hanzo_type: HanzoSubidentityType,
        name: String,
    ) -> Result<Self, &'static str> {
        // Validate and format the node_name
        let node_name = Self::correct_node_name(node_name);

        let hanzo_type_str = hanzo_type.to_string();

        // Construct the full_identity_name
        let full_identity_name = format!(
            "{}/{}/{}/{}",
            node_name.to_lowercase(),
            profile_name.to_lowercase(),
            hanzo_type_str,
            name.to_lowercase()
        );

        // Create a new HanzoName
        Self::new(full_identity_name)
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

    // This method checks if a name is a valid node identity name and doesn't contain subidentities
    #[allow(dead_code)]
    fn is_valid_node_identity_name_and_no_subidentities(name: &String) -> bool {
        // A node name is valid if it starts with '@@', ends with a valid ending, and doesn't contain '/'
        name.starts_with("@@")
            && !name.contains('/')
            && Self::VALID_ENDINGS.iter().any(|&ending| name.ends_with(ending))
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

    fn correct_node_name(raw_name: String) -> String {
        let parts: Vec<&str> = raw_name.splitn(2, '/').collect();

        let mut node_name = parts[0].to_string();

        // Prepend with "@@" if the node doesn't already start with "@@"
        if !node_name.starts_with("@@") {
            node_name = format!("@@{}", node_name);
        }

        // Check if the node_name ends with any of the valid endings, append ".hanzo" if not
        if !Self::VALID_ENDINGS.iter().any(|&ending| node_name.ends_with(ending)) {
            node_name = format!("{}.hanzo", node_name);
        }

        // Reconstruct the name

        if parts.len() > 1 {
            format!("{}/{}", node_name, parts[1])
        } else {
            node_name
        }
    }

    pub fn default_testnet_localhost() -> Self {
        HanzoName::new("@@localhost.sep-hanzo/main".to_string())
            .expect("Failed to create default testnet localhost HanzoName")
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
