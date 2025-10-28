use regex::Regex;
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::Mutex,
};

use crate::tools::{
    check_utils::normalize_error_message,
    execution_storage::ExecutionStorage,
    file_name_utils::{adapt_paths_in_value, normalize_for_docker_path},
    path_buf_ext::PathBufExt,
    runner_type::{resolve_runner_type, RunnerType},
};

use super::{
    code_files::CodeFiles, deno_runner_options::DenoRunnerOptions, execution_error::ExecutionError,
    run_result::RunResult,
};
use std::{
    collections::{HashMap, HashSet},
    path::{self, PathBuf},
    sync::Arc,
    time::Duration,
};

#[derive(Default)]
pub struct DenoRunner {
    code: CodeFiles,
    configurations: Value,
    options: DenoRunnerOptions,
}

impl DenoRunner {
    pub const MAX_EXECUTION_TIME_MS_INTERNAL_OPS: u64 = 1000;

    pub fn new(
        code_files: CodeFiles,
        configurations: Value,
        options: Option<DenoRunnerOptions>,
    ) -> Self {
        let options = options.unwrap_or_default();
        DenoRunner {
            code: code_files,
            configurations,
            options,
        }
    }

    /// Checks the code for errors without running it
    ///
    /// # Returns
    ///
    /// Returns a Result containing:
    /// - Ok(Vec<String>): The list of errors found in the code
    /// - Err(anyhow::Error): Any errors that occurred during setup or execution
    pub async fn check(&self) -> anyhow::Result<Vec<String>> {
        let execution_storage =
            ExecutionStorage::new(self.code.clone(), self.options.context.clone());
        execution_storage.init_for_deno(None, RunnerType::Host)?;

        let binary_path = path::absolute(self.options.deno_binary_path.clone())
            .unwrap()
            .to_string_lossy()
            .to_string();
        let mut command = tokio::process::Command::new(binary_path);
        command
            .args([
                "check",
                execution_storage
                    .code_entrypoint_file_path
                    .to_str()
                    .unwrap(),
            ])
            .env_clear()
            .env("NO_COLOR", "true")
            .current_dir(execution_storage.code_folder_path.clone())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let output = command.spawn()?.wait_with_output().await?;
        match output.status.success() {
            true => Ok(Vec::new()),
            false => {
                let error_message = String::from_utf8(output.stderr)?;
                let mut error_message =
                    normalize_error_message(error_message, &execution_storage.code_folder_path);
                log::error!("deno check error: {}", error_message);

                // Replace node_modules warning with empty string (it was confusing the llm)
                let node_modules_regex =
                    Regex::new(r"(?:Warning.*?not executed:(?:\n┠─.*)+\n┃\n(?:┠─.*\n)*┖─.*)")
                        .unwrap();
                error_message = node_modules_regex
                    .replace_all(&error_message, "")
                    .to_string();

                let error_match_regex =
                    Regex::new(r"(?:TS\d+ \[ERROR\]:.*(?:\n.*){2,4}at .*:\d+:\d+)").unwrap();
                let matched_errors = error_match_regex
                    .find_iter(&error_message)
                    .map(|m| m.as_str())
                    .collect::<Vec<_>>();
                if matched_errors.is_empty() {
                    log::warn!(
                        "no errors found in deno check but the command failed, this could be a bug"
                    );
                    let error_lines: Vec<String> =
                        error_message.lines().map(|s| s.to_string()).collect();
                    return Ok(error_lines);
                }
                Ok(matched_errors.iter().map(|s| s.to_string()).collect())
            }
        }
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

        let resolved_runner_type = resolve_runner_type(self.options.force_runner_type.clone());

        let mut adapted_configurations = self.configurations.clone();
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

        let mut code = self.code.clone();
        let entrypoint_code = code.files.get(&self.code.entrypoint.clone());
        if let Some(entrypoint_code) = entrypoint_code {
            let adapted_entrypoint_code = format!(
                r#"
            {}
            const configurations = JSON.parse('{}');
            const parameters = JSON.parse('{}');

            const result = await run(configurations, parameters);
            const adaptedResult = result === undefined ? null : result;
            console.log("<hanzo-code-result>");
            console.log(JSON.stringify(adaptedResult));
            console.log("</hanzo-code-result>");
            Deno.exit(0);
        "#,
                &entrypoint_code,
                serde_json::to_string(&adapted_configurations)
                    .unwrap()
                    .replace("\\", "\\\\")
                    .replace("'", "\\'")
                    .replace("\"", "\\\"")
                    .replace("`", "\\`"),
                serde_json::to_string(&adapted_parameters)
                    .unwrap()
                    .replace("\\", "\\\\")
                    .replace("'", "\\'")
                    .replace("\"", "\\\"")
                    .replace("`", "\\`")
            );
            code.files
                .insert(self.code.entrypoint.clone(), adapted_entrypoint_code);
        }

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

        log::info!("result text: {:?}", result);

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
            "using deno from container image:{:?}",
            self.options.code_runner_docker_image_name
        );

        let execution_storage = ExecutionStorage::new(code_files, self.options.context.clone());
        execution_storage.init_for_deno(None, RunnerType::Docker)?;

        let mut mount_params = Vec::<String>::new();

        let mount_dirs = [
            (
                execution_storage.code_folder_path.as_normalized_string(),
                execution_storage.relative_to_root(execution_storage.code_folder_path.clone()),
            ),
            (
                execution_storage
                    .deno_cache_folder_path(RunnerType::Docker)
                    .as_normalized_string(),
                execution_storage.relative_to_global_cache(
                    execution_storage
                        .deno_cache_folder_path(RunnerType::Docker)
                        .clone(),
                ),
            ),
            (
                execution_storage.home_folder_path.as_normalized_string(),
                execution_storage.relative_to_root(execution_storage.home_folder_path.clone()),
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
        container_envs.push("NO_COLOR=true".to_string());

        container_envs.push(String::from("-e"));
        container_envs.push(format!(
            "DENO_DIR={}",
            execution_storage.relative_to_global_cache(
                execution_storage
                    .deno_cache_folder_path(RunnerType::Docker)
                    .clone()
            )
        ));

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

        if let Some(envs) = envs {
            for (key, value) in envs {
                let env = format!("{}={}", key, value);
                container_envs.push(String::from("-e"));
                container_envs.push(env);
            }
        }

        let deno_permissions = self.get_deno_permissions(
            RunnerType::Docker,
            "/usr/bin/deno",
            "/app/home",
            &self
                .options
                .context
                .mount_files
                .iter()
                .map(|p| path::absolute(p).unwrap())
                .collect::<Vec<_>>(),
            &self
                .options
                .context
                .assets_files
                .iter()
                .map(|p| {
                    let path_in_docker = format!(
                        "/app/{}/{}",
                        execution_storage
                            .relative_to_root(execution_storage.assets_folder_path.clone()),
                        p.file_name().unwrap().to_str().unwrap()
                    );
                    PathBuf::from(path_in_docker)
                })
                .collect::<Vec<_>>(),
        );

        let code_entrypoint =
            execution_storage.relative_to_root(execution_storage.code_entrypoint_file_path.clone());
        let mut command = tokio::process::Command::new("docker");
        let mut args = vec!["run", "--rm"];
        args.extend(mount_params.iter().map(|s| s.as_str()));
        args.extend(container_envs.iter().map(|s| s.as_str()));
        args.extend([
            "--workdir",
            "/app",
            self.options.code_runner_docker_image_name.as_str(),
            "deno",
            "run",
            "--ext",
            "ts",
        ]);
        args.extend(deno_permissions.iter().map(|s| s.as_str()));
        args.extend([code_entrypoint.as_str()]);
        let command = command
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        log::info!("spawning docker command");
        let mut child = command.spawn().map_err(|e| {
            let error_msg = format!("failed to spawn command: {:?}, error: {}", command, e);
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
                    log::info!("from deno: {}", line);
                    stdout_lines_clone.lock().await.push(line.clone());
                    let _ = execution_storage_clone.append_log(line.as_str());
                }
            });
        });

        let stderr_task = tokio::task::spawn_blocking(move || {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                while let Ok(Some(line)) = stderr_stream.next_line().await {
                    log::info!("from deno: {}", line);
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
        execution_storage.init_for_deno(None, RunnerType::Host)?;

        let binary_path = path::absolute(self.options.deno_binary_path.clone())
            .unwrap()
            .to_string_lossy()
            .to_string();
        log::info!("using deno from host at path: {:?}", binary_path.clone());

        let deno_permissions: Vec<String> = self.get_deno_permissions(
            RunnerType::Host,
            binary_path.clone().as_str(),
            execution_storage
                .home_folder_path
                .to_string_lossy()
                .to_string()
                .as_str(),
            &self
                .options
                .context
                .mount_files
                .iter()
                .map(|p| path::absolute(p).unwrap())
                .collect::<Vec<_>>(),
            &self
                .options
                .context
                .assets_files
                .iter()
                .map(|p| path::absolute(p).unwrap())
                .collect::<Vec<_>>(),
        );

        let mut command = tokio::process::Command::new(binary_path);
        let command = command
            .args(["run", "--ext", "ts"])
            .args(deno_permissions)
            .arg(execution_storage.code_entrypoint_file_path.clone())
            .current_dir(execution_storage.root_folder_path.clone())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        command.env("NO_COLOR", "true");
        command.env(
            "DENO_DIR",
            execution_storage
                .deno_cache_folder_path(RunnerType::Host)
                .clone(),
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
                    log::info!("from deno: {}", line);
                    stdout_lines_clone.lock().await.push(line.clone());
                    let _ = execution_storage_clone.append_log(line.as_str());
                }
            });
        });

        let stderr_task = tokio::task::spawn_blocking(move || {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                while let Ok(Some(line)) = stderr_stream.next_line().await {
                    log::info!("from deno: {}", line);
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

    fn get_deno_permissions(
        &self,
        runner_type: RunnerType,
        exec_path: &str,
        home_path: &str,
        mount_files: &[PathBuf],
        assets_files: &[PathBuf],
    ) -> Vec<String> {
        log::info!("mount files: {:?}", mount_files);
        log::info!("assets files: {:?}", assets_files);
        let mut deno_permissions: Vec<String> = vec![
            // Basically all non-file related permissions
            "--allow-env".to_string(),
            "--allow-run".to_string(),
            "--allow-net".to_string(),
            "--allow-sys".to_string(),
            "--allow-scripts".to_string(),
            "--allow-ffi".to_string(),
            "--allow-import".to_string(),

            // Engine folders
            "--allow-read=.".to_string(),
            format!("--allow-write={}", home_path.to_string()),

            // Playwright/Chrome folders
            format!("--allow-read={}", exec_path.to_string()),
            "--allow-write=/var/folders".to_string(),
            "--allow-read=/var/folders".to_string(),
            "--allow-read=/tmp".to_string(),
            "--allow-write=/tmp".to_string(),
            format!("--allow-read={}", std::env::temp_dir().to_string_lossy()),
            format!("--allow-write={}", std::env::temp_dir().to_string_lossy()),
            "--allow-read=/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".to_string(),
            "--allow-read=/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary".to_string(),
            "--allow-read=/Applications/Chromium.app/Contents/MacOS/Chromium".to_string(),
            "--allow-read=C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe".to_string(),
            "--allow-read=C:\\Program Files (x86)\\Google\\Chrome SxS\\Application\\chrome.exe".to_string(),
            "--allow-read=C:\\Program Files (x86)\\Chromium\\Application\\chrome.exe".to_string(),
            "--allow-read=C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe".to_string(),
            "--allow-read=C:\\Program Files\\Google\\Chrome SxS\\Application\\chrome.exe".to_string(),
            "--allow-read=C:\\Program Files\\Chromium\\Application\\chrome.exe".to_string(),
            "--allow-read=/usr/bin/chromium".to_string(),
        ];

        if matches!(runner_type, RunnerType::Docker) {
            deno_permissions.push("--allow-read=/".to_string());
        }

        for file in mount_files {
            let path = match runner_type {
                RunnerType::Host => file.to_string_lossy().to_string(),
                RunnerType::Docker => normalize_for_docker_path(file.to_path_buf()),
            };
            let mount_param = format!(r#"--allow-read={},--allow-write={}"#, path, path);
            deno_permissions.extend(mount_param.split(',').map(String::from));
        }

        for file in assets_files {
            let asset_param = format!(r#"--allow-read={}"#, file.to_string_lossy());
            deno_permissions.push(asset_param);
        }
        log::info!("deno permissions: {}", deno_permissions.join(" "));
        deno_permissions
    }
}

#[cfg(test)]
#[path = "deno_runner.test.rs"]
mod tests;
