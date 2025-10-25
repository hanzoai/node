use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Internal comms preferences
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
pub struct HanzoInternalComms {
    pub internal_has_sync_default_tools: bool,
}
