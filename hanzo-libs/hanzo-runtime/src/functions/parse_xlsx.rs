use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;

use crate::{NonRustCodeRunnerFactory, NonRustRuntime, RunError};

#[derive(Debug, Serialize)]
pub struct Input {
    file_path: PathBuf,
}

#[derive(Debug, Deserialize)]
pub struct Output {
    pub rows: Vec<Vec<Value>>,
}

pub async fn parse_xlsx(file_path: PathBuf) -> Result<Output, RunError> {
    println!("parsing xlsx file: {:?}", file_path);
    let code = r#"
            // @deno-types="https://cdn.sheetjs.com/xlsx-0.20.3/package/types/index.d.ts"
            import * as XLSX from 'https://cdn.sheetjs.com/xlsx-0.20.3/package/xlsx.mjs';

            async function run(configurations, params) {
                console.log(params.file_path);
                const workbook = XLSX.read(params.file_path, {type: 'file'});
                const firstSheetName = workbook.SheetNames[0];
                const worksheet = workbook.Sheets[firstSheetName];
                const rows = XLSX.utils.sheet_to_json(worksheet, { header: 1, defval: null});
                console.log("Sheet name: ", firstSheetName);
                return {
                    rows
                };
            }
            "#
    .to_string();
    let runner = NonRustCodeRunnerFactory::new("parse_xlsx", code, vec![file_path.clone()])
        .with_runtime(NonRustRuntime::Deno)
        .create_runner(json!({}));
    runner.run::<_, Output>(Input { file_path }, None).await
}
