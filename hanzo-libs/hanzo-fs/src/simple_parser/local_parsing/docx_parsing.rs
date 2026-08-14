use std::path::PathBuf;

use hanzo_runtime::functions::parse_docx::parse_docx;

use crate::{
    hanzo_fs_error::HanzoFsError,
    simple_parser::{file_parser_helper::HanzoFileParser, text_group::TextGroup},
};

use super::LocalFileParser;

impl LocalFileParser {
    pub async fn process_docx_file(
        file_path: PathBuf,
        max_node_text_size: u64,
    ) -> Result<Vec<TextGroup>, HanzoFsError> {
        let parsed_docx = parse_docx(file_path)
            .await
            .map_err(|_| HanzoFsError::FailedDOCXParsing)?;

        let mut text_groups = Vec::new();
        HanzoFileParser::push_text_group_by_depth(&mut text_groups, 0, parsed_docx.text, max_node_text_size, None);
        Ok(text_groups)
    }
}
