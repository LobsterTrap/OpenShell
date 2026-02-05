//! CLI command implementations.

use bytes::Bytes;
use futures::StreamExt;
use http_body_util::Full;
use hyper::{Request, StatusCode};
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use miette::{IntoDiagnostic, Result, WrapErr};
use navigator_bootstrap::{
    DeployOptions, default_local_kubeconfig_path, print_kubeconfig, update_local_kubeconfig,
};
use navigator_core::proto::{
    CreateSandboxRequest, DeleteSandboxRequest, GetSandboxRequest, HealthRequest,
    LandlockCompatibility, ListSandboxesRequest, NetworkMode, Sandbox, SandboxPhase, SandboxPolicy,
    SandboxSpec, WatchSandboxRequest, navigator_client::NavigatorClient,
};
use owo_colors::OwoColorize;
use serde::{Deserialize, Serialize};
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::time::{Duration, Instant};

// Re-export SSH functions for backward compatibility
pub use crate::ssh::{sandbox_connect, sandbox_ssh_proxy};

/// Convert a sandbox phase integer to a human-readable string.
fn phase_name(phase: i32) -> &'static str {
    match SandboxPhase::try_from(phase) {
        Ok(SandboxPhase::Unspecified) => "Unspecified",
        Ok(SandboxPhase::Provisioning) => "Provisioning",
        Ok(SandboxPhase::Ready) => "Ready",
        Ok(SandboxPhase::Error) => "Error",
        Ok(SandboxPhase::Deleting) => "Deleting",
        Ok(SandboxPhase::Unknown) | Err(_) => "Unknown",
    }
}

/// Live-updating display showing spinner with phase and latest log line.
struct LogDisplay {
    mp: MultiProgress,
    spinner: ProgressBar,
    phase: String,
    latest_log: String,
}

impl LogDisplay {
    fn new() -> Self {
        let mp = MultiProgress::new();

        // Spinner for phase status + latest log
        let spinner = mp.add(ProgressBar::new_spinner());
        spinner.set_style(
            ProgressStyle::with_template("{spinner:.cyan} {msg}")
                .unwrap_or_else(|_| ProgressStyle::default_spinner()),
        );
        spinner.enable_steady_tick(Duration::from_millis(120));

        Self {
            mp,
            spinner,
            phase: String::new(),
            latest_log: String::new(),
        }
    }

    fn set_phase(&mut self, phase: &str) {
        self.phase = phase.to_string();
        self.update_spinner();
    }

    fn finish_phase(&mut self, phase: &str) {
        self.phase = phase.to_string();
        self.latest_log.clear();
        self.spinner
            .finish_with_message(format_phase_label(&self.phase));
    }

    fn shutdown(&self) {
        self.spinner.disable_steady_tick();
        self.spinner.finish_and_clear();
    }

    fn set_log(&mut self, line: String) {
        let line = line.trim().to_string();
        if line.is_empty() {
            return;
        }
        self.latest_log = line;
        self.update_spinner();
    }

    fn update_spinner(&self) {
        let msg = if self.latest_log.is_empty() {
            format_phase_label(&self.phase)
        } else {
            format!(
                "{} {}",
                format_phase_label(&self.phase),
                self.latest_log.dimmed()
            )
        };
        self.spinner.set_message(msg);
    }

    /// Print a line above the progress bars (for static header content).
    fn println(&self, msg: &str) {
        let _ = self.mp.println(msg);
    }
}

fn print_sandbox_header(sandbox: &Sandbox, display: Option<&LogDisplay>) {
    let lines = [
        format!("{}", "Created sandbox:".cyan().bold()),
        format!("  {} {}", "Id:".dimmed(), sandbox.id),
        format!("  {} {}", "Name:".dimmed(), sandbox.name),
        format!("  {} {}", "Namespace:".dimmed(), sandbox.namespace),
    ];
    match display {
        Some(d) => {
            for line in lines {
                d.println(&line);
            }
        }
        None => {
            for line in lines {
                println!("{line}");
            }
        }
    }
}

fn format_phase_label(phase: &str) -> String {
    let colored = match phase {
        "Ready" => phase.green().to_string(),
        "Error" => phase.red().to_string(),
        "Provisioning" => phase.yellow().to_string(),
        _ => phase.dimmed().to_string(),
    };
    format!("{} {colored}", "Phase:".dimmed())
}

/// Show cluster status.
#[allow(clippy::branches_sharing_code)]
pub async fn cluster_status(server: &str) -> Result<()> {
    println!("{}", "Server Status".bold().cyan());
    println!();
    println!("  {} {}", "Server:".dimmed(), server);

    // Try to connect and get health
    match NavigatorClient::connect(server.to_string()).await {
        Ok(mut client) => match client.health(HealthRequest {}).await {
            Ok(response) => {
                let health = response.into_inner();
                println!("  {} {}", "Status:".dimmed(), "Connected".green());
                println!("  {} {}", "Version:".dimmed(), health.version);
            }
            Err(e) => {
                if let Some(status) = http_health_check(server).await? {
                    if status.is_success() {
                        println!("  {} {}", "Status:".dimmed(), "Connected (HTTP)".yellow());
                        println!("  {} {}", "HTTP: ".dimmed(), status);
                        println!("  {} {}", "gRPC error:".dimmed(), e);
                    } else {
                        println!("  {} {}", "Status:".dimmed(), "Error".red());
                        println!("  {} {}", "HTTP:".dimmed(), status);
                        println!("  {} {}", "gRPC error:".dimmed(), e);
                    }
                } else {
                    println!("  {} {}", "Status:".dimmed(), "Error".red());
                    println!("  {} {}", "Error:".dimmed(), e);
                }
            }
        },
        Err(e) => {
            if let Some(status) = http_health_check(server).await? {
                if status.is_success() {
                    println!("  {} {}", "Status:".dimmed(), "Connected (HTTP)".yellow());
                    println!("  {} {}", "HTTP:".dimmed(), status);
                    println!("  {} {}", "gRPC error:".dimmed(), e);
                } else {
                    println!("  {} {}", "Status:".dimmed(), "Disconnected".red());
                    println!("  {} {}", "HTTP:".dimmed(), status);
                    println!("  {} {}", "Error:".dimmed(), e);
                }
            } else {
                println!("  {} {}", "Status:".dimmed(), "Disconnected".red());
                println!("  {} {}", "Error:".dimmed(), e);
            }
        }
    }

    Ok(())
}

async fn http_health_check(server: &str) -> Result<Option<StatusCode>> {
    let base = server.trim_end_matches('/');
    let uri: hyper::Uri = format!("{base}/healthz").parse().into_diagnostic()?;
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build_http();
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Full::new(Bytes::new()))
        .into_diagnostic()?;
    let resp = client.request(req).await.into_diagnostic()?;
    Ok(Some(resp.status()))
}

/// Provision or start a local cluster.
pub async fn cluster_admin_deploy(
    name: &str,
    update_kube_config: bool,
    get_kubeconfig: bool,
) -> Result<()> {
    eprintln!("Deploying cluster {name}...");
    let options = DeployOptions::new(name);
    let handle = navigator_bootstrap::deploy_cluster(options).await?;
    eprintln!("Cluster {name} ready.");

    if update_kube_config {
        let target_path = default_local_kubeconfig_path()?;
        update_local_kubeconfig(name, &target_path)?;
        eprintln!("Updated kubeconfig at {}", target_path.display());
    }

    if get_kubeconfig {
        print_kubeconfig(name)?;
    }

    eprintln!("Stored kubeconfig: {}", handle.kubeconfig_path().display());
    Ok(())
}

/// Stop a local cluster.
pub async fn cluster_admin_stop(name: &str) -> Result<()> {
    eprintln!("Stopping cluster {name}...");
    let handle = navigator_bootstrap::cluster_handle(name)?;
    handle.stop().await?;
    eprintln!("Cluster {name} stopped.");
    Ok(())
}

/// Destroy a local cluster and its state.
pub async fn cluster_admin_destroy(name: &str) -> Result<()> {
    eprintln!("Destroying cluster {name}...");
    let handle = navigator_bootstrap::cluster_handle(name)?;
    handle.destroy().await?;
    eprintln!("Cluster {name} destroyed.");
    Ok(())
}

/// Create a sandbox with default settings.
pub async fn sandbox_create(server: &str) -> Result<()> {
    let mut client = NavigatorClient::connect(server.to_string())
        .await
        .into_diagnostic()?;

    let policy = load_dev_sandbox_policy()?;
    let request = CreateSandboxRequest {
        spec: Some(SandboxSpec {
            policy: Some(policy),
            ..SandboxSpec::default()
        }),
    };

    let response = client.create_sandbox(request).await.into_diagnostic()?;
    let sandbox = response
        .into_inner()
        .sandbox
        .ok_or_else(|| miette::miette!("sandbox missing from response"))?;

    let interactive = std::io::stdout().is_terminal();
    let sandbox_id = sandbox.id.clone();

    // Set up display
    let mut display = if interactive {
        Some(LogDisplay::new())
    } else {
        None
    };

    // Print header
    print_sandbox_header(&sandbox, display.as_ref());

    // Set initial phase
    if let Some(d) = display.as_mut() {
        d.set_phase(phase_name(sandbox.phase));
    } else {
        println!("  {}", format_phase_label(phase_name(sandbox.phase)));
    }

    let mut stream = client
        .watch_sandbox(WatchSandboxRequest {
            id: sandbox.id.clone(),
            follow_status: true,
            follow_logs: true,
            follow_events: true,
            log_tail_lines: 200,
            event_tail: 0,
            stop_on_terminal: true,
        })
        .await
        .into_diagnostic()?
        .into_inner();

    let mut last_phase = sandbox.phase;
    let mut last_error_reason = String::new();
    let start_time = Instant::now();
    let provision_timeout = Duration::from_secs(120);

    while let Some(item) = stream.next().await {
        // Check for timeout
        if start_time.elapsed() > provision_timeout {
            if let Some(d) = display.as_mut() {
                d.finish_phase(phase_name(last_phase));
            }
            println!();
            return Err(miette::miette!(
                "sandbox provisioning timed out after {:?}",
                provision_timeout
            ));
        }

        let evt = item.into_diagnostic()?;
        match evt.payload {
            Some(navigator_core::proto::sandbox_stream_event::Payload::Sandbox(s)) => {
                last_phase = s.phase;
                // Capture error reason from conditions only when phase is Error
                // to avoid showing stale transient error reasons
                if SandboxPhase::try_from(s.phase) == Ok(SandboxPhase::Error)
                    && let Some(status) = &s.status
                {
                    for condition in &status.conditions {
                        if condition.r#type == "Ready"
                            && condition.status.eq_ignore_ascii_case("false")
                        {
                            last_error_reason =
                                format!("{}: {}", condition.reason, condition.message);
                        }
                    }
                }
                if let Some(d) = display.as_mut() {
                    d.set_phase(phase_name(s.phase));
                } else {
                    println!("  {}", format_phase_label(phase_name(s.phase)));
                }
            }
            Some(navigator_core::proto::sandbox_stream_event::Payload::Log(line)) => {
                if let Some(d) = display.as_mut() {
                    d.set_log(line.message);
                }
            }
            Some(navigator_core::proto::sandbox_stream_event::Payload::Event(ev)) => {
                let reason = if ev.reason.is_empty() {
                    "Event"
                } else {
                    &ev.reason
                };
                let msg = if ev.message.is_empty() {
                    ""
                } else {
                    &ev.message
                };
                let line = format!("{} {} {}", "EVENT".dimmed(), reason, msg);
                if let Some(d) = display.as_mut() {
                    d.set_log(line);
                }
            }
            Some(navigator_core::proto::sandbox_stream_event::Payload::Warning(w)) => {
                let line = format!("{} {}", "WARN".yellow(), w.message);
                if let Some(d) = display.as_mut() {
                    d.set_log(line);
                }
            }
            None => {}
        }
    }

    // Finish up - check final phase
    if let Some(d) = display.as_mut() {
        d.finish_phase(phase_name(last_phase));
        d.shutdown();
    }
    drop(display);
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    println!();

    match SandboxPhase::try_from(last_phase) {
        Ok(SandboxPhase::Ready) => {
            drop(stream);
            drop(client);
            sandbox_connect(server, &sandbox_id).await
        }
        Ok(SandboxPhase::Error) => {
            if last_error_reason.is_empty() {
                Err(miette::miette!(
                    "sandbox entered error phase while provisioning"
                ))
            } else {
                Err(miette::miette!(
                    "sandbox entered error phase while provisioning: {}",
                    last_error_reason
                ))
            }
        }
        _ => Err(miette::miette!(
            "sandbox provisioning stream ended before reaching terminal phase"
        )),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DevSandboxPolicyFile {
    version: u32,
    filesystem: DevFilesystemPolicy,
    network: DevNetworkPolicy,
    landlock: DevLandlockPolicy,
    process: DevProcessPolicy,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DevFilesystemPolicy {
    include_workdir: bool,
    read_only: Vec<String>,
    read_write: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DevNetworkPolicy {
    mode: String,
    #[serde(default)]
    proxy: Option<DevProxyPolicy>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DevProxyPolicy {
    #[serde(default)]
    unix_socket: Option<String>,
    #[serde(default)]
    http_addr: Option<String>,
    #[serde(default)]
    allow_hosts: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DevLandlockPolicy {
    compatibility: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DevProcessPolicy {
    #[serde(default)]
    run_as_user: Option<String>,
    #[serde(default)]
    run_as_group: Option<String>,
}

fn load_dev_sandbox_policy() -> Result<SandboxPolicy> {
    let policy_path = std::env::var("NAVIGATOR_SANDBOX_POLICY")
        .unwrap_or_else(|_| "dev-sandbox-policy.yaml".to_string());
    let path = Path::new(&policy_path);
    let contents = std::fs::read_to_string(path)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to read sandbox policy from {}", path.display()))?;
    let raw: DevSandboxPolicyFile = serde_yaml::from_str(&contents)
        .into_diagnostic()
        .wrap_err("failed to parse sandbox policy yaml")?;

    let network_mode = match raw.network.mode.as_str() {
        "proxy" => NetworkMode::Proxy as i32,
        "allow" => NetworkMode::Allow as i32,
        _ => NetworkMode::Block as i32,
    };

    let proxy = raw
        .network
        .proxy
        .map(|proxy| navigator_core::proto::ProxyPolicy {
            unix_socket: proxy.unix_socket.unwrap_or_default(),
            http_addr: proxy.http_addr.unwrap_or_default(),
            allow_hosts: proxy.allow_hosts,
        });

    let landlock_compat = match raw.landlock.compatibility.as_str() {
        "hard_requirement" => LandlockCompatibility::HardRequirement as i32,
        _ => LandlockCompatibility::BestEffort as i32,
    };

    Ok(SandboxPolicy {
        version: raw.version,
        filesystem: Some(navigator_core::proto::FilesystemPolicy {
            read_only: raw.filesystem.read_only,
            read_write: raw.filesystem.read_write,
            include_workdir: raw.filesystem.include_workdir,
        }),
        network: Some(navigator_core::proto::NetworkPolicy {
            mode: network_mode,
            proxy,
        }),
        landlock: Some(navigator_core::proto::LandlockPolicy {
            compatibility: landlock_compat,
        }),
        process: Some(navigator_core::proto::ProcessPolicy {
            run_as_user: raw.process.run_as_user.unwrap_or_default(),
            run_as_group: raw.process.run_as_group.unwrap_or_default(),
        }),
    })
}

/// Fetch a sandbox by id.
pub async fn sandbox_get(server: &str, id: &str) -> Result<()> {
    let mut client = NavigatorClient::connect(server.to_string())
        .await
        .into_diagnostic()?;

    let response = client
        .get_sandbox(GetSandboxRequest { id: id.to_string() })
        .await
        .into_diagnostic()?;
    let sandbox = response
        .into_inner()
        .sandbox
        .ok_or_else(|| miette::miette!("sandbox missing from response"))?;

    println!("Sandbox:");
    println!("  {} {}", "Id:".dimmed(), sandbox.id);
    println!("  {} {}", "Name:".dimmed(), sandbox.name);
    println!("  {} {}", "Namespace:".dimmed(), sandbox.namespace);
    println!("  {} {}", "Phase:".dimmed(), phase_name(sandbox.phase));

    if let Some(spec) = &sandbox.spec
        && let Some(policy) = &spec.policy
    {
        println!();
        print_sandbox_policy(policy);
    }

    Ok(())
}

/// Serializable policy structure for YAML output.
#[derive(Serialize)]
struct PolicyYaml {
    version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    filesystem: Option<FilesystemYaml>,
    #[serde(skip_serializing_if = "Option::is_none")]
    network: Option<NetworkYaml>,
    #[serde(skip_serializing_if = "Option::is_none")]
    landlock: Option<LandlockYaml>,
    #[serde(skip_serializing_if = "Option::is_none")]
    process: Option<ProcessYaml>,
}

#[derive(Serialize)]
struct FilesystemYaml {
    include_workdir: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    read_only: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    read_write: Vec<String>,
}

#[derive(Serialize)]
struct NetworkYaml {
    mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    proxy: Option<ProxyYaml>,
}

#[derive(Serialize)]
struct ProxyYaml {
    #[serde(skip_serializing_if = "String::is_empty")]
    unix_socket: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    http_addr: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    allow_hosts: Vec<String>,
}

#[derive(Serialize)]
struct LandlockYaml {
    compatibility: String,
}

#[derive(Serialize)]
struct ProcessYaml {
    #[serde(skip_serializing_if = "String::is_empty")]
    run_as_user: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    run_as_group: String,
}

/// Convert proto policy to serializable YAML structure.
fn policy_to_yaml(policy: &SandboxPolicy) -> PolicyYaml {
    let filesystem = policy.filesystem.as_ref().map(|fs| FilesystemYaml {
        include_workdir: fs.include_workdir,
        read_only: fs.read_only.clone(),
        read_write: fs.read_write.clone(),
    });

    let network = policy.network.as_ref().map(|net| {
        let mode = match NetworkMode::try_from(net.mode) {
            Ok(NetworkMode::Block) => "block",
            Ok(NetworkMode::Proxy) => "proxy",
            Ok(NetworkMode::Allow) => "allow",
            _ => "unspecified",
        }
        .to_string();

        let proxy = net.proxy.as_ref().map(|p| ProxyYaml {
            unix_socket: p.unix_socket.clone(),
            http_addr: p.http_addr.clone(),
            allow_hosts: p.allow_hosts.clone(),
        });

        NetworkYaml { mode, proxy }
    });

    let landlock = policy.landlock.as_ref().map(|ll| {
        let compatibility = match LandlockCompatibility::try_from(ll.compatibility) {
            Ok(LandlockCompatibility::BestEffort) => "best_effort",
            Ok(LandlockCompatibility::HardRequirement) => "hard_requirement",
            _ => "unspecified",
        }
        .to_string();
        LandlockYaml { compatibility }
    });

    let process = policy.process.as_ref().and_then(|p| {
        if p.run_as_user.is_empty() && p.run_as_group.is_empty() {
            None
        } else {
            Some(ProcessYaml {
                run_as_user: p.run_as_user.clone(),
                run_as_group: p.run_as_group.clone(),
            })
        }
    });

    PolicyYaml {
        version: policy.version,
        filesystem,
        network,
        landlock,
        process,
    }
}

/// Print a single YAML line with dimmed keys and regular values.
fn print_yaml_line(line: &str) {
    // Find leading whitespace
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];

    // Handle list items
    if let Some(rest) = trimmed.strip_prefix("- ") {
        print!("{indent}");
        print!("{}", "- ".dimmed());
        print!("{rest}");
        println!();
        return;
    }

    // Handle key: value pairs
    if let Some(colon_pos) = trimmed.find(':') {
        let key = &trimmed[..colon_pos];
        let after_colon = &trimmed[colon_pos + 1..];

        print!("{indent}");
        print!("{}", key.dimmed());
        print!("{}", ":".dimmed());

        if after_colon.is_empty() {
            // Key with nested content (no value on this line)
        } else if let Some(value) = after_colon.strip_prefix(' ') {
            // Key: value
            print!(" {value}");
        } else {
            // Shouldn't happen in valid YAML, but handle it
            print!("{after_colon}");
        }
        println!();
        return;
    }

    // Plain line (shouldn't happen often in YAML)
    println!("{line}");
}

/// Print sandbox policy as YAML with dimmed keys.
fn print_sandbox_policy(policy: &SandboxPolicy) {
    println!("Policy:");
    let policy_yaml = policy_to_yaml(policy);
    if let Ok(yaml_str) = serde_yaml::to_string(&policy_yaml) {
        // Indent the YAML output and skip the initial "---" line
        for line in yaml_str.lines() {
            if line == "---" {
                continue;
            }
            print!("  ");
            print_yaml_line(line);
        }
    }
}

/// List sandboxes.
pub async fn sandbox_list(server: &str, limit: u32, offset: u32, ids_only: bool) -> Result<()> {
    let mut client = NavigatorClient::connect(server.to_string())
        .await
        .into_diagnostic()?;

    let response = client
        .list_sandboxes(ListSandboxesRequest { limit, offset })
        .await
        .into_diagnostic()?;

    let sandboxes = response.into_inner().sandboxes;
    if sandboxes.is_empty() {
        if !ids_only {
            println!("No sandboxes found.");
        }
        return Ok(());
    }

    if ids_only {
        for sandbox in sandboxes {
            println!("{}", sandbox.id);
        }
        return Ok(());
    }

    // Calculate column widths
    let id_width = sandboxes
        .iter()
        .map(|s| s.id.len())
        .max()
        .unwrap_or(2)
        .max(2);
    let name_width = sandboxes
        .iter()
        .map(|s| s.name.len())
        .max()
        .unwrap_or(4)
        .max(4);
    let ns_width = sandboxes
        .iter()
        .map(|s| s.namespace.len())
        .max()
        .unwrap_or(9)
        .max(9);

    // Print header
    println!(
        "{:<id_width$}  {:<name_width$}  {:<ns_width$}  {}",
        "ID".bold(),
        "NAME".bold(),
        "NAMESPACE".bold(),
        "PHASE".bold(),
    );

    // Print rows
    for sandbox in sandboxes {
        let phase = phase_name(sandbox.phase);
        let phase_colored = match SandboxPhase::try_from(sandbox.phase) {
            Ok(SandboxPhase::Ready) => phase.green().to_string(),
            Ok(SandboxPhase::Error) => phase.red().to_string(),
            Ok(SandboxPhase::Provisioning) => phase.yellow().to_string(),
            Ok(SandboxPhase::Deleting) => phase.dimmed().to_string(),
            _ => phase.to_string(),
        };
        println!(
            "{:<id_width$}  {:<name_width$}  {:<ns_width$}  {}",
            sandbox.id, sandbox.name, sandbox.namespace, phase_colored,
        );
    }

    Ok(())
}

/// Delete a sandbox by id.
pub async fn sandbox_delete(server: &str, ids: &[String]) -> Result<()> {
    let mut client = NavigatorClient::connect(server.to_string())
        .await
        .into_diagnostic()?;

    for id in ids {
        let response = client
            .delete_sandbox(DeleteSandboxRequest { id: id.clone() })
            .await
            .into_diagnostic()?;

        let deleted = response.into_inner().deleted;
        if deleted {
            println!("Deleted sandbox {id}");
        } else {
            println!("Sandbox {id} not found");
        }
    }

    Ok(())
}
