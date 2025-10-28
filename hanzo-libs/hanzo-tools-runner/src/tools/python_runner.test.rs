use std::collections::HashMap;

use crate::tools::runner_type::RunnerType;
use rstest::rstest;
use serde_json::{json, Value};

use crate::tools::execution_context::ExecutionContext;
use crate::tools::python_runner_options::PythonRunnerOptions;
use crate::tools::hanzo_node_location::HanzoNodeLocation;
use crate::tools::{code_files::CodeFiles, python_runner::PythonRunner};

#[rstest]
#[case::host(RunnerType::Host)]
#[case::docker(RunnerType::Docker)]
#[tokio::test]
async fn run_echo_tool(#[case] runner_type: RunnerType) {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let code_files = CodeFiles {
        files: HashMap::from([(
            "main.py".to_string(),
            r#"
import json

def run(configurations, parameters):
    value = { 'message': 'hello world' }
    return value
            "#
            .to_string(),
        )]),
        entrypoint: "main.py".to_string(),
    };
    let python_runner = PythonRunner::new(
        code_files,
        Value::Null,
        Some(PythonRunnerOptions {
            force_runner_type: Some(runner_type),
            ..Default::default()
        }),
    );

    let result = python_runner
        .run(None, serde_json::Value::Null, None)
        .await
        .map_err(|e| {
            log::error!("Failed to run python code: {}", e);
            e
        })
        .unwrap();

    assert_eq!(result.data.get("message").unwrap(), "hello world");
}

#[rstest]
#[case::host(RunnerType::Host)]
#[case::docker(RunnerType::Docker)]
#[tokio::test]
async fn run_with_env(#[case] runner_type: RunnerType) {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let code_files = CodeFiles {
        files: HashMap::from([(
            "main.py".to_string(),
            r#"
import os
def run(configurations, parameters):
    return os.getenv('HELLO_WORLD')
                "#
            .to_string(),
        )]),
        entrypoint: "main.py".to_string(),
    };
    let python_runner = PythonRunner::new(
        code_files,
        Value::Null,
        Some(PythonRunnerOptions {
            force_runner_type: Some(runner_type),
            ..Default::default()
        }),
    );

    let mut envs = HashMap::<String, String>::new();
    envs.insert("HELLO_WORLD".to_string(), "hello world!".to_string()); // Insert the key-value pair
    let result = python_runner
        .run(Some(envs), serde_json::Value::Null, None)
        .await
        .unwrap();

    assert_eq!(result.data.as_str().unwrap(), "hello world!");
}

#[rstest]
#[case::host(RunnerType::Host, "127.0.0.2")]
#[case::docker(RunnerType::Docker, "host.docker.internal")]
#[tokio::test]
async fn run_with_hanzo_node_location_host(
    #[case] runner_type: RunnerType,
    #[case] expected_host: String,
) {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let code = r#"
import os
def run(configurations, parameters):
    return os.getenv('SHINKAI_NODE_LOCATION')
            "#;

    let code_files = CodeFiles {
        files: HashMap::from([("main.py".to_string(), code.to_string())]),
        entrypoint: "main.py".to_string(),
    };

    let python_runner = PythonRunner::new(
        code_files,
        Value::Null,
        Some(PythonRunnerOptions {
            hanzo_node_location: HanzoNodeLocation {
                protocol: String::from("https"),
                host: String::from("127.0.0.2"),
                port: 9554,
            },
            force_runner_type: Some(runner_type),
            ..Default::default()
        }),
    );

    let result = python_runner
        .run(None, serde_json::Value::Null, None)
        .await
        .unwrap();
    assert_eq!(
        result.data.as_str().unwrap(),
        format!("https://{}:9554", expected_host)
    );
}

#[rstest]
#[case::host(RunnerType::Host)]
#[case::docker(RunnerType::Docker)]
#[tokio::test]
async fn run_with_file_sub_path(#[case] runner_type: RunnerType) {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();
    let code_files = CodeFiles {
        files: HashMap::from([(
            "potato_a/potato_b/main.py".to_string(),
            r#"
def run(configurations, parameters):
    return "hello world"
        "#
            .to_string(),
        )]),
        entrypoint: "potato_a/potato_b/main.py".to_string(),
    };
    let python_runner = PythonRunner::new(
        code_files,
        Value::Null,
        Some(PythonRunnerOptions {
            hanzo_node_location: HanzoNodeLocation {
                protocol: String::from("https"),
                host: String::from("127.0.0.2"),
                port: 9554,
            },
            force_runner_type: Some(runner_type),
            ..Default::default()
        }),
    );

    let result = python_runner.run(None, Value::Null, None).await.unwrap();
    assert_eq!(result.data.as_str().unwrap(), "hello world");
}

#[rstest]
#[case::host(RunnerType::Host)]
#[case::docker(RunnerType::Docker)]
#[tokio::test]
async fn run_with_imports(#[case] runner_type: RunnerType) {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let code_files = CodeFiles {
        files: HashMap::from([
            (
                "main.py".to_string(),
                r#"
from secondary import hello
def run(configurations, parameters):
    return hello
                "#
                .to_string(),
            ),
            (
                "secondary.py".to_string(),
                r#"
hello = 'hello world'
                "#
                .to_string(),
            ),
        ]),
        entrypoint: "main.py".to_string(),
    };

    let python_runner = PythonRunner::new(
        code_files,
        Value::Null,
        Some(PythonRunnerOptions {
            force_runner_type: Some(runner_type),
            ..Default::default()
        }),
    );

    let result = python_runner.run(None, Value::Null, None).await.unwrap();
    assert_eq!(result.data.as_str().unwrap(), "hello world");
}

#[tokio::test]
async fn check_code_success() {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let code_files = CodeFiles {
        files: HashMap::from([(
            "main.py".to_string(),
            String::from(
                r#"
def run(configurations, parameters):
    return "Hello world from successful test"
                "#,
            ),
        )]),
        entrypoint: "main.py".to_string(),
    };

    let python_runner = PythonRunner::new(
        code_files,
        Value::Null,
        Some(PythonRunnerOptions {
            ..Default::default()
        }),
    );

    let check_result = python_runner.check().await.unwrap();
    assert_eq!(check_result.len(), 0);
}

#[rstest]
#[case::host(RunnerType::Host)]
#[tokio::test]
async fn check_code_with_errors(#[case] runner_type: RunnerType) {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let code_files = CodeFiles {
        files: HashMap::from([(
            "main.py".to_string(),
            String::from(
                r#"
print('test's)
                "#,
            ),
        )]),
        entrypoint: "main.py".to_string(),
    };

    let python_runner = PythonRunner::new(
        code_files,
        Value::Null,
        Some(PythonRunnerOptions {
            force_runner_type: Some(runner_type),
            ..Default::default()
        }),
    );

    let check_result = python_runner.check().await.unwrap();
    assert!(!check_result.is_empty());
    assert!(check_result.iter().any(|err| err.contains("Expected ','")));
}

#[rstest]
#[case::host(RunnerType::Host)]
#[tokio::test]
async fn check_code_with_unexisting_fn(#[case] runner_type: RunnerType) {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let code_files = CodeFiles {
        files: HashMap::from([(
            "main.py".to_string(),
            String::from(
                r#"
print('hello', hello())
                "#,
            ),
        )]),
        entrypoint: "main.py".to_string(),
    };

    let python_runner = PythonRunner::new(
        code_files,
        Value::Null,
        Some(PythonRunnerOptions {
            force_runner_type: Some(runner_type),
            ..Default::default()
        }),
    );

    let check_result = python_runner.check().await.unwrap();
    assert!(!check_result.is_empty());
    assert!(check_result
        .iter()
        .any(|err| err.contains("Undefined name `hello`")));
}

#[rstest]
#[case::host(RunnerType::Host)]
#[tokio::test]
async fn check_code_with_import_with_error(#[case] runner_type: RunnerType) {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let code_files = CodeFiles {
        files: HashMap::from([
            (
                "secondary.py".to_string(),
                String::from(
                    r#"
def hellow():
    return 'hello world' + world()
                "#,
                ),
            ),
            (
                "main.py".to_string(),
                String::from(
                    r#"
import secondary
print('hello', secondary.hello())
                "#,
                ),
            ),
        ]),
        entrypoint: "main.py".to_string(),
    };

    let python_runner = PythonRunner::new(
        code_files,
        Value::Null,
        Some(PythonRunnerOptions {
            force_runner_type: Some(runner_type),
            ..Default::default()
        }),
    );

    let check_result = python_runner.check().await.unwrap();
    assert!(!check_result.is_empty());
    assert!(check_result
        .iter()
        .any(|err| err.contains("Undefined name `world`")));
}

#[rstest]
#[case::host(RunnerType::Host)]
#[case::docker(RunnerType::Docker)]
#[tokio::test]
async fn run_with_import_library(#[case] runner_type: RunnerType) {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let code_files = CodeFiles {
        files: HashMap::from([(
            "main.py".to_string(),
            r#"
# /// script
# dependencies = [
#     "requests"
# ]
# ///
import requests
def run(configurations, parameters):
    response = requests.get('https://jsonplaceholder.typicode.com/todos/1')
    return response.json()['id']
                "#
            .to_string(),
        )]),
        entrypoint: "main.py".to_string(),
    };

    let python_runner = PythonRunner::new(
        code_files,
        Value::Null,
        Some(PythonRunnerOptions {
            force_runner_type: Some(runner_type),
            ..Default::default()
        }),
    );

    let result = python_runner
        .run(None, Value::Null, None)
        .await
        .map_err(|e| {
            log::error!("Failed to run python code: {}", e);
            e
        })
        .unwrap();

    assert_eq!(result.data.as_number().unwrap().as_i64().unwrap(), 1);
}

#[rstest]
#[case::host(RunnerType::Host)]
#[case::docker(RunnerType::Docker)]
#[tokio::test]
async fn hanzo_tool_run_concurrency(#[case] runner_type: RunnerType) {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();
    let js_code1 = r#"
# /// script
# dependencies = [
#     "requests"
# ]
# ///
import requests
def run(configurations, params):
    response = requests.get('https://jsonplaceholder.typicode.com/todos/1')
    return {
        'status': response.status_code,
        'data': response.json()
    }
    "#;

    let code_files1 = CodeFiles {
        files: HashMap::from([("main.py".to_string(), js_code1.to_string())]),
        entrypoint: "main.py".to_string(),
    };

    let js_code2 = r#"
def run(configurations, params):
    return {
        'foo': 1 + 2
    }
    "#;

    let code_files2 = CodeFiles {
        files: HashMap::from([("main.py".to_string(), js_code2.to_string())]),
        entrypoint: "main.py".to_string(),
    };

    let js_code3 = r#"
def run(configurations, params):
    return {
        'foo': sum([1, 2, 3, 4])
    }
    "#;

    let code_files3 = CodeFiles {
        files: HashMap::from([("main.py".to_string(), js_code3.to_string())]),
        entrypoint: "main.py".to_string(),
    };

    let execution_storage = "./hanzo-tools-runner-execution-storage";
    let context_id = String::from("context-patata");
    let execution_id = String::from("2");
    let tool1 = PythonRunner::new(
        code_files1,
        serde_json::Value::Null,
        Some(PythonRunnerOptions {
            context: ExecutionContext {
                storage: execution_storage.into(),
                execution_id: execution_id.clone(),
                context_id: context_id.clone(),
                code_id: "js_code1".into(),
                ..Default::default()
            },
            force_runner_type: Some(runner_type),
            ..Default::default()
        }),
    );
    let tool2 = PythonRunner::new(
        code_files2,
        serde_json::Value::Null,
        Some(PythonRunnerOptions {
            context: ExecutionContext {
                storage: execution_storage.into(),
                execution_id: execution_id.clone(),
                context_id: context_id.clone(),
                code_id: "js_code2".into(),
                ..Default::default()
            },
            ..Default::default()
        }),
    );
    let tool3 = PythonRunner::new(
        code_files3,
        serde_json::Value::Null,
        Some(PythonRunnerOptions {
            context: ExecutionContext {
                storage: execution_storage.into(),
                execution_id: execution_id.clone(),
                context_id: context_id.clone(),
                code_id: "js_code3".into(),
                ..Default::default()
            },
            ..Default::default()
        }),
    );

    let (result1, result2, result3) = tokio::join!(
        tool1.run(None, serde_json::json!({ "name": "world" }), None),
        tool2.run(None, serde_json::Value::Null, None),
        tool3.run(None, serde_json::Value::Null, None)
    );

    let run_result1 = result1.unwrap();
    let run_result2 = result2.unwrap();
    let run_result3 = result3.unwrap();

    assert_eq!(run_result1.data["status"], 200);
    assert_eq!(run_result2.data["foo"], 3);
    assert_eq!(run_result3.data["foo"], 10);
}

#[rstest]
#[case::host(RunnerType::Host)]
#[case::docker(RunnerType::Docker)]
#[tokio::test]
async fn file_persistence_in_home(#[case] runner_type: RunnerType) {
    let code_files = CodeFiles {
        files: HashMap::from([(
            "main.py".to_string(),
            r#"
import os
import pathlib

async def run(c, p):
    content = "Hello from tool!"
    print("Current directory contents:")
    for entry in os.listdir("./"):
        print(entry)
    
    home_path = pathlib.Path(os.environ["SHINKAI_HOME"]) / "test.txt"
    with open(home_path, "w") as f:
        f.write(content)
    
    data = {"success": True}
    return data
    "#
            .to_string(),
        )]),
        entrypoint: "main.py".to_string(),
    };

    let execution_storage = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("hanzo-tools-runner-execution-storage");
    let context_id = nanoid::nanoid!();

    let tool = PythonRunner::new(
        code_files,
        serde_json::Value::Null,
        Some(PythonRunnerOptions {
            context: ExecutionContext {
                storage: execution_storage.clone(),
                context_id: context_id.clone(),
                code_id: "js_code".into(),
                ..Default::default()
            },
            force_runner_type: Some(runner_type),
            ..Default::default()
        }),
    );

    let result = tool.run(None, serde_json::Value::Null, None).await.unwrap();
    assert_eq!(result.data["success"], true);

    let file_path = execution_storage.join(format!("{}/home/test.txt", context_id));
    assert!(file_path.exists());
}

#[rstest]
#[case::host(RunnerType::Host)]
#[case::docker(RunnerType::Docker)]
#[tokio::test]
async fn mount_file_in_mount(#[case] runner_type: RunnerType) {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let test_file_path = tempfile::NamedTempFile::new().unwrap().into_temp_path();
    std::fs::create_dir_all(test_file_path.parent().unwrap()).unwrap();
    println!("test file path: {:?}", test_file_path);
    std::fs::write(&test_file_path, "1").unwrap();

    let code_files = CodeFiles {
        files: HashMap::from([(
            "main.py".to_string(),
            r#"
import os

def run(c, p):
    mount = os.environ["SHINKAI_MOUNT"].split(',')
    for file in mount:
        print("file in mount: ", file)
    with open(mount[0]) as f:
        content = f.read()
    print(content)
    return content
"#
            .to_string(),
        )]),
        entrypoint: "main.py".to_string(),
    };

    let tool = PythonRunner::new(
        code_files,
        serde_json::Value::Null,
        Some(PythonRunnerOptions {
            context: ExecutionContext {
                mount_files: vec![test_file_path.to_path_buf().clone()],
                ..Default::default()
            },
            force_runner_type: Some(runner_type),
            ..Default::default()
        }),
    );
    let mut envs = HashMap::new();
    envs.insert(
        "FILE_NAME".to_string(),
        test_file_path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string(),
    );
    let result = tool.run(Some(envs), Value::Null, None).await;
    assert!(result.is_ok());
    assert!(result.unwrap().data == "1");
}

#[rstest]
#[case::host(RunnerType::Host)]
#[case::docker(RunnerType::Docker)]
#[tokio::test]
async fn mount_and_edit_file_in_mount(#[case] runner_type: RunnerType) {
    use std::path;

    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let tp = tempfile::NamedTempFile::new().unwrap().path().to_path_buf();
    let test_file_path = path::absolute(tp).unwrap();
    println!("test file path: {:?}", test_file_path);
    std::fs::write(&test_file_path, "1").unwrap();

    let execution_storage = std::path::PathBuf::from("./hanzo-tools-runner-execution-storage");

    let code_files = CodeFiles {
        files: HashMap::from([(
            "main.py".to_string(),
            r#"
import os

def run(c, p):
    mount = os.environ["SHINKAI_MOUNT"].split(',')
    with open(mount[0], 'w') as f:
        f.write("2")
    return None
"#
            .to_string(),
        )]),
        entrypoint: "main.py".to_string(),
    };

    let tool = PythonRunner::new(
        code_files,
        serde_json::Value::Null,
        Some(PythonRunnerOptions {
            context: ExecutionContext {
                storage: execution_storage.clone(),
                mount_files: vec![test_file_path.to_path_buf().clone()],
                ..Default::default()
            },
            force_runner_type: Some(runner_type),
            ..Default::default()
        }),
    );
    let mut envs = HashMap::new();
    envs.insert(
        "FILE_NAME".to_string(),
        test_file_path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string(),
    );
    let result = tool.run(Some(envs), Value::Null, None).await;
    assert!(result.is_ok());
    assert!(result.unwrap().data == serde_json::Value::Null);

    let content = std::fs::read_to_string(&test_file_path).unwrap();
    assert_eq!(content, "2");
}

#[rstest]
#[case::host(RunnerType::Host)]
#[case::docker(RunnerType::Docker)]
#[tokio::test]
async fn mount_file_in_assets(#[case] runner_type: RunnerType) {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let test_file_path = tempfile::NamedTempFile::new().unwrap().into_temp_path();
    println!("test file path: {:?}", test_file_path);
    std::fs::write(&test_file_path, "1").unwrap();

    let code_files = CodeFiles {
        files: HashMap::from([(
            "main.py".to_string(),
            r#"
import os

def run(c, p):
    assets = os.environ["SHINKAI_ASSETS"].split(',')
    with open(assets[0]) as f:
        content = f.read()
    print(content)
    return content
"#
            .to_string(),
        )]),
        entrypoint: "main.py".to_string(),
    };

    let tool = PythonRunner::new(
        code_files,
        serde_json::Value::Null,
        Some(PythonRunnerOptions {
            context: ExecutionContext {
                assets_files: vec![test_file_path.to_path_buf().clone()],
                ..Default::default()
            },
            force_runner_type: Some(runner_type),
            ..Default::default()
        }),
    );
    let mut envs = HashMap::new();
    envs.insert(
        "FILE_NAME".to_string(),
        test_file_path
            .to_path_buf()
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string(),
    );
    let result = tool.run(Some(envs), Value::Null, None).await;
    assert!(result.is_ok());
    assert!(result.unwrap().data == "1");
}

#[rstest]
#[case::host(RunnerType::Host)]
#[case::docker(RunnerType::Docker)]
#[tokio::test]
async fn hanzo_tool_param_with_quotes(#[case] runner_type: RunnerType) {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let code_files = CodeFiles {
        files: HashMap::from([(
            "main.py".to_string(),
            r#"
def run(configurations, params):
    return {
        'single': params['single'],
        'double': params['double'],
        'backtick': params['backtick'],
        'mixed': params['mixed'],
        'escaped': params['escaped']
    }
"#
            .to_string(),
        )]),
        entrypoint: "main.py".to_string(),
    };

    let tool = PythonRunner::new(
        code_files,
        serde_json::Value::Null,
        Some(PythonRunnerOptions {
            force_runner_type: Some(runner_type),
            ..Default::default()
        }),
    );
    let run_result = tool
        .run(
            None,
            serde_json::json!({
                "single": "bar's quote",
                "double": "she said \"hello\"",
                "backtick": "using `backticks`",
                "mixed": "single ' and double \" quotes",
                "escaped": "escaped \' and \" quotes"
            }),
            None,
        )
        .await;
    assert!(run_result.is_ok());
    let result = run_result.unwrap().data;
    assert_eq!(result["single"], "bar's quote");
    assert_eq!(result["double"], "she said \"hello\"");
    assert_eq!(result["backtick"], "using `backticks`");
    assert_eq!(result["mixed"], "single ' and double \" quotes");
    assert_eq!(result["escaped"], "escaped \' and \" quotes");
}

#[rstest]
#[case::host(RunnerType::Host)]
#[case::docker(RunnerType::Docker)]
#[tokio::test]
async fn multiple_file_imports(#[case] runner_type: RunnerType) {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let code_files = CodeFiles {
        files: HashMap::from([
            (
                "main.py".to_string(),
                r#"
from helper import helper
from data import data

def run(configurations, params):
    return helper(data)
"#
                .to_string(),
            ),
            (
                "helper.py".to_string(),
                r#"
def helper(input):
    return f"processed {input}"
"#
                .to_string(),
            ),
            (
                "data.py".to_string(),
                r#"
data = "test data"
"#
                .to_string(),
            ),
        ]),
        entrypoint: "main.py".to_string(),
    };

    let tool = PythonRunner::new(
        code_files,
        Value::Null,
        Some(PythonRunnerOptions {
            force_runner_type: Some(runner_type),
            ..Default::default()
        }),
    );
    let result = tool.run(None, Value::Null, None).await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap().data, "processed test data");
}

#[rstest]
#[case::host(RunnerType::Host)]
#[case::docker(RunnerType::Docker)]
#[tokio::test]
async fn context_and_execution_id(#[case] runner_type: RunnerType) {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let context_id = nanoid::nanoid!();
    let execution_id = nanoid::nanoid!();

    let code_files = CodeFiles {
        files: HashMap::from([(
            "main.py".to_string(),
            r#"
import os

def run(configurations, params):
    return {
        'contextId': os.environ['SHINKAI_CONTEXT_ID'],
        'executionId': os.environ['SHINKAI_EXECUTION_ID']
    }
"#
            .to_string(),
        )]),
        entrypoint: "main.py".to_string(),
    };

    let tool = PythonRunner::new(
        code_files,
        Value::Null,
        Some(PythonRunnerOptions {
            force_runner_type: Some(runner_type),
            context: ExecutionContext {
                context_id: context_id.clone(),
                execution_id: execution_id.clone(),
                ..Default::default()
            },
            ..Default::default()
        }),
    );
    let result = tool.run(None, Value::Null, None).await.unwrap();

    assert_eq!(result.data["contextId"], context_id);
    assert_eq!(result.data["executionId"], execution_id);
}

#[rstest]
#[case::host(RunnerType::Host)]
#[case::docker(RunnerType::Docker)]
#[tokio::test]
async fn run_output_class_object(#[case] runner_type: RunnerType) {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let code_files = CodeFiles {
        files: HashMap::from([(
            "main.py".to_string(),
            r#"
import json

class Potato:
    def __init__(self):
        self.kind = 'vegetable'

def run(configurations, parameters):
    potato = Potato()
    return potato
    "#
            .to_string(),
        )]),
        entrypoint: "main.py".to_string(),
    };

    let python_runner = PythonRunner::new(
        code_files,
        Value::Null,
        Some(PythonRunnerOptions {
            force_runner_type: Some(runner_type),
            ..Default::default()
        }),
    );

    let result = python_runner
        .run(None, serde_json::Value::Null, None)
        .await
        .map_err(|e| {
            log::error!("Failed to run python code: {}", e);
            e
        })
        .unwrap();

    assert_eq!(result.data.get("kind").unwrap(), "vegetable");
}

#[rstest]
#[case::host(RunnerType::Host)]
#[case::docker(RunnerType::Docker)]
#[tokio::test]
async fn run_pip_lib_name_neq_to_import_name(#[case] runner_type: RunnerType) {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let code_files = CodeFiles {
        files: HashMap::from([(
            "main.py".to_string(),
            r#"
# /// script
# dependencies = [
#   "googlesearch-python",
# ]
# ///
from googlesearch import search, SearchResult
from typing import List
from dataclasses import dataclass

class CONFIG:
    pass

class INPUTS:
    query: str
    num_results: int = 10

class OUTPUT:
    results: List[SearchResult]
    query: str

async def run(c: CONFIG, p: INPUTS) -> OUTPUT:
    query = p.query
    if not query:
        raise ValueError("No search query provided")

    results = []
    try:
        results = search(query, num_results=p.num_results, advanced=True)
    except Exception as e:
        raise RuntimeError(f"Search failed: {str(e)}")

    output = OUTPUT()
    output.results = results
    output.query = query
    return output
    "#
            .to_string(),
        )]),
        entrypoint: "main.py".to_string(),
    };

    let python_runner = PythonRunner::new(
        code_files,
        Value::Null,
        Some(PythonRunnerOptions {
            force_runner_type: Some(runner_type),
            ..Default::default()
        }),
    );

    let result = python_runner
        .run(
            None,
            serde_json::json!({ "query": "macbook pro m4", "num_results": 5 }),
            None,
        )
        .await
        .map_err(|e| {
            log::error!("Failed to run python code: {}", e);
            e
        })
        .unwrap();

    let results_length = result
        .data
        .get("results")
        .unwrap()
        .as_array()
        .unwrap()
        .len();
    assert!(
        results_length > 0 && results_length <= 5,
        "results should be an array with 0 to 5 elements"
    );
    assert!(!result
        .data
        .get("query")
        .unwrap()
        .as_str()
        .unwrap()
        .is_empty());
}

/*
    This test utilizes the hidden `tricky_json_dump` function, which is part of the engine.
    This function serves as a tricky way to test the engine's serialization capabilities
    without requiring extensive setup.
*/
#[rstest]
// #[case::host(RunnerType::Host)]
#[case::docker(RunnerType::Docker)]
#[tokio::test]
async fn tricky_json_dump(#[case] runner_type: RunnerType) {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let code_files = CodeFiles {
        files: HashMap::from([(
            "main.py".to_string(),
            r#"
# /// script
# dependencies = [
#   "googlesearch-python",
# ]
# ///

import asyncio
from googlesearch import SearchResult
from typing import List

from datetime import datetime
from typing import Dict, Optional

class CONFIG:
    pass

class INPUTS:
    pass

class OUTPUT:
    pass

class AnyClass1:
    query: str
    num_results: int = 10
    timestamp: datetime
    def __init__(self, query: str):
        self.query = query
        self.timestamp = datetime.now()
        self.num_results = 10

    def any_method_1(self):
        return "any_method_1"

class AnyClass2:
    results: List[SearchResult]
    query: str
    status_code: Optional[int] = None

class VeryComplexClass:
    results: List[SearchResult]
    query: str
    input: AnyClass1
    number: int
    output: AnyClass2
    unique_ids: set
    metadata: Dict[str, str]
    additional_info: Optional[str]  # New attribute for extra information
    status: str  # New attribute to track the status of the class
    creation_date: datetime  # New attribute to track the creation date
    slice: slice  # New attribute to represent a slice of data
    complex_number: complex
    byte_array: bytearray
    memoryview: memoryview
    frozen_set: frozenset
    def cry(self):
        return "cry"

def create_very_complex_class():
    search_result = SearchResult(title="potato", url="https://potato.com", description="potato is a vegetable")
    search_result2 = SearchResult(title="tomato", url="https://tomato.com", description="tomato is a fruit")  # Additional search result
    
    search_results = list[SearchResult]()
    search_results.append(search_result)
    search_results.append(search_result2)

    any_class_2 = AnyClass2()
    any_class_2.results = iter(search_results)
    any_class_2.query = "something about potatoes and tomatoes"  # Updated query
    any_class_2.status_code = 200

    very_complex_class = VeryComplexClass()
    very_complex_class.results = any_class_2.results
    very_complex_class.query = any_class_2.query
    very_complex_class.number = 5
    very_complex_class.input = AnyClass1(query="potato")
    very_complex_class.output = any_class_2
    very_complex_class.unique_ids = set()
    very_complex_class.metadata = {"source": "google", "category": "vegetable"}
    
    very_complex_class.unique_ids.add("potato_id_1")
    very_complex_class.unique_ids.add("tomato_id_1")  # Adding unique ID for the new search result
    very_complex_class.metadata["potato_name"] = "potato"
    very_complex_class.metadata["tomato_name"] = "tomato"  # New metadata for tomato
    very_complex_class.metadata["potato_search_results"] = str(search_results)
    very_complex_class.metadata["additional_info"] = "This class contains search results for vegetables and fruits."  # New metadata
    very_complex_class.status = "active"  # Setting the status
    very_complex_class.creation_date = datetime.now()  # Setting the creation date
    very_complex_class.slice = slice(6)  # Example slice initialization
    very_complex_class.complex_number = 4+3j
    very_complex_class.byte_array = bytearray(b"Hello, World!")
    very_complex_class.memoryview = memoryview(b"Hello, World!")
    very_complex_class.frozen_set = frozenset([1, 2, 3])
    return very_complex_class

async def run(c: CONFIG, p: INPUTS) -> OUTPUT:
    very_complex_class = create_very_complex_class()
    json_dump = tricky_json_dump(very_complex_class)
    print("json:", json_dump)

    loaded_data = json.loads(json_dump)

    # Assertions to validate the loaded data
    assert isinstance(loaded_data, dict), "Loaded data should be a dictionary"
    assert "results" in loaded_data, "'results' key should be present in the loaded data"
    assert isinstance(loaded_data["results"], list), "'results' should be a list"
    assert len(loaded_data["results"]) == 2, "There should be two search results"
    assert all("title" in result for result in loaded_data["results"]), "Each result should have a 'title'"
    assert all("url" in result for result in loaded_data["results"]), "Each result should have a 'url'"
    assert all("description" in result for result in loaded_data["results"]), "Each result should have a 'description'"

    # Additional assertions for metadata
    assert "query" in loaded_data, "'query' key should be present in the loaded data"
    assert loaded_data["query"] == "something about potatoes and tomatoes", "Query should match the expected value"
    assert "status_code" in loaded_data.get("output", {}), "'status_code' key should be present in the loaded data"
    assert loaded_data.get("output", {}).get("status_code") == 200, "Status code should be 200"
    assert "metadata" in loaded_data, "'metadata' key should be present in the loaded data"
    assert "source" in loaded_data.get("metadata", {}), "'source' should be present in metadata"
    assert len(loaded_data.get("output", {}).get("results")) == 2, "output.results should be 2"

    return loaded_data
    "#
            .to_string(),
        )]),
        entrypoint: "main.py".to_string(),
    };

    let python_runner = PythonRunner::new(
        code_files,
        Value::Null,
        Some(PythonRunnerOptions {
            force_runner_type: Some(runner_type),
            ..Default::default()
        }),
    );

    let result = python_runner
        .run(
            None,
            serde_json::json!({ "query": "macbook pro m4", "num_results": 5 }),
            None,
        )
        .await
        .map_err(|e| {
            log::error!("Failed to run python code: {}", e);
            e
        });

    assert!(result.is_ok());
}

#[rstest]
#[case::host(RunnerType::Host)]
//#[case::docker(RunnerType::Docker)]
#[tokio::test]
async fn override_python_version(#[case] runner_type: RunnerType) {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let code_files = CodeFiles {
        files: HashMap::from([(
            "main.py".to_string(),
            r#"
# /// script
# requires-python = "==3.10.*"
# ///
import sys

class CONFIG:
    pass

class INPUTS:
    pass

class OUTPUT:
    version: str
    pass

async def run(c: CONFIG, p: INPUTS) -> OUTPUT:
    import sys
    print(f"Python version: {sys.version}")
    output = OUTPUT()
    output.version = sys.version
    return output
    "#
            .to_string(),
        )]),
        entrypoint: "main.py".to_string(),
    };

    let python_runner = PythonRunner::new(
        code_files,
        Value::Null,
        Some(PythonRunnerOptions {
            force_runner_type: Some(runner_type),
            ..Default::default()
        }),
    );

    let result = python_runner
        .run(None, serde_json::json!({}), None)
        .await
        .map_err(|e| {
            log::error!("Failed to run python code: {}", e);
            e
        })
        .unwrap();

    let version = result.data.get("version").unwrap().as_str().unwrap();
    assert!(version.contains("3.10"), "version should be 3.10");
}

#[rstest]
#[case::host(RunnerType::Host)]
#[case::docker(RunnerType::Docker)]
#[tokio::test]
async fn rembg_with_python_3_10(#[case] runner_type: RunnerType) {
    use std::path::PathBuf;

    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let code_files = CodeFiles {
        files: HashMap::from([(
            "main.py".to_string(),
            r#"
# /// script
# requires-python = "==3.12.0"
# dependencies = [
#   "rembg[cpu]",
#   "numba==0.59.0",
# ]
# ///
import requests
import sys
import os
from rembg import remove

class CONFIG:
    pass

class INPUTS:
    path: str

class OUTPUT:
    version: str
    pass

async def run(c: CONFIG, p: INPUTS) -> OUTPUT:

    from urllib.parse import urlparse

    # Get home path from environment variable
    home_path = os.environ.get('SHINKAI_HOME')
    output_path = os.path.join(home_path, 'output.png')

    print(f"Home path: {home_path}")
    print(f"Output path: {output_path}")
    print(f"Input path: {p.path}")

    # If input is URL, download it first
    if urlparse(p.path).scheme in ('http', 'https'):
        print(f"Downloading from URL: {p.path}")
        response = requests.get(p.path)
        response.raise_for_status()
        temp_path = output_path.replace('.png', '.tmp.png')
        with open(temp_path, 'wb') as f:
            f.write(response.content)
        p.path = temp_path
        print(f"Downloaded to: {p.path}")

    print(f"Processing image: {p.path}")
    with open(p.path, 'rb') as i:
        with open(output_path, 'wb') as o:
            input = i.read()
            print("Removing background...")
            output = remove(input)
            o.write(output)
            print(f"Saved result to: {output_path}")
            
            return None
    "#
            .to_string(),
        )]),
        entrypoint: "main.py".to_string(),
    };

    let context_id = nanoid::nanoid!();
    let context = ExecutionContext {
        storage: match (cfg!(windows), runner_type.clone()) {
            (true, RunnerType::Host) => {
                PathBuf::from("C:/hanzo-tools-runner-execution-storage/storage")
            }
            _ => PathBuf::from("./hanzo-tools-runner-execution-storage/storage"),
        },
        context_id: context_id.clone(),
        ..Default::default()
    };

    let python_runner = PythonRunner::new(
        code_files,
        Value::Null,
        Some(PythonRunnerOptions {
            context: context.clone(),
            force_runner_type: Some(runner_type),
            ..Default::default()
        }),
    );

    let result = python_runner
        .run(
            None,
            serde_json::json!({
                "path": "https://images.unsplash.com/photo-1514151458560-b9d0291a8676?fm=jpg&q=60&w=3000&ixlib=rb-4.0.3&ixid=M3wxMjA3fDB8MHxzZWFyY2h8MTZ8fGRvZyUyMGZvcmVzdHxlbnwwfHwwfHx8MA%3D%3D"
            }),
            None,
        )
        .await
        .map_err(|e| {
            log::error!("Failed to run python code: {}", e);
            e
        });
    assert!(result.is_ok());

    let output_path = context.storage.join(context_id).join("home/output.png");
    assert!(output_path.exists());
}

#[rstest]
#[case::host(RunnerType::Host)]
#[tokio::test]
async fn check_with_wrong_class_instance(#[case] runner_type: RunnerType) {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let code_files = CodeFiles {
        files: HashMap::from([(
            "main.py".to_string(),
            String::from(
                r#"
class OUTPUT:
    success: bool
    file_path: str

output = OUTPUT(success=False, file_path="None")
                "#,
            ),
        )]),
        entrypoint: "main.py".to_string(),
    };

    let python_runner = PythonRunner::new(
        code_files,
        Value::Null,
        Some(PythonRunnerOptions {
            force_runner_type: Some(runner_type),
            ..Default::default()
        }),
    );

    let check_result = python_runner.check().await.unwrap();
    assert!(!check_result.is_empty());
    assert!(check_result
        .iter()
        .any(|err| err.contains("No parameter named \"success\"")));
}

#[tokio::test]
async fn check_code_with_third_party_library() {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let code_files = CodeFiles {
        files: HashMap::from([(
            "main.py".to_string(),
            String::from(
                r#"
# /// script
# requires-python = ">=3.10,<3.12"
# dependencies = [
#   "requests",
#   "faster-whisper",
# ]
# ///
import os
from faster_whisper import WhisperModel

class CONFIG:
    # configure model-size / device via these fields if you like
    model_name: str = "base"      # "tiny" | "small" | "medium" | "large-v2" ...
    device: str = "cpu"           # "cuda" or "cpu"
    compute_type: str = "float32" # "int8"  | "float16" | "float32"
    # int8 = fastest CPU; float16 = fastest GPU

class INPUTS:
    audio_file_path: str

class OUTPUT:
    transcript: str

async def run(config: CONFIG, inputs: INPUTS) -> OUTPUT:
    if not os.path.exists(inputs.audio_file_path):
        raise FileNotFoundError(f"Audio file not found: {inputs.audio_file_path}")

    # initialise faster-whisper
    model = WhisperModel(
        config.model_name,
        device=config.device,
        compute_type=config.compute_type,
    )

    # transcribe and concatenate segment texts
    segments, _ = model.transcribe(inputs.audio_file_path)
    transcription: str = "".join(seg.text for seg in segments).strip()

    out = OUTPUT()
    out.transcript = transcription
    return out
                "#,
            ),
        )]),
        entrypoint: "main.py".to_string(),
    };

    let python_runner = PythonRunner::new(
        code_files,
        Value::Null,
        Some(PythonRunnerOptions {
            ..Default::default()
        }),
    );

    let check_result = python_runner.check().await.unwrap();
    assert!(check_result.is_empty());
}

#[rstest]
#[case::host(RunnerType::Host)]
#[case::docker(RunnerType::Docker)]
#[tokio::test]
async fn print_with_python_3_8(#[case] runner_type: RunnerType) {
    use std::path::PathBuf;

    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let code_files = CodeFiles {
        files: HashMap::from([(
            "main.py".to_string(),
            r#"
# /// script
# requires-python = ">=3.8,<=3.9"
# dependencies = [
#   "requests",
# ]
# ///
import requests
import sys
import os

class CONFIG:
    pass

class INPUTS:
    pass

class OUTPUT:
    pass

async def run(c: CONFIG, p: INPUTS) -> OUTPUT:
    print(f"Python version: {sys.version}")
   
    "#
            .to_string(),
        )]),
        entrypoint: "main.py".to_string(),
    };

    let context_id = nanoid::nanoid!();
    let context = ExecutionContext {
        storage: match (cfg!(windows), runner_type.clone()) {
            (true, RunnerType::Host) => {
                PathBuf::from("C:/hanzo-tools-runner-execution-storage/storage")
            }
            _ => PathBuf::from("./hanzo-tools-runner-execution-storage/storage"),
        },
        context_id: context_id.clone(),
        ..Default::default()
    };

    let python_runner = PythonRunner::new(
        code_files,
        Value::Null,
        Some(PythonRunnerOptions {
            context: context.clone(),
            force_runner_type: Some(runner_type),
            ..Default::default()
        }),
    );

    let result = python_runner
        .run(None, serde_json::Value::Null, None)
        .await
        .map_err(|e| {
            log::error!("Failed to run python code: {}", e);
            e
        });
    assert!(result.is_ok());
}

#[rstest]
#[case::host(RunnerType::Host)]
#[tokio::test]
async fn override_tool_uv_fields(#[case] runner_type: RunnerType) {
    use std::path::PathBuf;

    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let code_files = CodeFiles {
        files: HashMap::from([(
            "main.py".to_string(),
            r#"
# /// script
# requires-python = "==3.12.*"
# dependencies = [
#   "nari-tts @ git+https://github.com/nari-labs/dia",
# ]
# [[tool.uv.index]]
# name = "pytorch-cpu"
# url = "https://download.pytorch.org/whl/cpu"
# explicit = true
# [tool.uv.sources]
# torch = [
#   { index = "pytorch-cpu" },
# ]
# torchvision = [
#   { index = "pytorch-cpu" },
# ]
# [tool.setuptools.packages.find]
# where = ["."]
# include = ["dia"]
# namespaces = false
# ///
import requests
import sys
import os

class CONFIG:
    pass

class INPUTS:
    pass

class OUTPUT:
    pass

async def run(c: CONFIG, p: INPUTS) -> OUTPUT:
    print(f"Python version: {sys.version}")
   
    "#
            .to_string(),
        )]),
        entrypoint: "main.py".to_string(),
    };

    let context_id = nanoid::nanoid!();
    let execution_id = nanoid::nanoid!();
    let code_id = nanoid::nanoid!();
    let context = ExecutionContext {
        storage: match (cfg!(windows), runner_type.clone()) {
            (true, RunnerType::Host) => {
                PathBuf::from("C:/hanzo-tools-runner-execution-storage/storage")
            }
            _ => PathBuf::from("./hanzo-tools-runner-execution-storage/storage"),
        },
        context_id: context_id.clone(),
        execution_id: execution_id.clone(),
        code_id: code_id.clone(),
        ..Default::default()
    };

    let python_runner = PythonRunner::new(
        code_files,
        Value::Null,
        Some(PythonRunnerOptions {
            context: context.clone(),
            force_runner_type: Some(runner_type),
            ..Default::default()
        }),
    );

    let result = python_runner
        .run(None, serde_json::Value::Null, None)
        .await
        .map_err(|e| {
            log::error!("Failed to run python code: {}", e);
            e
        });
    assert!(result.is_ok());

    let pyproject_toml_path = context
        .storage
        .join(context_id)
        .join("code")
        .join(code_id)
        .join("pyproject.toml");
    println!("pyproject_toml_path: {:?}", pyproject_toml_path);
    assert!(pyproject_toml_path.exists());
    let pyproject_toml_content = std::fs::read_to_string(pyproject_toml_path).unwrap();
    assert!(pyproject_toml_content.contains("[tool.setuptools.packages.find]"));
    assert!(pyproject_toml_content.contains("where = [\".\""));
    assert!(pyproject_toml_content.contains("include = [\"dia\"]"));
    assert!(pyproject_toml_content.contains("namespaces = false"));
}

#[rstest]
#[case::host(RunnerType::Host)]
#[tokio::test]
async fn run_with_wrong_binary_error_message(#[case] runner_type: RunnerType) {
    use std::path::PathBuf;

    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let code_files = CodeFiles {
        files: HashMap::from([(
            "main.py".to_string(),
            r#"
import sys
import os

class CONFIG:
    pass

class INPUTS:
    pass

class OUTPUT:
    pass

async def run(c: CONFIG, p: INPUTS) -> OUTPUT:
    return {"message": "hello world"}
            "#
            .to_string(),
        )]),
        entrypoint: "main.py".to_string(),
    };

    let python_runner = PythonRunner::new(
        code_files,
        Value::Null,
        Some(PythonRunnerOptions {
            force_runner_type: Some(runner_type),
            uv_binary_path: PathBuf::from("/uvpotato"),
            ..Default::default()
        }),
    );

    let result = python_runner.run(None, serde_json::Value::Null, None).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().message().contains("uvpotato"));
}

#[rstest]
#[case::host(RunnerType::Host)]
#[case::host(RunnerType::Docker)]
#[tokio::test]
async fn run_reading_external_file(#[case] runner_type: RunnerType) {
    use std::io::Write;

    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();

    let code_files = CodeFiles {
        files: HashMap::from([(
            "main.py".to_string(),
            r#"
# /// script
# requires-python = ">=3.10,<3.12"
# dependencies = [
#   "requests"
# ]
# ///
import os

class CONFIG:
    configurations_file_path: str

class INPUTS:
    parameters_file_path: str

class OUTPUT:
    text_content_configurations: str
    text_content_parameters: str
async def run(config: CONFIG, inputs: INPUTS) -> OUTPUT:
    if not os.path.exists(config.configurations_file_path):
        raise FileNotFoundError(f"file not found: {config.configurations_file_path}")

    if not os.path.exists(inputs.parameters_file_path):
        raise FileNotFoundError(f"file not found: {inputs.parameters_file_path}")

    with open(config.configurations_file_path, "r") as file:
        text_content_configurations = file.read()

    with open(inputs.parameters_file_path, "r") as file:
        text_content_parameters = file.read()

    out = OUTPUT()
    out.text_content_configurations = text_content_configurations
    out.text_content_parameters = text_content_parameters
    return out
            "#
            .to_string(),
        )]),
        entrypoint: "main.py".to_string(),
    };

    let temp_dir = tempfile::tempdir().unwrap();

    let temp_file_path_configurations = temp_dir.path().join("test_file_configurations.txt");
    let mut temp_file_configurations =
        std::fs::File::create(&temp_file_path_configurations).unwrap();
    write!(temp_file_configurations, "Hello, world configurations!").unwrap();
    temp_file_configurations.flush().unwrap();

    let temp_file_path_parameters = temp_dir.path().join("test_file_parameters.txt");
    let mut temp_file_parameters = std::fs::File::create(&temp_file_path_parameters).unwrap();
    write!(temp_file_parameters, "Hello, world parameters!").unwrap();
    temp_file_parameters.flush().unwrap();

    let python_runner = PythonRunner::new(
        code_files,
        json!({
            "configurations_file_path": temp_file_path_configurations.to_string_lossy().to_string(),
        }),
        Some(PythonRunnerOptions {
            force_runner_type: Some(runner_type),
            context: ExecutionContext {
                mount_files: vec![
                    temp_file_path_configurations.to_path_buf(),
                    temp_file_path_parameters.to_path_buf(),
                ],
                ..Default::default()
            },
            ..Default::default()
        }),
    );

    let result = python_runner
        .run(
            None,
            json!(
                {
                    "parameters_file_path": temp_file_path_parameters.to_string_lossy().to_string()
                }
            ),
            None,
        )
        .await;
    assert!(result.is_ok());
    let result_data = result.unwrap().data;
    assert!(result_data["text_content_configurations"]
        .as_str()
        .unwrap()
        .contains("Hello, world configurations!"));
    assert!(result_data["text_content_parameters"]
        .as_str()
        .unwrap()
        .contains("Hello, world parameters!"));
}
