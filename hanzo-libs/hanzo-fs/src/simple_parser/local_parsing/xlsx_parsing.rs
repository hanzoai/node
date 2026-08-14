use serde_json::Value;
use hanzo_runtime::functions::parse_xlsx::parse_xlsx;

use crate::{hanzo_fs_error::HanzoFsError, simple_parser::text_group::TextGroup};

use std::path::PathBuf;

use super::LocalFileParser;

impl LocalFileParser {
    pub async fn parse_xlsx(file_path: PathBuf) -> Result<Vec<String>, HanzoFsError> {
        let parsed_xlsx = parse_xlsx(file_path)
            .await
            .map_err(|_| HanzoFsError::FailedXLSXParsing)?;

        let parsed_xlsx: Vec<Vec<String>> = parsed_xlsx
            .rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|cell| match cell {
                        Value::String(s) => s.to_string(),
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => b.to_string(),
                        _ => "".to_string(),
                    })
                    .collect::<Vec<String>>()
            })
            .collect();

        let parsed_lines = parsed_xlsx
            .into_iter()
            .map(|row| row.join("|"))
            .collect::<Vec<String>>();
        Ok(parsed_lines)
    }

    pub async fn process_xlsx_file(
        file_path: PathBuf,
        max_node_text_size: u64,
    ) -> Result<Vec<TextGroup>, HanzoFsError> {
        let parsed_xls = parse_xlsx(file_path)
            .await
            .map_err(|_| HanzoFsError::FailedXLSXParsing)?;
        let parsed_xls: Vec<Vec<String>> = parsed_xls
            .rows
            .iter()
            .map(|row| -> Result<Vec<String>, HanzoFsError> {
                Ok(row
                    .iter()
                    .map(|cell| match cell {
                        Value::String(s) => s.to_string(),
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => b.to_string(),
                        _ => "".to_string(),
                    })
                    .collect())
            })
            .collect::<Result<Vec<Vec<String>>, HanzoFsError>>()?;
        let parsed_lines = parsed_xls.into_iter().map(|row| row.join("|")).collect::<Vec<String>>();
        Self::process_table_rows(parsed_lines, max_node_text_size)
    }
}
