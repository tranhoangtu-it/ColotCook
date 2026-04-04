use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum FilesystemIsolationMode {
    Off,
    #[default]
    WorkspaceOnly,
    AllowList,
}

impl FilesystemIsolationMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::WorkspaceOnly => "workspace-only",
            Self::AllowList => "allow-list",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SandboxConfig {
    pub enabled: Option<bool>,
    pub namespace_restrictions: Option<bool>,
    pub network_isolation: Option<bool>,
    pub filesystem_mode: Option<FilesystemIsolationMode>,
    pub allowed_mounts: Vec<String>,
    pub resource_limits: Option<ResourceLimits>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SandboxRequest {
    pub enabled: bool,
    pub namespace_restrictions: bool,
    pub network_isolation: bool,
    pub filesystem_mode: FilesystemIsolationMode,
    pub allowed_mounts: Vec<String>,
    pub resource_limits: ResourceLimits,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ContainerEnvironment {
    pub in_container: bool,
    pub markers: Vec<String>,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SandboxStatus {
    pub enabled: bool,
    pub requested: SandboxRequest,
    pub supported: bool,
    pub active: bool,
    pub namespace_supported: bool,
    pub namespace_active: bool,
    pub network_supported: bool,
    pub network_active: bool,
    pub filesystem_mode: FilesystemIsolationMode,
    pub filesystem_active: bool,
    pub allowed_mounts: Vec<String>,
    pub resource_limits: ResourceLimits,
    pub in_container: bool,
    pub container_markers: Vec<String>,
    pub fallback_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxDetectionInputs<'a> {
    pub env_pairs: Vec<(String, String)>,
    pub dockerenv_exists: bool,
    pub containerenv_exists: bool,
    pub proc_1_cgroup: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxSandboxCommand {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// Resource limits for sandboxed processes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceLimits {
    /// Maximum CPU time in seconds (0 = unlimited).
    pub max_cpu_seconds: u64,
    /// Maximum memory in bytes (0 = unlimited).
    pub max_memory_bytes: u64,
    /// Maximum number of open file descriptors.
    pub max_open_files: u64,
    /// Maximum number of child processes.
    pub max_processes: u64,
    /// Maximum output file size in bytes.
    pub max_file_size_bytes: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_cpu_seconds: 300,                // 5 minutes
            max_memory_bytes: 512 * 1024 * 1024, // 512 MB
            max_open_files: 256,
            max_processes: 64,
            max_file_size_bytes: 100 * 1024 * 1024, // 100 MB
        }
    }
}

impl SandboxConfig {
    #[must_use]
    pub fn resolve_request(
        &self,
        enabled_override: Option<bool>,
        namespace_override: Option<bool>,
        network_override: Option<bool>,
        filesystem_mode_override: Option<FilesystemIsolationMode>,
        allowed_mounts_override: Option<Vec<String>>,
    ) -> SandboxRequest {
        SandboxRequest {
            enabled: enabled_override.unwrap_or(self.enabled.unwrap_or(true)),
            namespace_restrictions: namespace_override
                .unwrap_or(self.namespace_restrictions.unwrap_or(true)),
            network_isolation: network_override.unwrap_or(self.network_isolation.unwrap_or(false)),
            filesystem_mode: filesystem_mode_override
                .or(self.filesystem_mode)
                .unwrap_or_default(),
            allowed_mounts: allowed_mounts_override.unwrap_or_else(|| self.allowed_mounts.clone()),
            resource_limits: self.resource_limits.clone().unwrap_or_default(),
        }
    }
}

#[must_use]
pub fn detect_container_environment() -> ContainerEnvironment {
    let proc_1_cgroup = fs::read_to_string("/proc/1/cgroup").ok();
    detect_container_environment_from(SandboxDetectionInputs {
        env_pairs: env::vars().collect(),
        dockerenv_exists: Path::new("/.dockerenv").exists(),
        containerenv_exists: Path::new("/run/.containerenv").exists(),
        proc_1_cgroup: proc_1_cgroup.as_deref(),
    })
}

#[must_use]
pub fn detect_container_environment_from(
    inputs: SandboxDetectionInputs<'_>,
) -> ContainerEnvironment {
    let mut markers = Vec::new();
    if inputs.dockerenv_exists {
        markers.push("/.dockerenv".to_string());
    }
    if inputs.containerenv_exists {
        markers.push("/run/.containerenv".to_string());
    }
    for (key, value) in inputs.env_pairs {
        let normalized = key.to_ascii_lowercase();
        if matches!(
            normalized.as_str(),
            "container" | "docker" | "podman" | "kubernetes_service_host"
        ) && !value.is_empty()
        {
            markers.push(format!("env:{key}={value}"));
        }
    }
    if let Some(cgroup) = inputs.proc_1_cgroup {
        for needle in ["docker", "containerd", "kubepods", "podman", "libpod"] {
            if cgroup.contains(needle) {
                markers.push(format!("/proc/1/cgroup:{needle}"));
            }
        }
    }
    markers.sort();
    markers.dedup();
    ContainerEnvironment {
        in_container: !markers.is_empty(),
        markers,
    }
}

#[must_use]
pub fn resolve_sandbox_status(config: &SandboxConfig, cwd: &Path) -> SandboxStatus {
    let request = config.resolve_request(None, None, None, None, None);
    resolve_sandbox_status_for_request(&request, cwd)
}

#[must_use]
pub fn resolve_sandbox_status_for_request(request: &SandboxRequest, cwd: &Path) -> SandboxStatus {
    let container = detect_container_environment();
    let namespace_supported = cfg!(target_os = "linux") && command_exists("unshare");
    let network_supported = namespace_supported;
    let filesystem_active =
        request.enabled && request.filesystem_mode != FilesystemIsolationMode::Off;
    let mut fallback_reasons = Vec::new();

    if request.enabled && request.namespace_restrictions && !namespace_supported {
        fallback_reasons
            .push("namespace isolation unavailable (requires Linux with `unshare`)".to_string());
    }
    if request.enabled && request.network_isolation && !network_supported {
        fallback_reasons
            .push("network isolation unavailable (requires Linux with `unshare`)".to_string());
    }
    if request.enabled
        && request.filesystem_mode == FilesystemIsolationMode::AllowList
        && request.allowed_mounts.is_empty()
    {
        fallback_reasons
            .push("filesystem allow-list requested without configured mounts".to_string());
    }

    let active = request.enabled
        && (!request.namespace_restrictions || namespace_supported)
        && (!request.network_isolation || network_supported);

    let allowed_mounts = normalize_mounts(&request.allowed_mounts, cwd);

    SandboxStatus {
        enabled: request.enabled,
        requested: request.clone(),
        supported: namespace_supported,
        active,
        namespace_supported,
        namespace_active: request.enabled && request.namespace_restrictions && namespace_supported,
        network_supported,
        network_active: request.enabled && request.network_isolation && network_supported,
        filesystem_mode: request.filesystem_mode,
        filesystem_active,
        allowed_mounts,
        resource_limits: request.resource_limits.clone(),
        in_container: container.in_container,
        container_markers: container.markers,
        fallback_reason: (!fallback_reasons.is_empty()).then(|| fallback_reasons.join("; ")),
    }
}

#[must_use]
pub fn build_linux_sandbox_command(
    command: &str,
    cwd: &Path,
    status: &SandboxStatus,
) -> Option<LinuxSandboxCommand> {
    if !cfg!(target_os = "linux")
        || !status.enabled
        || (!status.namespace_active && !status.network_active)
    {
        return None;
    }

    let mut args = vec![
        "--user".to_string(),
        "--map-root-user".to_string(),
        "--mount".to_string(),
        "--ipc".to_string(),
        "--pid".to_string(),
        "--uts".to_string(),
        "--fork".to_string(),
    ];
    if status.network_active {
        args.push("--net".to_string());
    }
    args.push("sh".to_string());
    args.push("-lc".to_string());
    args.push(command.to_string());

    let sandbox_home = cwd.join(".sandbox-home");
    let sandbox_tmp = cwd.join(".sandbox-tmp");
    let mut env = vec![
        ("HOME".to_string(), sandbox_home.display().to_string()),
        ("TMPDIR".to_string(), sandbox_tmp.display().to_string()),
        (
            "COLOTCOOK_SANDBOX_FILESYSTEM_MODE".to_string(),
            status.filesystem_mode.as_str().to_string(),
        ),
        (
            "COLOTCOOK_SANDBOX_ALLOWED_MOUNTS".to_string(),
            status.allowed_mounts.join(":"),
        ),
    ];
    if let Ok(path) = env::var("PATH") {
        env.push(("PATH".to_string(), path));
    }

    Some(LinuxSandboxCommand {
        program: "unshare".to_string(),
        args,
        env,
    })
}

/// Build a shell command prefix that applies resource limits via `ulimit`.
/// Returns an empty vec if no limits are configured.
#[must_use]
pub fn resource_limit_shell_prefix(limits: &ResourceLimits) -> Vec<String> {
    let mut parts = Vec::new();
    if limits.max_cpu_seconds > 0 {
        parts.push(format!("ulimit -t {}", limits.max_cpu_seconds));
    }
    if limits.max_memory_bytes > 0 {
        // ulimit -v uses KB
        let kb = limits.max_memory_bytes / 1024;
        parts.push(format!("ulimit -v {kb}"));
    }
    if limits.max_open_files > 0 {
        parts.push(format!("ulimit -n {}", limits.max_open_files));
    }
    if limits.max_processes > 0 {
        parts.push(format!("ulimit -u {}", limits.max_processes));
    }
    if limits.max_file_size_bytes > 0 {
        // ulimit -f uses 512-byte blocks
        let blocks = limits.max_file_size_bytes / 512;
        parts.push(format!("ulimit -f {blocks}"));
    }
    parts
}

fn normalize_mounts(mounts: &[String], cwd: &Path) -> Vec<String> {
    let cwd = cwd.to_path_buf();
    mounts
        .iter()
        .map(|mount| {
            let path = PathBuf::from(mount);
            if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            }
        })
        .map(|path| path.display().to_string())
        .collect()
}

fn command_exists(command: &str) -> bool {
    env::var_os("PATH")
        .is_some_and(|paths| env::split_paths(&paths).any(|path| path.join(command).exists()))
}

/// Paths that should never be writable in a sandbox.
const SENSITIVE_PATHS: &[&str] = &[
    "/etc/passwd",
    "/etc/shadow",
    "/etc/sudoers",
    "/root",
    "/proc/sys",
    "/sys",
];

/// Validate that `allowed_mounts` don't include sensitive system paths.
pub fn validate_allowed_mounts(mounts: &[String]) -> Result<(), String> {
    for mount in mounts {
        let mount_path = Path::new(mount);
        for sensitive in SENSITIVE_PATHS {
            let sensitive_path = Path::new(sensitive);
            if mount_path.starts_with(sensitive_path) || sensitive_path.starts_with(mount_path) {
                return Err(format!(
                    "mount path '{mount}' overlaps with sensitive system path '{sensitive}'"
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        build_linux_sandbox_command, detect_container_environment_from, FilesystemIsolationMode,
        SandboxConfig, SandboxDetectionInputs,
    };
    use std::path::Path;

    #[test]
    fn detects_container_markers_from_multiple_sources() {
        let detected = detect_container_environment_from(SandboxDetectionInputs {
            env_pairs: vec![("container".to_string(), "docker".to_string())],
            dockerenv_exists: true,
            containerenv_exists: false,
            proc_1_cgroup: Some("12:memory:/docker/abc"),
        });

        assert!(detected.in_container);
        assert!(detected
            .markers
            .iter()
            .any(|marker| marker == "/.dockerenv"));
        assert!(detected
            .markers
            .iter()
            .any(|marker| marker == "env:container=docker"));
        assert!(detected
            .markers
            .iter()
            .any(|marker| marker == "/proc/1/cgroup:docker"));
    }

    #[test]
    fn resolves_request_with_overrides() {
        let config = SandboxConfig {
            enabled: Some(true),
            namespace_restrictions: Some(true),
            network_isolation: Some(false),
            filesystem_mode: Some(FilesystemIsolationMode::WorkspaceOnly),
            allowed_mounts: vec!["logs".to_string()],
            resource_limits: None,
        };

        let request = config.resolve_request(
            Some(true),
            Some(false),
            Some(true),
            Some(FilesystemIsolationMode::AllowList),
            Some(vec!["tmp".to_string()]),
        );

        assert!(request.enabled);
        assert!(!request.namespace_restrictions);
        assert!(request.network_isolation);
        assert_eq!(request.filesystem_mode, FilesystemIsolationMode::AllowList);
        assert_eq!(request.allowed_mounts, vec!["tmp"]);
    }

    #[test]
    fn builds_linux_launcher_with_network_flag_when_requested() {
        let config = SandboxConfig::default();
        let status = super::resolve_sandbox_status_for_request(
            &config.resolve_request(
                Some(true),
                Some(true),
                Some(true),
                Some(FilesystemIsolationMode::WorkspaceOnly),
                None,
            ),
            Path::new("/workspace"),
        );

        if let Some(launcher) =
            build_linux_sandbox_command("printf hi", Path::new("/workspace"), &status)
        {
            assert_eq!(launcher.program, "unshare");
            assert!(launcher.args.iter().any(|arg| arg == "--mount"));
            assert!(launcher.args.iter().any(|arg| arg == "--net") == status.network_active);
        }
    }

    #[test]
    fn validates_allowed_mounts_rejects_sensitive_paths() {
        let result = super::validate_allowed_mounts(&["/etc/passwd".to_string()]);
        assert!(result.is_err());

        let result = super::validate_allowed_mounts(&["/root".to_string()]);
        assert!(result.is_err());

        let result = super::validate_allowed_mounts(&["/sys".to_string()]);
        assert!(result.is_err());
    }

    #[test]
    fn validates_allowed_mounts_accepts_safe_paths() {
        let result = super::validate_allowed_mounts(&["/home/user/data".to_string()]);
        assert!(result.is_ok());

        let result = super::validate_allowed_mounts(&["/tmp/workspace".to_string()]);
        assert!(result.is_ok());
    }

    #[test]
    fn resource_limit_shell_prefix_builds_correct_commands() {
        let limits = super::ResourceLimits {
            max_cpu_seconds: 300,
            max_memory_bytes: 512 * 1024 * 1024,
            max_open_files: 256,
            max_processes: 64,
            max_file_size_bytes: 100 * 1024 * 1024,
        };

        let prefix = super::resource_limit_shell_prefix(&limits);
        assert!(!prefix.is_empty());
        assert!(prefix.iter().any(|p| p.contains("ulimit -t 300")));
        assert!(prefix.iter().any(|p| p.contains("ulimit -v")));
        assert!(prefix.iter().any(|p| p.contains("ulimit -n 256")));
        assert!(prefix.iter().any(|p| p.contains("ulimit -u 64")));
        assert!(prefix.iter().any(|p| p.contains("ulimit -f")));
    }

    #[test]
    fn resource_limits_have_sane_defaults() {
        let limits = super::ResourceLimits::default();
        assert!(limits.max_cpu_seconds > 0, "CPU limit must be positive");
        assert!(
            limits.max_cpu_seconds <= 3600,
            "CPU limit should be reasonable"
        );
        assert!(
            limits.max_memory_bytes >= 64 * 1024 * 1024,
            "Memory must be at least 64MB"
        );
        assert!(
            limits.max_memory_bytes <= 8 * 1024 * 1024 * 1024,
            "Memory should be reasonable"
        );
        assert!(
            limits.max_open_files >= 16,
            "Must allow some file descriptors"
        );
        assert!(limits.max_processes >= 4, "Must allow some processes");
    }

    #[test]
    fn validate_mounts_rejects_etc_shadow() {
        assert!(super::validate_allowed_mounts(&["/etc/shadow".to_string()]).is_err());
        assert!(super::validate_allowed_mounts(&["/etc".to_string()]).is_err());
    }

    #[test]
    fn validate_mounts_rejects_root() {
        assert!(super::validate_allowed_mounts(&["/root".to_string()]).is_err());
        assert!(super::validate_allowed_mounts(&["/root/.ssh".to_string()]).is_err());
    }

    #[test]
    fn validate_mounts_rejects_proc_sys() {
        assert!(super::validate_allowed_mounts(&["/proc/sys".to_string()]).is_err());
        assert!(super::validate_allowed_mounts(&["/sys".to_string()]).is_err());
    }

    #[test]
    fn validate_mounts_accepts_safe_paths() {
        assert!(super::validate_allowed_mounts(&["/home/user/project".to_string()]).is_ok());
        assert!(super::validate_allowed_mounts(&["/tmp".to_string()]).is_ok());
        assert!(super::validate_allowed_mounts(&["/opt/myapp".to_string()]).is_ok());
    }

    #[test]
    fn resource_limit_prefix_generates_ulimit_commands() {
        let limits = super::ResourceLimits {
            max_cpu_seconds: 60,
            max_memory_bytes: 256 * 1024 * 1024,
            max_open_files: 128,
            max_processes: 32,
            max_file_size_bytes: 50 * 1024 * 1024,
        };
        let prefix = super::resource_limit_shell_prefix(&limits);
        assert!(prefix.iter().any(|s| s.contains("ulimit -t 60")));
        assert!(prefix.iter().any(|s| s.contains("ulimit -n 128")));
        assert!(prefix.iter().any(|s| s.contains("ulimit -u 32")));
    }

    // --- Additional sandbox tests ---

    #[test]
    fn filesystem_isolation_mode_as_str() {
        assert_eq!(FilesystemIsolationMode::Off.as_str(), "off");
        assert_eq!(
            FilesystemIsolationMode::WorkspaceOnly.as_str(),
            "workspace-only"
        );
        assert_eq!(FilesystemIsolationMode::AllowList.as_str(), "allow-list");
    }

    #[test]
    fn filesystem_isolation_mode_default_is_workspace_only() {
        assert_eq!(
            FilesystemIsolationMode::default(),
            FilesystemIsolationMode::WorkspaceOnly
        );
    }

    #[test]
    fn sandbox_config_default_is_all_none() {
        let config = SandboxConfig::default();
        assert!(config.enabled.is_none());
        assert!(config.namespace_restrictions.is_none());
        assert!(config.network_isolation.is_none());
        assert!(config.filesystem_mode.is_none());
        assert!(config.allowed_mounts.is_empty());
        assert!(config.resource_limits.is_none());
    }

    #[test]
    fn resolve_request_uses_defaults_when_no_overrides() {
        let config = SandboxConfig::default();
        let request = config.resolve_request(None, None, None, None, None);
        // Default enabled = true
        assert!(request.enabled);
        // Default namespace_restrictions = true
        assert!(request.namespace_restrictions);
        // Default network_isolation = false
        assert!(!request.network_isolation);
        // Default filesystem_mode = WorkspaceOnly
        assert_eq!(
            request.filesystem_mode,
            FilesystemIsolationMode::WorkspaceOnly
        );
    }

    #[test]
    fn resolve_request_override_enabled_false() {
        let config = SandboxConfig {
            enabled: Some(true),
            ..Default::default()
        };
        let request = config.resolve_request(Some(false), None, None, None, None);
        assert!(!request.enabled);
    }

    #[test]
    fn resolve_request_uses_config_enabled_when_no_override() {
        let config = SandboxConfig {
            enabled: Some(false),
            ..Default::default()
        };
        let request = config.resolve_request(None, None, None, None, None);
        assert!(!request.enabled);
    }

    #[test]
    fn resolve_request_uses_config_resource_limits() {
        use super::ResourceLimits;
        let custom_limits = ResourceLimits {
            max_cpu_seconds: 99,
            max_memory_bytes: 1024,
            max_open_files: 10,
            max_processes: 5,
            max_file_size_bytes: 512,
        };
        let config = SandboxConfig {
            resource_limits: Some(custom_limits.clone()),
            ..Default::default()
        };
        let request = config.resolve_request(None, None, None, None, None);
        assert_eq!(request.resource_limits.max_cpu_seconds, 99);
    }

    #[test]
    fn resource_limits_default_values() {
        use super::ResourceLimits;
        let limits = ResourceLimits::default();
        assert_eq!(limits.max_cpu_seconds, 300);
        assert_eq!(limits.max_memory_bytes, 512 * 1024 * 1024);
        assert_eq!(limits.max_open_files, 256);
        assert_eq!(limits.max_processes, 64);
        assert_eq!(limits.max_file_size_bytes, 100 * 1024 * 1024);
    }

    #[test]
    fn resource_limit_shell_prefix_zero_values_skipped() {
        use super::ResourceLimits;
        let limits = ResourceLimits {
            max_cpu_seconds: 0,
            max_memory_bytes: 0,
            max_open_files: 0,
            max_processes: 0,
            max_file_size_bytes: 0,
        };
        let prefix = super::resource_limit_shell_prefix(&limits);
        assert!(prefix.is_empty());
    }

    #[test]
    fn resource_limit_shell_prefix_memory_converted_to_kb() {
        use super::ResourceLimits;
        let limits = ResourceLimits {
            max_cpu_seconds: 0,
            max_memory_bytes: 1024 * 1024, // 1 MB = 1024 KB
            max_open_files: 0,
            max_processes: 0,
            max_file_size_bytes: 0,
        };
        let prefix = super::resource_limit_shell_prefix(&limits);
        assert!(prefix.iter().any(|s| s.contains("ulimit -v 1024")));
    }

    #[test]
    fn resource_limit_shell_prefix_file_size_converted_to_blocks() {
        use super::ResourceLimits;
        let limits = ResourceLimits {
            max_cpu_seconds: 0,
            max_memory_bytes: 0,
            max_open_files: 0,
            max_processes: 0,
            max_file_size_bytes: 1024, // 2 blocks of 512 bytes
        };
        let prefix = super::resource_limit_shell_prefix(&limits);
        assert!(prefix.iter().any(|s| s.contains("ulimit -f 2")));
    }

    #[test]
    fn detect_container_from_no_markers() {
        let env = detect_container_environment_from(super::SandboxDetectionInputs {
            env_pairs: vec![],
            dockerenv_exists: false,
            containerenv_exists: false,
            proc_1_cgroup: None,
        });
        assert!(!env.in_container);
        assert!(env.markers.is_empty());
    }

    #[test]
    fn detect_container_from_containerenv_file() {
        let env = detect_container_environment_from(super::SandboxDetectionInputs {
            env_pairs: vec![],
            dockerenv_exists: false,
            containerenv_exists: true,
            proc_1_cgroup: None,
        });
        assert!(env.in_container);
        assert!(env.markers.iter().any(|m| m == "/run/.containerenv"));
    }

    #[test]
    fn detect_container_from_kubernetes_env() {
        let env = detect_container_environment_from(super::SandboxDetectionInputs {
            env_pairs: vec![(
                "KUBERNETES_SERVICE_HOST".to_string(),
                "10.0.0.1".to_string(),
            )],
            dockerenv_exists: false,
            containerenv_exists: false,
            proc_1_cgroup: None,
        });
        assert!(env.in_container);
        assert!(env
            .markers
            .iter()
            .any(|m| m.contains("KUBERNETES_SERVICE_HOST")));
    }

    #[test]
    fn detect_container_from_empty_env_value_not_marked() {
        // Env vars with empty values should not trigger container detection
        let env = detect_container_environment_from(super::SandboxDetectionInputs {
            env_pairs: vec![("DOCKER".to_string(), String::new())],
            dockerenv_exists: false,
            containerenv_exists: false,
            proc_1_cgroup: None,
        });
        assert!(!env.in_container);
    }

    #[test]
    fn detect_container_from_podman_cgroup() {
        let env = detect_container_environment_from(super::SandboxDetectionInputs {
            env_pairs: vec![],
            dockerenv_exists: false,
            containerenv_exists: false,
            proc_1_cgroup: Some("10:cpuset:/podman/abc123"),
        });
        assert!(env.in_container);
        assert!(env.markers.iter().any(|m| m.contains("podman")));
    }

    #[test]
    fn detect_container_from_kubepods_cgroup() {
        let env = detect_container_environment_from(super::SandboxDetectionInputs {
            env_pairs: vec![],
            dockerenv_exists: false,
            containerenv_exists: false,
            proc_1_cgroup: Some("12:memory:/kubepods/pod-abc"),
        });
        assert!(env.in_container);
        assert!(env.markers.iter().any(|m| m.contains("kubepods")));
    }

    #[test]
    fn detect_container_markers_dedup() {
        // Markers should be deduplicated
        let env = detect_container_environment_from(super::SandboxDetectionInputs {
            env_pairs: vec![],
            dockerenv_exists: true,
            containerenv_exists: false,
            proc_1_cgroup: None,
        });
        let dockerenv_count = env.markers.iter().filter(|m| *m == "/.dockerenv").count();
        assert_eq!(dockerenv_count, 1);
    }

    #[test]
    fn validate_allowed_mounts_empty_list_is_ok() {
        assert!(super::validate_allowed_mounts(&[]).is_ok());
    }

    #[test]
    fn validate_allowed_mounts_multiple_safe_paths_all_ok() {
        let mounts = vec![
            "/home/user".to_string(),
            "/data/workspace".to_string(),
            "/tmp/builds".to_string(),
        ];
        assert!(super::validate_allowed_mounts(&mounts).is_ok());
    }

    #[test]
    fn validate_allowed_mounts_child_of_sensitive_path_is_err() {
        // /root/.ssh is a child of /root, which is sensitive
        assert!(super::validate_allowed_mounts(&["/root/.ssh/keys".to_string()]).is_err());
    }

    #[test]
    fn validate_allowed_mounts_parent_of_sensitive_path_is_err() {
        // Mounting "/" would be a parent of /etc/passwd
        assert!(super::validate_allowed_mounts(&["/".to_string()]).is_err());
    }

    #[test]
    fn sandbox_config_serialization_roundtrip() {
        let config = SandboxConfig {
            enabled: Some(true),
            namespace_restrictions: Some(false),
            network_isolation: Some(true),
            filesystem_mode: Some(FilesystemIsolationMode::AllowList),
            allowed_mounts: vec!["/data".to_string()],
            resource_limits: None,
        };
        let json = serde_json::to_string(&config).expect("serialize");
        let decoded: SandboxConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(config, decoded);
    }

    #[test]
    fn filesystem_isolation_mode_serialization() {
        let mode = FilesystemIsolationMode::AllowList;
        let json = serde_json::to_string(&mode).expect("serialize");
        assert_eq!(json, "\"allow-list\"");
        let decoded: FilesystemIsolationMode = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, mode);
    }
}
