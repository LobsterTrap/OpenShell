//! Navigator Sandbox library.
//!
//! This crate provides process sandboxing and monitoring capabilities.

mod grpc_client;
mod policy;
mod process;
mod proxy;
mod sandbox;
mod ssh;

use miette::{IntoDiagnostic, Result};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{debug, error, info};

use crate::policy::NetworkMode;
use crate::policy::SandboxPolicy;
use crate::proxy::ProxyHandle;
#[cfg(target_os = "linux")]
use crate::sandbox::linux::netns::NetworkNamespace;
pub use process::{ProcessHandle, ProcessStatus};

/// Run a command in the sandbox.
///
/// # Errors
///
/// Returns an error if the command fails to start or encounters a fatal error.
#[allow(clippy::too_many_arguments)]
pub async fn run_sandbox(
    command: Vec<String>,
    workdir: Option<String>,
    timeout_secs: u64,
    interactive: bool,
    policy_path: Option<String>,
    sandbox_id: Option<String>,
    navigator_endpoint: Option<String>,
    ssh_listen_addr: Option<String>,
    ssh_handshake_secret: Option<String>,
    ssh_handshake_skew_secs: u64,
    _health_check: bool,
    _health_port: u16,
) -> Result<i32> {
    let (program, args) = command
        .split_first()
        .ok_or_else(|| miette::miette!("No command specified"))?;

    // Load policy - either via gRPC or from local file
    let policy = load_policy(policy_path, sandbox_id, navigator_endpoint).await?;

    // Prepare filesystem: create and chown read_write directories
    prepare_filesystem(&policy)?;

    if let Some(listen_addr) = ssh_listen_addr {
        let addr: SocketAddr = listen_addr.parse().into_diagnostic()?;
        let policy_clone = policy.clone();
        let workdir_clone = workdir.clone();
        let secret = ssh_handshake_secret.unwrap_or_default();
        tokio::spawn(async move {
            if let Err(err) = ssh::run_ssh_server(
                addr,
                policy_clone,
                workdir_clone,
                secret,
                ssh_handshake_skew_secs,
            )
            .await
            {
                tracing::error!(error = %err, "SSH server failed");
            }
        });
    }

    // Create network namespace for proxy mode (Linux only)
    // This must be created before the proxy so the proxy can bind to the veth IP
    #[cfg(target_os = "linux")]
    let netns = if matches!(policy.network.mode, NetworkMode::Proxy) {
        match NetworkNamespace::create() {
            Ok(ns) => Some(ns),
            Err(e) => {
                // Log warning but continue without netns - allows running without CAP_NET_ADMIN
                tracing::warn!(
                    error = %e,
                    "Failed to create network namespace, continuing without isolation"
                );
                None
            }
        }
    } else {
        None
    };

    // On non-Linux, network namespace isolation is not supported
    #[cfg(not(target_os = "linux"))]
    #[allow(clippy::no_effect_underscore_binding)]
    let _netns: Option<()> = None;

    let _proxy = if matches!(policy.network.mode, NetworkMode::Proxy) {
        let proxy_policy = policy.network.proxy.as_ref().ok_or_else(|| {
            miette::miette!("Network mode is set to proxy but no proxy configuration was provided")
        })?;

        // If we have a network namespace, bind to the veth host IP
        #[cfg(target_os = "linux")]
        let bind_addr = netns.as_ref().map(|ns| {
            // Use the host IP with the configured port (or default 3128)
            let port = proxy_policy.http_addr.map_or(3128, |addr| addr.port());
            SocketAddr::new(ns.host_ip(), port)
        });

        #[cfg(not(target_os = "linux"))]
        let bind_addr: Option<SocketAddr> = None;

        Some(ProxyHandle::start_with_bind_addr(proxy_policy, bind_addr).await?)
    } else {
        None
    };

    #[cfg(target_os = "linux")]
    let mut handle = ProcessHandle::spawn(
        program,
        args,
        workdir.as_deref(),
        interactive,
        &policy,
        netns.as_ref(),
    )?;

    #[cfg(not(target_os = "linux"))]
    let mut handle = ProcessHandle::spawn(program, args, workdir.as_deref(), interactive, &policy)?;

    info!(pid = handle.pid(), "Process started");

    // Wait for process with optional timeout
    let result = if timeout_secs > 0 {
        if let Ok(result) = timeout(Duration::from_secs(timeout_secs), handle.wait()).await {
            result
        } else {
            error!("Process timed out, killing");
            handle.kill()?;
            return Ok(124); // Standard timeout exit code
        }
    } else {
        handle.wait().await
    };

    let status = result.into_diagnostic()?;

    info!(exit_code = status.code(), "Process exited");

    Ok(status.code())
}

/// Load sandbox policy from either gRPC or local file.
///
/// Priority:
/// 1. If `sandbox_id` and `navigator_endpoint` are provided, fetch via gRPC
/// 2. If `policy_path` is provided (or `NAVIGATOR_SANDBOX_POLICY` env var), load from file
/// 3. Otherwise, return an error
async fn load_policy(
    policy_path: Option<String>,
    sandbox_id: Option<String>,
    navigator_endpoint: Option<String>,
) -> Result<SandboxPolicy> {
    // Try gRPC mode first if both sandbox_id and endpoint are provided
    if let (Some(id), Some(endpoint)) = (&sandbox_id, &navigator_endpoint) {
        info!(
            sandbox_id = %id,
            endpoint = %endpoint,
            "Fetching sandbox policy via gRPC"
        );
        let proto_policy = grpc_client::fetch_policy(endpoint, id).await?;
        return SandboxPolicy::try_from(proto_policy);
    }

    // Fall back to file-based policy loading
    let policy_path = policy_path.or_else(|| std::env::var("NAVIGATOR_SANDBOX_POLICY").ok());

    if let Some(path) = policy_path {
        info!(policy_path = %path, "Loading sandbox policy from file");
        return SandboxPolicy::from_path(std::path::Path::new(&path));
    }

    // No policy source available
    Err(miette::miette!(
        "Sandbox policy required. Provide one of:\n\
         - --sandbox-id and --navigator-endpoint (or NAVIGATOR_SANDBOX_ID and NAVIGATOR_ENDPOINT env vars)\n\
         - --policy (or NAVIGATOR_SANDBOX_POLICY env var)"
    ))
}

/// Prepare filesystem for the sandboxed process.
///
/// Creates `read_write` directories if they don't exist and sets ownership
/// to the configured sandbox user/group. This runs as the supervisor (root)
/// before forking the child process.
#[cfg(unix)]
fn prepare_filesystem(policy: &SandboxPolicy) -> Result<()> {
    use nix::unistd::{Group, User, chown};

    let user_name = match policy.process.run_as_user.as_deref() {
        Some(name) if !name.is_empty() => Some(name),
        _ => None,
    };
    let group_name = match policy.process.run_as_group.as_deref() {
        Some(name) if !name.is_empty() => Some(name),
        _ => None,
    };

    // If no user/group configured, nothing to do
    if user_name.is_none() && group_name.is_none() {
        return Ok(());
    }

    // Resolve user and group
    let uid = if let Some(name) = user_name {
        Some(
            User::from_name(name)
                .into_diagnostic()?
                .ok_or_else(|| miette::miette!("Sandbox user not found: {name}"))?
                .uid,
        )
    } else {
        None
    };

    let gid = if let Some(name) = group_name {
        Some(
            Group::from_name(name)
                .into_diagnostic()?
                .ok_or_else(|| miette::miette!("Sandbox group not found: {name}"))?
                .gid,
        )
    } else {
        None
    };

    // Create and chown each read_write path
    for path in &policy.filesystem.read_write {
        if !path.exists() {
            debug!(path = %path.display(), "Creating read_write directory");
            std::fs::create_dir_all(path).into_diagnostic()?;
        }

        debug!(path = %path.display(), ?uid, ?gid, "Setting ownership on read_write directory");
        chown(path, uid, gid).into_diagnostic()?;
    }

    Ok(())
}

#[cfg(not(unix))]
fn prepare_filesystem(_policy: &SandboxPolicy) -> Result<()> {
    Ok(())
}
