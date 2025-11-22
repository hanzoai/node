// Local type definitions for sheet types that were previously in hanzo-message-primitives
use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

pub type UuidString = String;
pub type ColumnUuid = String;
pub type RowUuid = String;

// CellId is a newtype wrapper around String
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct CellId(pub String);

impl CellId {
    pub fn new(id: String) -> Self {
        CellId(id)
    }
}

impl From<String> for CellId {
    fn from(s: String) -> Self {
        CellId(s)
    }
}

impl From<&str> for CellId {
    fn from(s: &str) -> Self {
        CellId(s.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Cell {
    pub id: CellId,
    pub value: Option<String>,
    pub status: CellStatus,
    pub input_hash: Option<String>,
    pub last_updated: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CellStatus {
    Empty,
    Filled,
    Computing,
    Ready,
    Pending,
    Waiting,
    Error(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CellUpdateData {
    pub cell_id: CellId,
    pub value: Option<String>,
    pub row_id: RowUuid,
    pub column_id: ColumnUuid,
    pub input_hash: Option<String>,
    pub status: CellStatus,
    pub last_updated: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CellUpdateInfo {
    pub cell_id: CellId,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub sheet_id: String,
    pub update_type: String,
    pub data: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ColumnDefinition {
    pub id: ColumnUuid,  // Alias for uuid
    pub uuid: ColumnUuid,
    pub name: String,
    pub behavior: ColumnBehavior,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ColumnBehavior {
    Text,
    Number,
    Formula(String),
    LLM {
        prompt_template: String,
        model: Option<String>,
    },
    LLMCall {
        input: String,
        llm_provider_name: Option<String>,
        input_hash: Option<String>,
    },
    MultipleVRFiles,
    UploadedFiles,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowSheetJobData {
    pub sheet_id: String,
    pub cell_updates: Vec<CellUpdateData>,
    pub row: RowUuid,
    pub col: ColumnUuid,
    pub col_definition: ColumnDefinition,
    pub input_cells: Vec<Cell>,
    pub llm_provider_name: Option<String>,
}
