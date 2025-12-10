use std::process::Command;

#[derive(Debug, PartialEq)]
pub enum DockerStatus {
    NotInstalled,
    NotRunning,
    Running,
}

/// Helper function for tests to skip Docker tests when Docker is not available.
/// Returns true if Docker is running and tests should proceed.
/// Returns false if Docker is not available and tests should be skipped.
#[cfg(test)]
pub fn skip_if_docker_unavailable() -> bool {
    is_docker_available() == DockerStatus::Running
}

/// Checks if Docker is available on the system by attempting to run 'docker info' command.
/// This function verifies both that Docker is installed and that the Docker daemon is running.
///
/// # Details
///
/// The function executes `docker info` which requires:
/// - Docker CLI to be installed and in PATH
/// - Docker daemon to be running and accessible
/// - Current user to have permissions to access Docker
///
/// # Returns
///
/// * `true` - If Docker is fully operational (installed, running and accessible)
/// * `false` - If Docker is not available for any reason:
///   - Docker is not installed
///   - Docker daemon is not running
///   - User lacks permissions
///   - Other Docker configuration issues
pub fn is_docker_available() -> DockerStatus {
    let docker_check = Command::new("docker").arg("info").output();

    match docker_check {
        Ok(output) => {
            if output.status.success() {
                DockerStatus::Running
            } else {
                DockerStatus::NotRunning
            }
        }
        Err(_) => DockerStatus::NotInstalled,
    }
}
