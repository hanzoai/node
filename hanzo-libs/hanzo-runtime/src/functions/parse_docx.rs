use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

use crate::{NonRustCodeRunnerFactory, NonRustRuntime, RunError};

#[derive(Debug, Serialize)]
pub struct Input {
    file_path: PathBuf,
}

#[derive(Debug, Deserialize)]
pub struct Output {
    pub text: String,
}

pub async fn parse_docx(file_path: PathBuf) -> Result<Output, RunError> {
    println!("parsing docx file: {:?}", file_path);
    let code = r#"
            import mammoth from 'npm:mammoth';
            import TurndownService from 'npm:turndown';
            import turndownPluginGfm from 'npm:turndown-plugin-gfm';
            const gfm = turndownPluginGfm.gfm;
            const tables = turndownPluginGfm.tables;

            const turndownService = new TurndownService();
            turndownService.use(gfm);
            turndownService.use([tables]);



            async function run(configurations, params) {
                console.log(params.file_path);
                const htmlResult = await mammoth.convertToHtml({ path: params.file_path });
                const markdownResult = turndownService.turndown(htmlResult.value);
                return {
                    text: markdownResult
                };
            }
            "#
    .to_string();
    let runner = NonRustCodeRunnerFactory::new("parse_docx", code, vec![file_path.clone()])
        .with_runtime(NonRustRuntime::Deno)
        .create_runner(json!({}));
    runner.run::<_, Output>(Input { file_path }, None).await
}
