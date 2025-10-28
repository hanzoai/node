use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::{self, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::Mutex,
};
use toml_edit::DocumentMut;

use crate::tools::{
    check_utils::normalize_error_message,
    execution_error::ExecutionError,
    file_name_utils::{adapt_paths_in_value, normalize_for_docker_path},
    path_buf_ext::PathBufExt,
    run_result::RunResult,
    runner_type::resolve_runner_type,
};

use super::{
    code_files::CodeFiles, execution_storage::ExecutionStorage,
    python_runner_options::PythonRunnerOptions, runner_type::RunnerType,
};

pub struct PythonRunner {
    code: CodeFiles,
    configurations: Value,
    options: PythonRunnerOptions,
}

impl PythonRunner {
    pub const MAX_EXECUTION_TIME_MS_INTERNAL_OPS: u64 = 1000;
    pub const PYPROJECT_TOML_FILE_NAME: &'static str = "pyproject.toml";

    pub fn new(
        code_files: CodeFiles,
        configurations: Value,
        options: Option<PythonRunnerOptions>,
    ) -> Self {
        let options = options.unwrap_or_default();
        PythonRunner {
            code: code_files,
            configurations,
            options,
        }
    }

    async fn ensure_ruff(&self) -> anyhow::Result<()> {
        let uv_binary_path = path::absolute(self.options.uv_binary_path.clone())
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        let mut install_ruff_command = tokio::process::Command::new(uv_binary_path);
        let install_ruff_command = install_ruff_command
            .args(["tool", "install", "ruff"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        log::info!("Installing ruff...");
        let install_output = install_ruff_command
            .spawn()?
            .wait_with_output()
            .await
            .map_err(|e| anyhow::anyhow!("failed to install ruff: {}", e))?;

        if !install_output.status.success() {
            let stderr = String::from_utf8_lossy(&install_output.stderr);
            log::error!("failed to install ruff: {}", stderr);
            return Err(anyhow::anyhow!("failed to install ruff: {}", stderr));
        }

        log::info!("ruff installed successfully.");
        Ok(())
    }

    async fn ensure_pyright(&self, venv_path: PathBuf) -> anyhow::Result<()> {
        let uv_binary_path = path::absolute(self.options.uv_binary_path.clone())
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        let mut install_ruff_command = tokio::process::Command::new(uv_binary_path);
        let install_ruff_command = install_ruff_command
            .args(["pip", "install", "pyright"])
            .env(
                "VIRTUAL_ENV",
                venv_path.to_string_lossy().to_string().as_str(),
            )
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        log::info!("Installing pyright...");
        let install_output = install_ruff_command
            .spawn()?
            .wait_with_output()
            .await
            .map_err(|e| anyhow::anyhow!("failed to install pyright: {}", e))?;

        if !install_output.status.success() {
            let stderr = String::from_utf8_lossy(&install_output.stderr);
            log::error!("failed to install pyright: {}", stderr);
            return Err(anyhow::anyhow!("failed to install pyright: {}", stderr));
        }

        log::info!("pyright installed successfully.");
        Ok(())
    }

    pub fn extend_with_pyproject_toml(code_files: CodeFiles) -> anyhow::Result<CodeFiles> {
        let mut code_files = code_files.clone();
        let code_entrypoint = match code_files.files.get(&code_files.entrypoint.clone()) {
            Some(content) => content,
            None => return Err(anyhow::anyhow!("Code entrypoint file is empty")),
        };

        let pyproject_toml_string = r#"
[project]
name = "hanzo-tool"
version = "0.0.1"
dependencies = [
    "jsonpickle~=4.0.0",
]
requires-python = ">=3.10"
        "#;
        let mut pyproject_toml = pyproject_toml_string
            .parse::<DocumentMut>()
            .map_err(anyhow::Error::new)?;

        // Extract pyproject.toml script section between #///script and #///
        let mut script_lines = Vec::new();
        let mut in_script = false;
        let mut line_start = None;
        let mut line_end = None;
        for (line_number, code_line) in code_entrypoint.lines().enumerate() {
            if code_line.trim() == "# /// script" {
                line_start = Some(line_number);
                in_script = true;
                continue;
            } else if code_line.trim() == "# ///" {
                line_end = Some(line_number);
                break;
            }
            if in_script {
                let line = code_line
                    .trim()
                    .to_string()
                    .replace("#", "")
                    .trim()
                    .to_string();
                script_lines.push(line);
            }
        }

        // Remove lines between line_start and line_end
        if let (Some(line_start), Some(line_end)) = (line_start, line_end) {
            let mut lines: Vec<&str> = code_entrypoint.lines().collect();
            lines.drain(line_start..=line_end);
            let updated_code_entrypoint = lines.join("\n");
            log::info!("Updated code entrypoint: {}", updated_code_entrypoint);
            code_files
                .files
                .insert(code_files.entrypoint.clone(), updated_code_entrypoint);
        }

        let pyproject_toml_from_code_endpoint = script_lines
            .join("\n")
            .parse::<DocumentMut>()
            .map_err(anyhow::Error::new)?;

        // Merge everything from pyproject_toml_from_code_endpoint into pyproject_toml
        // except for dependencies which we'll handle separately
        let from_code_table = pyproject_toml_from_code_endpoint.as_table();
        // Iterate over all top level keys in the from_code_table
        for (key, value) in from_code_table.iter() {
            // Skip dependencies as we'll handle them separately
            if key == "dependencies" {
                continue;
            }

            // Handle requires-python specially as it goes inside the project table
            if key == "requires-python" {
                if let Some(project) = pyproject_toml.get_mut("project") {
                    if let Some(project_table) = project.as_table_mut() {
                        project_table.insert(key, value.clone());
                        log::info!("overriding python version: {}", value);
                    }
                }
            } else {
                let target_table = pyproject_toml.as_table_mut();
                // Check if the key already exists in pyproject_toml
                if target_table.contains_key(key)
                    && value.is_table()
                    && target_table[key].is_table()
                {
                    // Deep merge tables
                    Self::deep_merge_tables(
                        target_table.get_mut(key).unwrap().as_table_mut().unwrap(),
                        value.as_table().unwrap(),
                    );
                    log::info!("deep merging table: {}", key);
                } else {
                    // For all other keys, copy them directly to the root
                    target_table.insert(key, value.clone());
                    log::info!("adding key from script: {}", key);
                }
            }
        }

        // If dependencies exist in code endpoint toml, merge them into main toml
        if let Some(deps) = pyproject_toml_from_code_endpoint.get("dependencies") {
            if let Some(project) = pyproject_toml.get_mut("project") {
                if let Some(existing_deps) = project.get_mut("dependencies") {
                    // Merge the dependencies arrays
                    if let (Some(existing_arr), Some(new_arr)) =
                        (existing_deps.as_array_mut(), deps.as_array())
                    {
                        existing_arr.extend(new_arr.clone());
                        log::info!("merged dependencies from script");
                    }
                } else if let Some(project_table) = project.as_table_mut() {
                    // No existing dependencies, add them to the project table
                    project_table.insert("dependencies", deps.clone());
                    log::info!("added dependencies from script");
                }
            }
        }

        log::info!(
            "autogenerated pyproject_toml: {}",
            pyproject_toml.to_string()
        );
        code_files.files.insert(
            Self::PYPROJECT_TOML_FILE_NAME.to_string(),
            pyproject_toml.to_string(),
        );
        Ok(code_files)
    }

    pub async fn check(&self) -> anyhow::Result<Vec<String>> {
        let code = Self::extend_with_pyproject_toml(self.code.clone())
            .map_err(|e| anyhow::anyhow!("failed to create pyproject.toml: {}", e))?;
        let execution_storage = ExecutionStorage::new(code.clone(), self.options.context.clone());
        execution_storage.init_for_python(None)?;

        let uv_binary_path = path::absolute(self.options.uv_binary_path.clone())
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        let mut create_check_venv_command = tokio::process::Command::new(uv_binary_path);
        let command = create_check_venv_command
            .env_clear()
            .args([
                "venv",
                execution_storage
                    .python_check_venv_folder_path()
                    .to_string_lossy()
                    .to_string()
                    .as_str(),
            ])
            .kill_on_drop(true);
        match command.spawn() {
            Ok(child) => child.wait_with_output().await?,
            Err(e) => {
                let error_msg = format!("failed to spawn command: {:?} error: {}", command, e);
                log::error!("{}", error_msg);
                return Err(anyhow::anyhow!("{}", error_msg));
            }
        };

        self.ensure_ruff().await?;
        self.ensure_pyright(execution_storage.python_check_venv_folder_path())
            .await?;

        log::info!("Starting code check with ruff...");
        let mut command = tokio::process::Command::new("ruff");
        command
            .args(["check"])
            .current_dir(execution_storage.code_folder_path.clone())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let output = match command.spawn() {
            Ok(child) => child.wait_with_output().await?,
            Err(e) => {
                let error_msg = format!("failed to spawn command: {:?} error: {}", command, e);
                log::error!("{}", error_msg);
                return Err(anyhow::anyhow!("{}", error_msg));
            }
        };

        let mut lint_message = String::from_utf8(output.stdout)?;
        lint_message = normalize_error_message(lint_message, &execution_storage.code_folder_path);
        let lint_message_lines: Vec<String> = lint_message.lines().map(|s| s.to_string()).collect();

        for line in &lint_message_lines {
            log::info!("python ruff lint message: {}", line);
        }

        // When success, ruff returns 1 line with "All check passed!"
        if lint_message_lines.len() > 1 {
            return Ok(lint_message_lines);
        }

        log::info!("starting pyright check");
        let mut command = tokio::process::Command::new("uv");
        command
            .args([
                "run",
                "python",
                "-m",
                "pyright",
                "--level=error",
                execution_storage
                    .code_entrypoint_file_path
                    .to_string_lossy()
                    .to_string()
                    .as_str(),
            ])
            .env(
                "VIRTUAL_ENV",
                execution_storage
                    .python_check_venv_folder_path()
                    .to_string_lossy()
                    .to_string()
                    .as_str(),
            )
            .env(
                "UV_PROJECT_ENVIRONMENT",
                execution_storage
                    .python_check_venv_folder_path()
                    .to_string_lossy()
                    .to_string()
                    .as_str(),
            )
            .current_dir(execution_storage.code_folder_path.clone())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let output = match command.spawn() {
            Ok(child) => match child.wait_with_output().await {
                Ok(output) => output,
                Err(e) => {
                    log::error!("failed to get command output: {}", e);
                    return Err(anyhow::anyhow!(
                        "failed to get pyright command output: {}",
                        e
                    ));
                }
            },
            Err(e) => {
                let error_msg = format!("failed to spawn command: {:?} error: {}", command, e);
                log::error!("{}", error_msg);
                return Err(anyhow::anyhow!("{}", error_msg));
            }
        };
        log::info!("pyright check finished");
        if !output.status.success() {
            println!(
                "pyright check failed error: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let lint_message = String::from_utf8(output.stdout)?;
            let lint_message_lines: Vec<String> =
                lint_message.lines().map(|s| s.to_string()).collect();
            log::info!(
                "python pyright check finished with errors:\n\n{}",
                lint_message_lines.join("\n")
            );
            return Ok(lint_message_lines);
        }

        log::info!("pyright check finished: passed");
        Ok(vec![])
    }

    pub async fn run(
        &self,
        envs: Option<HashMap<String, String>>,
        parameters: Value,
        max_execution_timeout: Option<Duration>,
    ) -> Result<RunResult, ExecutionError> {
        log::info!("preparing to run tool");
        log::info!("configurations: {}", self.configurations.to_string());
        log::info!("parameters: {}", parameters.to_string());

        let entrypoint_code = self.code.files.get(&self.code.entrypoint.clone());
        if entrypoint_code.is_none() {
            return Err(ExecutionError::new(
                format!("no entrypoint found {}", self.code.entrypoint),
                None,
            ));
        }
        let resolved_runner_type = resolve_runner_type(self.options.force_runner_type.clone());
        let mut code = Self::extend_with_pyproject_toml(self.code.clone()).map_err(|e| {
            ExecutionError::new(format!("failed to create pyproject.toml: {}", e), None)
        })?;

        let entrypoint_code = code.files.get(&self.code.entrypoint.clone()).unwrap();

        log::info!(
            "Extended pyproject.toml {:?}",
            code.files.get(Self::PYPROJECT_TOML_FILE_NAME).unwrap()
        );
        let mut adapted_configurations = self.configurations.clone();
        if let Some(object) = adapted_configurations.as_object_mut() {
            object.insert(
                "py/object".to_string(),
                Value::String("__main__.CONFIG".to_string()),
            );
        }
        // Deep traverse adapted_parameters and normalize mount file paths
        if !self.options.context.mount_files.is_empty()
            && matches!(resolved_runner_type, RunnerType::Docker)
        {
            let mount_files = self
                .options
                .context
                .mount_files
                .iter()
                .map(|p| path::absolute(p).unwrap().to_string_lossy().to_string())
                .collect::<HashSet<String>>();

            adapted_configurations = adapt_paths_in_value(&adapted_configurations, &mount_files);
        }

        let mut adapted_parameters = parameters.clone();

        // Deep traverse adapted_parameters and normalize mount file paths
        if !self.options.context.mount_files.is_empty()
            && matches!(resolved_runner_type, RunnerType::Docker)
        {
            let mount_files = self
                .options
                .context
                .mount_files
                .iter()
                .map(|p| path::absolute(p).unwrap().to_string_lossy().to_string())
                .collect::<HashSet<String>>();

            adapted_parameters = adapt_paths_in_value(&adapted_parameters, &mount_files);
        }

        if let Some(object) = adapted_parameters.as_object_mut() {
            object.insert(
                "py/object".to_string(),
                Value::String("__main__.INPUTS".to_string()),
            );
        }

        let adapted_entrypoint_code = format!(
            r#"
{}
import asyncio
import jsonpickle
import json

class TrickyJsonEncoder(json.JSONEncoder):
    def default(self, obj):
        if isinstance(obj, (list, tuple)):
            return [self.default(item) for item in obj]
        elif isinstance(obj, dict):
            return {{key: self.default(value) for key, value in obj.items()}}
        elif isinstance(obj, set):
            return list(obj)
        elif isinstance(obj, bytes):
            return obj.decode('utf-8')  # Convert bytes to string
        elif isinstance(obj, object) and hasattr(obj, '__dict__'):
            return {{key: self.default(value) for key, value in obj.__dict__.items() if not key.startswith('__')}}
        elif isinstance(obj, str):
            return obj  # Return string as is
        elif obj is None:
            return None
        elif hasattr(obj,'__iter__'):
            return list(obj)  # Convert list_iterator to a list
        else:
            print("warning: trying to serialize an unknown type", type(obj), obj)
            return str(obj)  # Fallback for unknown types

def tricky_json_dump(obj):
    jsonpickle_encoded = jsonpickle.encode(obj, unpicklable=False, make_refs=False, indent=4)
    jsonpickle_decoded = jsonpickle.decode(jsonpickle_encoded, reset=True)
    custom_json_dump = json.dumps(jsonpickle_decoded, indent=4, cls=TrickyJsonEncoder)
    return custom_json_dump

configurations = jsonpickle.decode('{}')
parameters = jsonpickle.decode('{}')

result = run(configurations, parameters)
if asyncio.iscoroutine(result):
    result = asyncio.run(result)

serialized_result = tricky_json_dump(result)

print("<hanzo-code-result>")
print(serialized_result)
print("</hanzo-code-result>")
        "#,
            &entrypoint_code,
            serde_json::to_string(&adapted_configurations)
                .unwrap()
                .replace("\\", "\\\\")
                .replace("'", "\\'")
                .replace("\"", "\\\""),
            serde_json::to_string(&adapted_parameters)
                .unwrap()
                .replace("\\", "\\\\")
                .replace("'", "\\'")
                .replace("\"", "\\\"")
        );
        code.files
            .insert(self.code.entrypoint.clone(), adapted_entrypoint_code);

        let result = match resolved_runner_type {
            RunnerType::Host => self.run_in_host(code, envs, max_execution_timeout).await,
            RunnerType::Docker => self.run_in_docker(code, envs, max_execution_timeout).await,
        }
        .map_err(|e| ExecutionError::new(e.to_string(), None))?;

        let result_text = result
            .iter()
            .skip_while(|line| !line.contains("<hanzo-code-result>"))
            .skip(1)
            .take_while(|line| !line.contains("</hanzo-code-result>"))
            .map(|s| s.to_string())
            .collect::<Vec<String>>()
            .join("\n");

        log::info!("result : {:?}", result);
        log::info!("result text: {:?}", result_text);

        let result: Value = serde_json::from_str(&result_text).map_err(|e| {
            log::info!("failed to parse result: {}", e);
            ExecutionError::new(format!("failed to parse result: {}", e), None)
        })?;
        log::info!("successfully parsed run result: {:?}", result);
        Ok(RunResult { data: result })
    }

    async fn run_in_docker(
        &self,
        code_files: CodeFiles,
        envs: Option<HashMap<String, String>>,
        max_execution_timeout: Option<Duration>,
    ) -> anyhow::Result<Vec<String>> {
        log::info!(
            "using python from container image:{:?}",
            self.options.code_runner_docker_image_name
        );

        log::info!("code files: {:?}", code_files.files.get("main.py"));
        let execution_storage = ExecutionStorage::new(code_files, self.options.context.clone());
        execution_storage.init_for_python(None)?;

        let mut mount_params = Vec::<String>::new();

        let mount_dirs = [
            (
                execution_storage.code_folder_path.as_normalized_string(),
                execution_storage.relative_to_root(execution_storage.code_folder_path.clone()),
            ),
            (
                execution_storage.home_folder_path.as_normalized_string(),
                execution_storage.relative_to_root(execution_storage.home_folder_path.clone()),
            ),
            (
                execution_storage
                    .python_run_docker_venv_folder_path()
                    .as_normalized_string(),
                execution_storage
                    .relative_to_root(execution_storage.python_run_docker_venv_folder_path()),
            ),
            (
                execution_storage
                    .python_run_docker_uv_cache_folder_path()
                    .as_normalized_string(),
                execution_storage.relative_to_global_cache(
                    execution_storage.python_run_docker_uv_cache_folder_path(),
                ),
            ),
        ];
        for (dir, relative_path) in mount_dirs {
            let mount_param = format!(r#"type=bind,source={},target=/app/{}"#, dir, relative_path);
            log::info!("mount parameter created: {}", mount_param);
            mount_params.extend([String::from("--mount"), mount_param]);
        }

        let mut mount_env = String::from("");
        log::info!("mount files: {:?}", self.options.context.mount_files);
        // Mount each writable file to /app/mount
        for file in &self.options.context.mount_files {
            // Copy the files to the exact same path in the volume.
            // This will allow to run the same code in the host and in the container.
            let path = normalize_for_docker_path(file.to_path_buf());
            let mount_param = format!(r#"type=bind,source={},target={}"#, path, path);
            log::info!("mount parameter created: {}", mount_param);
            mount_env += &format!("{},", path);
            mount_params.extend([String::from("--mount"), mount_param]);
        }

        let mut mount_assets_env = String::from("");
        // Mount each asset file to /app/assets
        for file in &self.options.context.assets_files {
            let target_path = format!(
                "/app/{}/{}",
                execution_storage.relative_to_root(execution_storage.assets_folder_path.clone()),
                file.file_name().unwrap().to_str().unwrap()
            );
            let mount_param = format!(
                r#"type=bind,readonly=true,source={},target={}"#,
                path::absolute(file).unwrap().as_normalized_string(),
                target_path,
            );
            log::debug!("mount parameter created: {}", mount_param);
            mount_assets_env += &format!("{},", target_path);
            mount_params.extend([String::from("--mount"), mount_param]);
        }

        let mut container_envs = Vec::<String>::new();

        container_envs.push(String::from("-e"));
        container_envs.push(format!(
            "SHINKAI_NODE_LOCATION={}://host.docker.internal:{}",
            self.options.hanzo_node_location.protocol, self.options.hanzo_node_location.port
        ));

        container_envs.push(String::from("-e"));
        container_envs.push(String::from("SHINKAI_HOME=/app/home"));
        container_envs.push(String::from("-e"));
        container_envs.push(format!("SHINKAI_ASSETS={}", mount_assets_env));
        container_envs.push(String::from("-e"));
        container_envs.push(format!("SHINKAI_MOUNT={}", mount_env));
        container_envs.push(String::from("-e"));
        container_envs.push(format!(
            "SHINKAI_CONTEXT_ID={}",
            self.options.context.context_id
        ));
        container_envs.push(String::from("-e"));
        container_envs.push(format!(
            "SHINKAI_EXECUTION_ID={}",
            self.options.context.execution_id
        ));
        container_envs.push(String::from("-e"));
        container_envs.push(format!(
            "VIRTUAL_ENV=/app/{}",
            execution_storage
                .relative_to_root(execution_storage.python_run_docker_venv_folder_path())
        ));
        container_envs.push(String::from("-e"));
        container_envs.push(format!(
            "UV_PROJECT_ENVIRONMENT=/app/{}",
            execution_storage
                .relative_to_root(execution_storage.python_run_docker_venv_folder_path())
        ));
        container_envs.push(String::from("-e"));
        container_envs.push(format!(
            "UV_CACHE_DIR=/app/{}",
            execution_storage.relative_to_global_cache(
                execution_storage.python_run_docker_uv_cache_folder_path()
            )
        ));

        if let Some(envs) = envs {
            for (key, value) in envs {
                let env = format!("{}={}", key, value);
                container_envs.push(String::from("-e"));
                container_envs.push(env);
            }
        }

        let code_entrypoint =
            execution_storage.relative_to_root(execution_storage.code_entrypoint_file_path.clone());

        let mut command = tokio::process::Command::new("docker");
        let mut args = vec!["run", "--rm"];
        args.extend(mount_params.iter().map(|s| s.as_str()));
        args.extend(container_envs.iter().map(|s| s.as_str()));

        let pyproject_toml_path = execution_storage
            .relative_to_root(
                execution_storage
                    .code_folder_path
                    .clone()
                    .join(Self::PYPROJECT_TOML_FILE_NAME),
            )
            .to_string();

        let python_start_script = format!(
            "uv run --project {} {}",
            pyproject_toml_path,
            code_entrypoint.clone().as_str(),
        );

        args.extend([
            "--workdir",
            "/app",
            self.options.code_runner_docker_image_name.as_str(),
            "/bin/bash",
            "-c",
            python_start_script.as_str(),
        ]);

        let command = command
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        log::info!("spawning docker command: {:?}", command);
        let mut child = command.spawn().map_err(|e| {
            let error_msg = format!("failed to spawn command: {:?} error: {}", command, e);
            log::error!("{}", error_msg);
            anyhow::anyhow!("{}", error_msg)
        })?;

        let stdout = child.stdout.take().expect("Failed to get stdout");
        let mut stdout_stream = BufReader::new(stdout).lines();

        let stderr = child.stderr.take().expect("Failed to get stderr");
        let mut stderr_stream = BufReader::new(stderr).lines();

        let stdout_lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let stderr_lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let execution_storage_clone = execution_storage.clone();

        let stdout_lines_clone = stdout_lines.clone();
        let stderr_lines_clone = stderr_lines.clone();
        let execution_storage_clone2 = execution_storage_clone.clone();

        let stdout_task = tokio::task::spawn_blocking(move || {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                while let Ok(Some(line)) = stdout_stream.next_line().await {
                    log::info!("from python: {}", line);
                    stdout_lines_clone.lock().await.push(line.clone());
                    let _ = execution_storage_clone.append_log(line.as_str());
                }
            });
        });

        let stderr_task = tokio::task::spawn_blocking(move || {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                while let Ok(Some(line)) = stderr_stream.next_line().await {
                    log::info!("from python: {}", line);
                    stderr_lines_clone.lock().await.push(line.clone());
                    let _ = execution_storage_clone2.append_log(line.as_str());
                }
            });
        });

        #[allow(clippy::let_underscore_future)]
        let std_tasks = tokio::spawn(async move {
            let _ = futures::future::join_all(vec![stdout_task, stderr_task]).await;
        });

        let output = if let Some(timeout) = max_execution_timeout {
            log::info!("executing command with {}[s] timeout", timeout.as_secs());
            match tokio::time::timeout(timeout, child.wait_with_output()).await {
                Ok(result) => result?,
                Err(_) => {
                    log::error!("command execution timed out after {}[s]", timeout.as_secs());
                    return Err(anyhow::anyhow!(
                        "process timed out after {}[s]",
                        timeout.as_secs()
                    ));
                }
            }
        } else {
            log::info!("executing command without timeout");
            child.wait_with_output().await?
        };
        let _ = std_tasks.await;
        if !output.status.success() {
            let stderr = stderr_lines.lock().await.to_vec().join("\n");
            log::error!("command execution failed: {}", stderr);
            return Err(anyhow::Error::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                stderr.to_string(),
            )));
        }

        let stdout: Vec<String> = stdout_lines.lock().await.to_vec();
        log::info!("command completed successfully with output: {:?}", stdout);
        Ok(stdout)
    }

    async fn run_in_host(
        &self,
        code_files: CodeFiles,
        envs: Option<HashMap<String, String>>,
        max_execution_timeout: Option<Duration>,
    ) -> anyhow::Result<Vec<String>> {
        let execution_storage = ExecutionStorage::new(code_files, self.options.context.clone());
        execution_storage.init_for_python(None)?;

        let uv_binary_path = path::absolute(self.options.uv_binary_path.clone())
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        log::info!("using uv from host at path: {:?}", uv_binary_path.clone());

        let mut command = tokio::process::Command::new(uv_binary_path);

        let command = command
            .arg("run")
            .args([
                "--project",
                execution_storage
                    .code_folder_path
                    .join(Self::PYPROJECT_TOML_FILE_NAME)
                    .to_str()
                    .unwrap(),
            ])
            .arg(execution_storage.code_entrypoint_file_path.clone())
            .current_dir(execution_storage.root_folder_path.clone())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        command.env(
            "VIRTUAL_ENV",
            execution_storage
                .python_run_host_venv_folder_path()
                .to_string_lossy()
                .to_string()
                .as_str(),
        );
        command.env(
            "UV_PROJECT_ENVIRONMENT",
            execution_storage
                .python_run_host_venv_folder_path()
                .to_string_lossy()
                .to_string()
                .as_str(),
        );

        command.env(
            "SHINKAI_NODE_LOCATION",
            format!(
                "{}://{}:{}",
                self.options.hanzo_node_location.protocol,
                self.options.hanzo_node_location.host,
                self.options.hanzo_node_location.port
            ),
        );

        command.env("SHINKAI_HOME", execution_storage.home_folder_path.clone());
        command.env(
            "SHINKAI_ASSETS",
            self.options
                .context
                .assets_files
                .iter()
                .map(|p| path::absolute(p).unwrap().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join(","),
        );
        command.env(
            "SHINKAI_MOUNT",
            self.options
                .context
                .mount_files
                .iter()
                .map(|p| path::absolute(p).unwrap().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join(","),
        );
        command.env(
            "SHINKAI_CONTEXT_ID",
            self.options.context.context_id.clone(),
        );
        command.env(
            "SHINKAI_EXECUTION_ID",
            self.options.context.execution_id.clone(),
        );

        if let Some(envs) = envs {
            command.envs(envs);
        }
        log::info!("prepared command with arguments: {:?}", command);
        let mut child = command.spawn().map_err(|e| {
            let error_msg = format!("failed to spawn command: {:?} error: {}", command, e);
            log::error!("{}", error_msg);
            anyhow::anyhow!("{}", error_msg)
        })?;

        let stdout = child.stdout.take().expect("Failed to get stdout");
        let mut stdout_stream = BufReader::new(stdout).lines();

        let stderr = child.stderr.take().expect("Failed to get stderr");
        let mut stderr_stream = BufReader::new(stderr).lines();

        let stdout_lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let stderr_lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let execution_storage_clone = execution_storage.clone();

        let stdout_lines_clone = stdout_lines.clone();
        let stderr_lines_clone = stderr_lines.clone();
        let execution_storage_clone2 = execution_storage_clone.clone();

        let stdout_task = tokio::task::spawn_blocking(move || {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                while let Ok(Some(line)) = stdout_stream.next_line().await {
                    log::info!("from python: {}", line);
                    stdout_lines_clone.lock().await.push(line.clone());
                    let _ = execution_storage_clone.append_log(line.as_str());
                }
            });
        });

        let stderr_task = tokio::task::spawn_blocking(move || {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                while let Ok(Some(line)) = stderr_stream.next_line().await {
                    log::info!("from python: {}", line);
                    stderr_lines_clone.lock().await.push(line.clone());
                    let _ = execution_storage_clone2.append_log(line.as_str());
                }
            });
        });

        #[allow(clippy::let_underscore_future)]
        let std_tasks = tokio::spawn(async move {
            let _ = futures::future::join_all(vec![stdout_task, stderr_task]).await;
        });

        let output = if let Some(timeout) = max_execution_timeout {
            log::info!("executing command with {}[s] timeout", timeout.as_secs());
            match tokio::time::timeout(timeout, child.wait_with_output()).await {
                Ok(result) => result?,
                Err(_) => {
                    log::error!("command execution timed out after {}[s]", timeout.as_secs());
                    return Err(anyhow::Error::new(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!("process timed out after {}[s]", timeout.as_secs()),
                    )));
                }
            }
        } else {
            log::info!("executing command without timeout");
            child.wait_with_output().await?
        };
        let _ = std_tasks.await;
        if !output.status.success() {
            let stderr = stderr_lines.lock().await.to_vec().join("\n");
            log::error!("command execution failed: {}", stderr);
            return Err(anyhow::Error::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                stderr.to_string(),
            )));
        }
        let stdout: Vec<String> = stdout_lines.lock().await.to_vec();
        log::info!("command completed successfully with output: {:?}", stdout);
        Ok(stdout)
    }

    // Helper function for deep merging TOML tables
    fn deep_merge_tables(target: &mut toml_edit::Table, source: &toml_edit::Table) {
        for (key, value) in source.iter() {
            if !target.contains_key(key) {
                // Key doesn't exist in target, simply insert
                target.insert(key, value.clone());
                continue;
            }

            // Key exists in both, check if both are tables for deep merge
            if value.is_table() && target[key].is_table() {
                // Recursive merge for nested tables
                if let (Some(target_subtable), Some(source_subtable)) = (
                    target.get_mut(key).unwrap().as_table_mut(),
                    value.as_table(),
                ) {
                    Self::deep_merge_tables(target_subtable, source_subtable);
                }
            } else if value.is_array() && target[key].is_array() {
                // Merge arrays
                if let (Some(target_array), Some(source_array)) = (
                    target.get_mut(key).unwrap().as_array_mut(),
                    value.as_array(),
                ) {
                    target_array.extend(source_array.clone());
                }
            } else {
                // For other values, override the target value
                target.insert(key, value.clone());
            }
        }
    }
}

#[cfg(test)]
#[path = "python_runner.test.rs"]
mod tests;
