// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Container runtime detection and abstraction.
//!
//! OpenShell supports both Docker and Podman as container runtimes. This module
//! provides auto-detection with Podman preferred when both are available, plus
//! explicit override via the `OPENSHELL_CONTAINER_RUNTIME` environment variable
//! or `--container-runtime` CLI flag.

use miette::{Result, miette};
use std::fmt;
use std::path::Path;
use std::sync::OnceLock;

/// The environment variable used to override runtime auto-detection.
pub const RUNTIME_ENV_VAR: &str = "OPENSHELL_CONTAINER_RUNTIME";

/// Supported container runtimes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContainerRuntime {
    Docker,
    Podman,
}

impl ContainerRuntime {
    /// CLI binary name (`"docker"` or `"podman"`).
    pub fn binary_name(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Podman => "podman",
        }
    }

    /// The host-gateway DNS alias injected into containers.
    ///
    /// Docker uses `host.docker.internal`; Podman uses
    /// `host.containers.internal`.
    pub fn host_gateway_alias(self) -> &'static str {
        match self {
            Self::Docker => "host.docker.internal",
            Self::Podman => "host.containers.internal",
        }
    }

    /// Primary environment variable for specifying the daemon endpoint.
    ///
    /// Docker: `DOCKER_HOST`
    /// Podman: `CONTAINER_HOST` (Podman also respects `DOCKER_HOST` for compat)
    pub fn host_env_var(self) -> &'static str {
        match self {
            Self::Docker => "DOCKER_HOST",
            Self::Podman => "CONTAINER_HOST",
        }
    }

    /// Human-readable display name for user-facing messages.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Docker => "Docker",
            Self::Podman => "Podman",
        }
    }

    /// Parse a string into a `ContainerRuntime`.
    ///
    /// Accepts `"docker"` or `"podman"` (case-insensitive).
    pub fn from_str_loose(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "docker" => Ok(Self::Docker),
            "podman" => Ok(Self::Podman),
            other => Err(miette!(
                "unknown container runtime '{other}'. Expected 'docker' or 'podman'."
            )),
        }
    }
}

impl fmt::Display for ContainerRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.display_name())
    }
}

/// Cached result of runtime detection so we only probe once per process.
static DETECTED_RUNTIME: OnceLock<ContainerRuntime> = OnceLock::new();

/// Detect the container runtime, accepting an optional CLI override.
///
/// Priority:
/// 1. `cli_override` argument (from `--container-runtime` flag)
/// 2. `OPENSHELL_CONTAINER_RUNTIME` environment variable
/// 3. Auto-detect by probing sockets and binaries (Podman preferred)
pub fn detect_runtime(cli_override: Option<&str>) -> Result<ContainerRuntime> {
    // 1. CLI override takes highest priority
    if let Some(value) = cli_override {
        return ContainerRuntime::from_str_loose(value);
    }

    // 2. Environment variable override
    if let Ok(value) = std::env::var(RUNTIME_ENV_VAR) {
        let value = value.trim();
        if !value.is_empty() {
            return ContainerRuntime::from_str_loose(value);
        }
    }

    // 3. Auto-detect (cached)
    let runtime = DETECTED_RUNTIME.get_or_init(|| {
        auto_detect_runtime().unwrap_or_else(|_| {
            // If auto-detection fails entirely, fall back to Docker for
            // backward compatibility. The actual connection attempt will
            // produce a better error message later.
            ContainerRuntime::Docker
        })
    });

    Ok(*runtime)
}

/// Auto-detect the container runtime by probing sockets and binaries.
///
/// Podman is preferred when both are available.
fn auto_detect_runtime() -> Result<ContainerRuntime> {
    // Probe sockets first — a running daemon is a stronger signal than
    // just having the binary installed.
    if has_podman_socket() {
        tracing::debug!("auto-detected Podman (socket found)");
        return Ok(ContainerRuntime::Podman);
    }
    if has_docker_socket() {
        tracing::debug!("auto-detected Docker (socket found)");
        return Ok(ContainerRuntime::Docker);
    }

    // Fall back to checking for binaries on PATH
    if has_binary("podman") {
        tracing::debug!("auto-detected Podman (binary found on PATH)");
        return Ok(ContainerRuntime::Podman);
    }
    if has_binary("docker") {
        tracing::debug!("auto-detected Docker (binary found on PATH)");
        return Ok(ContainerRuntime::Docker);
    }

    Err(miette!(
        help = "Install Podman or Docker and ensure the daemon is running.\n\n  \
                Podman: https://podman.io/docs/installation\n  \
                Docker: https://docs.docker.com/get-docker/",
        "No container runtime found. OpenShell requires Podman or Docker."
    ))
}

// ---------------------------------------------------------------------------
// Socket probing
// ---------------------------------------------------------------------------

/// Well-known Docker socket paths.
const DOCKER_SOCKET_PATHS: &[&str] = &["/var/run/docker.sock"];

/// Well-known rootful Podman socket paths.
const PODMAN_ROOTFUL_SOCKET_PATHS: &[&str] =
    &["/run/podman/podman.sock", "/var/run/podman/podman.sock"];

/// Check whether a Podman socket exists (rootless first, then rootful).
fn has_podman_socket() -> bool {
    !find_podman_sockets().is_empty()
}

/// Check whether a Docker socket exists (default path, then alternatives).
fn has_docker_socket() -> bool {
    // DOCKER_HOST with a non-unix scheme (tcp://, ssh://) indicates Docker
    // is the intended runtime even without a local socket file.
    if let Ok(docker_host) = std::env::var("DOCKER_HOST") {
        if docker_host.starts_with("tcp://") || docker_host.starts_with("ssh://") {
            return true;
        }
    }
    !find_docker_sockets().is_empty()
}

/// Return the path to the rootless Podman socket for the current user.
///
/// Uses `XDG_RUNTIME_DIR` if set, otherwise falls back to `/run/user/<uid>/`.
fn podman_rootless_socket_path() -> Option<String> {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").ok().or_else(|| {
        // Fall back to /run/user/<uid>
        let uid = current_uid()?;
        Some(format!("/run/user/{uid}"))
    })?;

    Some(format!("{runtime_dir}/podman/podman.sock"))
}

/// Get the current user's UID.
///
/// Reads `/proc/self/status` to avoid a `libc` dependency.
fn current_uid() -> Option<u32> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        // Format: "Uid:\t<real>\t<effective>\t<saved>\t<filesystem>"
        if let Some(rest) = line.strip_prefix("Uid:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

/// Check whether a binary is on PATH.
fn has_binary(name: &str) -> bool {
    std::process::Command::new(name)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

// ---------------------------------------------------------------------------
// Socket path accessors (used by the bollard client abstraction in docker.rs)
// ---------------------------------------------------------------------------

/// Return all Podman socket paths that exist on this system.
///
/// Used by the bollard client connection logic to find the right socket.
pub fn find_podman_sockets() -> Vec<String> {
    let mut found = Vec::new();

    if let Some(path) = podman_rootless_socket_path() {
        if Path::new(&path).exists() {
            found.push(path);
        }
    }

    for path in PODMAN_ROOTFUL_SOCKET_PATHS {
        if Path::new(path).exists() && !found.iter().any(|p| p == path) {
            found.push((*path).to_string());
        }
    }

    found
}

/// Return all Docker socket paths that exist on this system.
///
/// Used by the bollard client connection logic to find the right socket.
/// Also checks `DOCKER_HOST` for unix socket paths.
pub fn find_docker_sockets() -> Vec<String> {
    let mut found = Vec::new();

    // Check DOCKER_HOST for a unix socket path
    if let Ok(docker_host) = std::env::var("DOCKER_HOST") {
        if let Some(path) = docker_host.strip_prefix("unix://") {
            if Path::new(path).exists() && !found.contains(&path.to_string()) {
                found.push(path.to_string());
            }
        }
    }

    for path in DOCKER_SOCKET_PATHS {
        if Path::new(path).exists() && !found.contains(&(*path).to_string()) {
            found.push((*path).to_string());
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        let alt_paths = [
            format!("{home}/.colima/docker.sock"),
            format!("{home}/.orbstack/run/docker.sock"),
        ];
        for path in alt_paths {
            if Path::new(&path).exists() && !found.contains(&path) {
                found.push(path);
            }
        }
    }

    found
}

/// Return all container runtime sockets that exist, tagged with their runtime.
///
/// Used by error messages to suggest alternative sockets.
pub fn find_all_sockets() -> Vec<(String, ContainerRuntime)> {
    let mut found = Vec::new();

    for path in find_podman_sockets() {
        found.push((path, ContainerRuntime::Podman));
    }
    for path in find_docker_sockets() {
        found.push((path, ContainerRuntime::Docker));
    }

    found
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- ContainerRuntime methods ---

    #[test]
    fn binary_name_values() {
        assert_eq!(ContainerRuntime::Docker.binary_name(), "docker");
        assert_eq!(ContainerRuntime::Podman.binary_name(), "podman");
    }

    #[test]
    fn host_gateway_alias_values() {
        assert_eq!(
            ContainerRuntime::Docker.host_gateway_alias(),
            "host.docker.internal"
        );
        assert_eq!(
            ContainerRuntime::Podman.host_gateway_alias(),
            "host.containers.internal"
        );
    }

    #[test]
    fn host_env_var_values() {
        assert_eq!(ContainerRuntime::Docker.host_env_var(), "DOCKER_HOST");
        assert_eq!(ContainerRuntime::Podman.host_env_var(), "CONTAINER_HOST");
    }

    #[test]
    fn display_name_values() {
        assert_eq!(ContainerRuntime::Docker.display_name(), "Docker");
        assert_eq!(ContainerRuntime::Podman.display_name(), "Podman");
    }

    #[test]
    fn display_trait() {
        assert_eq!(format!("{}", ContainerRuntime::Docker), "Docker");
        assert_eq!(format!("{}", ContainerRuntime::Podman), "Podman");
    }

    // --- from_str_loose ---

    #[test]
    fn from_str_loose_docker() {
        assert_eq!(
            ContainerRuntime::from_str_loose("docker").unwrap(),
            ContainerRuntime::Docker,
        );
    }

    #[test]
    fn from_str_loose_podman() {
        assert_eq!(
            ContainerRuntime::from_str_loose("podman").unwrap(),
            ContainerRuntime::Podman,
        );
    }

    #[test]
    fn from_str_loose_case_insensitive() {
        assert_eq!(
            ContainerRuntime::from_str_loose("Docker").unwrap(),
            ContainerRuntime::Docker,
        );
        assert_eq!(
            ContainerRuntime::from_str_loose("PODMAN").unwrap(),
            ContainerRuntime::Podman,
        );
        assert_eq!(
            ContainerRuntime::from_str_loose("Podman").unwrap(),
            ContainerRuntime::Podman,
        );
    }

    #[test]
    fn from_str_loose_unknown() {
        let err = ContainerRuntime::from_str_loose("containerd").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("containerd"), "error should mention the input");
        assert!(
            msg.contains("docker") || msg.contains("podman"),
            "error should mention valid options"
        );
    }

    // --- detect_runtime with CLI override ---

    #[test]
    fn detect_runtime_cli_override_docker() {
        let rt = detect_runtime(Some("docker")).unwrap();
        assert_eq!(rt, ContainerRuntime::Docker);
    }

    #[test]
    fn detect_runtime_cli_override_podman() {
        let rt = detect_runtime(Some("podman")).unwrap();
        assert_eq!(rt, ContainerRuntime::Podman);
    }

    #[test]
    fn detect_runtime_cli_override_invalid() {
        let err = detect_runtime(Some("rkt")).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("rkt"));
    }

    // --- socket path helpers ---

    #[test]
    fn podman_rootless_socket_path_format() {
        // If we can determine the UID, the path should follow the expected format
        if let Some(path) = podman_rootless_socket_path() {
            assert!(
                path.contains("/podman/podman.sock"),
                "path should end with /podman/podman.sock: {path}"
            );
        }
    }

    #[test]
    fn current_uid_returns_value_on_linux() {
        // On Linux, /proc/self/status should always be readable
        if cfg!(target_os = "linux") {
            let uid = current_uid();
            assert!(uid.is_some(), "should be able to read UID on Linux");
        }
    }

    #[test]
    fn find_all_sockets_returns_vec() {
        // Should not panic regardless of system state
        let sockets = find_all_sockets();
        // Each entry should have a non-empty path
        for (path, _runtime) in &sockets {
            assert!(!path.is_empty(), "socket path should not be empty");
        }
    }
}
