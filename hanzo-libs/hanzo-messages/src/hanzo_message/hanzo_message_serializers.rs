use super::hanzo_message::HanzoVersion;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

impl Serialize for HanzoVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let version = match *self {
            HanzoVersion::V1_0 => "V1_0",
            HanzoVersion::Unsupported => "Unsupported",
        };
        serializer.serialize_str(version)
    }
}

impl<'de> Deserialize<'de> for HanzoVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let version = String::deserialize(deserializer)?;
        Ok(match version.as_str() {
            "V1_0" => HanzoVersion::V1_0,
            _ => HanzoVersion::Unsupported,
        })
    }
}
