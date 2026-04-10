// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Network namespace isolation for sandboxed processes.
//!
//! Creates an isolated network namespace with a veth pair connecting
//! the sandbox to the host. This ensures the sandboxed process can only
//! communicate through the proxy running on the host side of the veth.

use miette::{IntoDiagnostic, Result};
use std::net::IpAddr;
use std::os::unix::io::RawFd;
use std::os::unix::process::CommandExt;
use std::process::Command;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Default subnet for sandbox networking.
const SUBNET_PREFIX: &str = "10.200.0";
const HOST_IP_SUFFIX: u8 = 1;
const SANDBOX_IP_SUFFIX: u8 = 2;

/// How the namespace was created — determines cleanup and command execution.
#[derive(Debug)]
enum NamespaceMode {
    /// Created via `ip netns add` — bind-mounted at `/var/run/netns/{name}`.
    /// Commands inside the namespace use `ip netns exec {name} ...`.
    Named,
    /// Created via `unshare(CLONE_NEWNET)` in a holder process — referenced
    /// by PID at `/proc/{pid}/ns/net`. Used when `ip netns add` fails
    /// (e.g. rootless Podman with nested user namespaces).
    /// Commands inside the namespace use `nsenter -t {pid} -n -- ...`.
    Unshare { holder: std::process::Child },
}

/// Handle to a network namespace with veth pair.
///
/// The namespace and veth interfaces are automatically cleaned up on drop.
#[derive(Debug)]
pub struct NetworkNamespace {
    /// Namespace name (e.g., "sandbox-{uuid}") — used for logging and
    /// bypass-monitor log prefix matching in both modes.
    name: String,
    /// How the namespace was created.
    mode: NamespaceMode,
    /// Host-side veth interface name
    veth_host: String,
    /// Sandbox-side veth interface name (inside namespace, used only during setup)
    _veth_sandbox: String,
    /// Host-side IP address (proxy binds here)
    host_ip: IpAddr,
    /// Sandbox-side IP address
    sandbox_ip: IpAddr,
    /// File descriptor for the namespace (for setns)
    ns_fd: Option<RawFd>,
}

impl NetworkNamespace {
    /// Create a new isolated network namespace with veth pair.
    ///
    /// Sets up:
    /// - A new network namespace (named or PID-based)
    /// - A veth pair connecting host and sandbox
    /// - IP addresses on both ends (10.200.0.1/24 and 10.200.0.2/24)
    /// - Default route in sandbox pointing to host
    ///
    /// Tries `ip netns add` first (named namespace, works with real root).
    /// Falls back to `unshare(CLONE_NEWNET)` in a holder process when named
    /// namespaces are unavailable (e.g. rootless Podman / rootless Docker).
    ///
    /// # Errors
    ///
    /// Returns an error if both namespace creation strategies fail.
    pub fn create() -> Result<Self> {
        match Self::create_named() {
            Ok(ns) => Ok(ns),
            Err(named_err) => {
                info!(
                    error = %named_err,
                    "Named namespace unavailable, trying unshare fallback \
                     (expected in rootless container runtimes)"
                );
                Self::create_via_unshare().map_err(|unshare_err| {
                    miette::miette!(
                        "Network namespace creation failed.\n\
                         Named namespace: {named_err}\n\
                         Unshare fallback: {unshare_err}"
                    )
                })
            }
        }
    }

    /// Create a namespace via `ip netns add` (named, bind-mounted).
    ///
    /// This is the standard path for Docker and rootful Podman where the
    /// container runs with real root privileges.
    fn create_named() -> Result<Self> {
        let (name, veth_host, veth_sandbox, host_ip, sandbox_ip) = Self::generate_names();

        info!(
            namespace = %name,
            host_veth = %veth_host,
            sandbox_veth = %veth_sandbox,
            mode = "named",
            "Creating network namespace"
        );

        // Create the named namespace (bind-mount at /var/run/netns/)
        run_ip(&["netns", "add", &name])?;

        // Set up veth pair and network configuration.
        // On failure, clean up the named namespace.
        let cleanup_ns = || {
            let _ = run_ip(&["netns", "delete", &name]);
        };

        if let Err(e) = Self::setup_veth_pair(&name, &veth_host, &veth_sandbox) {
            cleanup_ns();
            return Err(e);
        }

        if let Err(e) = Self::configure_host_side(&veth_host, host_ip) {
            let _ = run_ip(&["link", "delete", &veth_host]);
            cleanup_ns();
            return Err(e);
        }

        if let Err(e) =
            Self::configure_sandbox_side_named(&name, &veth_sandbox, sandbox_ip, host_ip)
        {
            let _ = run_ip(&["link", "delete", &veth_host]);
            cleanup_ns();
            return Err(e);
        }

        // Open the namespace fd for later setns() calls
        let ns_path = format!("/var/run/netns/{name}");
        let ns_fd = match nix::fcntl::open(
            ns_path.as_str(),
            nix::fcntl::OFlag::O_RDONLY,
            nix::sys::stat::Mode::empty(),
        ) {
            Ok(fd) => Some(fd),
            Err(e) => {
                warn!(error = %e, "Failed to open namespace fd");
                None
            }
        };

        info!(
            namespace = %name,
            host_ip = %host_ip,
            sandbox_ip = %sandbox_ip,
            mode = "named",
            "Network namespace created"
        );

        Ok(Self {
            name,
            mode: NamespaceMode::Named,
            veth_host,
            _veth_sandbox: veth_sandbox,
            host_ip,
            sandbox_ip,
            ns_fd,
        })
    }

    /// Create a namespace via `unshare(CLONE_NEWNET)` in a holder process.
    ///
    /// This is the fallback path for rootless Podman / rootless Docker where
    /// `ip netns add` fails because it requires a bind-mount to `/var/run/netns/`
    /// which needs real root or mount namespace privileges.
    ///
    /// A helper process (`sleep infinity`) calls `unshare(CLONE_NEWNET)` to
    /// create the namespace, and the namespace fd is obtained from
    /// `/proc/{pid}/ns/net`. Commands inside the namespace use
    /// `nsenter -t {pid} -n -- ...` instead of `ip netns exec {name} ...`.
    fn create_via_unshare() -> Result<Self> {
        let (name, veth_host, veth_sandbox, host_ip, sandbox_ip) = Self::generate_names();

        info!(
            namespace = %name,
            host_veth = %veth_host,
            sandbox_veth = %veth_sandbox,
            mode = "unshare",
            "Creating network namespace via unshare"
        );

        // Spawn a holder process that creates and holds the network namespace.
        // SAFETY: unshare is async-signal-safe and safe in pre_exec.
        let mut cmd = Command::new("sleep");
        cmd.arg("infinity")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[allow(unsafe_code)]
        unsafe {
            cmd.pre_exec(|| {
                let rc = libc::unshare(libc::CLONE_NEWNET);
                if rc != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut holder = cmd.spawn().into_diagnostic()?;

        let holder_pid = holder.id();

        // Open the namespace fd from /proc/{pid}/ns/net
        let ns_path = format!("/proc/{holder_pid}/ns/net");
        let ns_fd = match nix::fcntl::open(
            ns_path.as_str(),
            nix::fcntl::OFlag::O_RDONLY,
            nix::sys::stat::Mode::empty(),
        ) {
            Ok(fd) => Some(fd),
            Err(e) => {
                let _ = holder.kill();
                let _ = holder.wait();
                return Err(miette::miette!(
                    "Failed to open namespace fd at {ns_path}: {e}"
                ));
            }
        };

        // Set up veth pair, moving the sandbox end into the holder's namespace
        // by PID (ip link set ... netns {pid}).
        let cleanup = |holder: &mut std::process::Child| {
            let _ = holder.kill();
            let _ = holder.wait();
        };

        let pid_str = holder_pid.to_string();
        if let Err(e) = Self::setup_veth_pair(&pid_str, &veth_host, &veth_sandbox) {
            cleanup(&mut holder);
            return Err(e);
        }

        if let Err(e) = Self::configure_host_side(&veth_host, host_ip) {
            let _ = run_ip(&["link", "delete", &veth_host]);
            cleanup(&mut holder);
            return Err(e);
        }

        if let Err(e) =
            Self::configure_sandbox_side_unshare(holder_pid, &veth_sandbox, sandbox_ip, host_ip)
        {
            let _ = run_ip(&["link", "delete", &veth_host]);
            cleanup(&mut holder);
            return Err(e);
        }

        info!(
            namespace = %name,
            holder_pid = holder_pid,
            host_ip = %host_ip,
            sandbox_ip = %sandbox_ip,
            mode = "unshare",
            "Network namespace created via unshare"
        );

        Ok(Self {
            name,
            mode: NamespaceMode::Unshare { holder },
            veth_host,
            _veth_sandbox: veth_sandbox,
            host_ip,
            sandbox_ip,
            ns_fd,
        })
    }

    /// Generate identifiers and addresses for a new namespace.
    fn generate_names() -> (String, String, String, IpAddr, IpAddr) {
        let id = Uuid::new_v4();
        let short_id = &id.to_string()[..8];
        let name = format!("sandbox-{short_id}");
        let veth_host = format!("veth-h-{short_id}");
        let veth_sandbox = format!("veth-s-{short_id}");
        let host_ip: IpAddr = format!("{SUBNET_PREFIX}.{HOST_IP_SUFFIX}").parse().unwrap();
        let sandbox_ip: IpAddr = format!("{SUBNET_PREFIX}.{SANDBOX_IP_SUFFIX}")
            .parse()
            .unwrap();
        (name, veth_host, veth_sandbox, host_ip, sandbox_ip)
    }

    /// Create a veth pair and move the sandbox end into the target namespace.
    ///
    /// `ns_ref` is either a namespace name (for named mode) or a PID string
    /// (for unshare mode) — `ip link set ... netns` accepts both.
    fn setup_veth_pair(ns_ref: &str, veth_host: &str, veth_sandbox: &str) -> Result<()> {
        // Create veth pair
        run_ip(&[
            "link",
            "add",
            veth_host,
            "type",
            "veth",
            "peer",
            "name",
            veth_sandbox,
        ])?;

        // Move sandbox veth into namespace (by name or by PID)
        if let Err(e) = run_ip(&["link", "set", veth_sandbox, "netns", ns_ref]) {
            let _ = run_ip(&["link", "delete", veth_host]);
            return Err(e);
        }

        Ok(())
    }

    /// Configure the host-side veth interface (runs on the host network).
    fn configure_host_side(veth_host: &str, host_ip: IpAddr) -> Result<()> {
        let host_cidr = format!("{host_ip}/24");
        run_ip(&["addr", "add", &host_cidr, "dev", veth_host])?;
        run_ip(&["link", "set", veth_host, "up"])?;
        Ok(())
    }

    /// Configure the sandbox-side network inside a named namespace.
    fn configure_sandbox_side_named(
        name: &str,
        veth_sandbox: &str,
        sandbox_ip: IpAddr,
        host_ip: IpAddr,
    ) -> Result<()> {
        let sandbox_cidr = format!("{sandbox_ip}/24");
        let host_ip_str = host_ip.to_string();
        run_ip_netns(name, &["addr", "add", &sandbox_cidr, "dev", veth_sandbox])?;
        run_ip_netns(name, &["link", "set", veth_sandbox, "up"])?;
        run_ip_netns(name, &["link", "set", "lo", "up"])?;
        run_ip_netns(name, &["route", "add", "default", "via", &host_ip_str])?;
        Ok(())
    }

    /// Configure the sandbox-side network inside a PID-based namespace.
    fn configure_sandbox_side_unshare(
        pid: u32,
        veth_sandbox: &str,
        sandbox_ip: IpAddr,
        host_ip: IpAddr,
    ) -> Result<()> {
        let sandbox_cidr = format!("{sandbox_ip}/24");
        let host_ip_str = host_ip.to_string();
        run_ip_in_ns(pid, &["addr", "add", &sandbox_cidr, "dev", veth_sandbox])?;
        run_ip_in_ns(pid, &["link", "set", veth_sandbox, "up"])?;
        run_ip_in_ns(pid, &["link", "set", "lo", "up"])?;
        run_ip_in_ns(pid, &["route", "add", "default", "via", &host_ip_str])?;
        Ok(())
    }

    /// Get the host-side IP address (proxy should bind to this).
    #[must_use]
    pub const fn host_ip(&self) -> IpAddr {
        self.host_ip
    }

    /// Get the sandbox-side IP address.
    #[must_use]
    pub const fn sandbox_ip(&self) -> IpAddr {
        self.sandbox_ip
    }

    /// Get the namespace name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Enter this network namespace.
    ///
    /// Must be called from the child process after fork, before exec.
    /// Uses `setns()` to switch the calling process into the namespace.
    ///
    /// # Errors
    ///
    /// Returns an error if setns fails.
    ///
    /// # Safety
    ///
    /// This function should only be called in a `pre_exec` context after fork.
    #[allow(unsafe_code)]
    pub fn enter(&self) -> Result<()> {
        if let Some(fd) = self.ns_fd {
            debug!(namespace = %self.name, "Entering network namespace via setns");
            // SAFETY: setns is safe to call after fork, before exec
            let result = unsafe { libc::setns(fd, libc::CLONE_NEWNET) };
            if result != 0 {
                return Err(miette::miette!(
                    "setns failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(())
        } else {
            Err(miette::miette!(
                "No namespace file descriptor available for setns"
            ))
        }
    }

    /// Get the namespace file descriptor for use with clone/unshare.
    #[must_use]
    pub const fn ns_fd(&self) -> Option<RawFd> {
        self.ns_fd
    }

    /// Run an iptables command inside this namespace, dispatching to the
    /// appropriate execution method based on how the namespace was created.
    fn run_iptables(&self, iptables_cmd: &str, args: &[&str]) -> Result<()> {
        match &self.mode {
            NamespaceMode::Named => run_iptables_netns(&self.name, iptables_cmd, args),
            NamespaceMode::Unshare { holder } => {
                run_iptables_in_ns(holder.id(), iptables_cmd, args)
            }
        }
    }

    /// Install iptables rules for bypass detection inside the namespace.
    ///
    /// Sets up OUTPUT chain rules that:
    /// 1. ACCEPT traffic destined for the proxy (host_ip:proxy_port)
    /// 2. ACCEPT loopback traffic
    /// 3. ACCEPT established/related connections (response packets)
    /// 4. LOG + REJECT all other TCP/UDP traffic (bypass attempts)
    ///
    /// This provides two benefits:
    /// - **Fast-fail UX**: applications get immediate ECONNREFUSED instead of
    ///   a 30-second timeout when they bypass the proxy
    /// - **Diagnostics**: iptables LOG entries are picked up by the bypass
    ///   monitor to emit structured tracing events
    ///
    /// Degrades gracefully if `iptables` is not available — the namespace
    /// still provides isolation via routing, just without fast-fail and
    /// diagnostic logging.
    pub fn install_bypass_rules(&self, proxy_port: u16) -> Result<()> {
        // Check if iptables is available before attempting to install rules.
        let iptables_path = match find_iptables() {
            Some(path) => path,
            None => {
                warn!(
                    namespace = %self.name,
                    search_paths = ?IPTABLES_SEARCH_PATHS,
                    "iptables not found; bypass detection rules will not be installed. \
                     Install the iptables package for proxy bypass diagnostics."
                );
                return Ok(());
            }
        };

        let host_ip_str = self.host_ip.to_string();
        let proxy_port_str = proxy_port.to_string();
        let log_prefix = format!("openshell:bypass:{}:", &self.name);

        info!(
            namespace = %self.name,
            iptables = %iptables_path,
            proxy_addr = %format!("{}:{}", host_ip_str, proxy_port),
            "Installing bypass detection rules"
        );

        // Install IPv4 rules
        if let Err(e) = self.install_bypass_rules_for(
            &iptables_path,
            &host_ip_str,
            &proxy_port_str,
            &log_prefix,
        ) {
            warn!(
                namespace = %self.name,
                error = %e,
                "Failed to install IPv4 bypass detection rules"
            );
            return Err(e);
        }

        // Install IPv6 rules — best-effort.
        // Skip the proxy ACCEPT rule for IPv6 since the proxy address is IPv4.
        if let Some(ip6_path) = find_ip6tables(&iptables_path) {
            if let Err(e) = self.install_bypass_rules_for_v6(&ip6_path, &log_prefix) {
                warn!(
                    namespace = %self.name,
                    error = %e,
                    "Failed to install IPv6 bypass detection rules (non-fatal)"
                );
            }
        }

        info!(
            namespace = %self.name,
            "Bypass detection rules installed"
        );

        Ok(())
    }

    /// Install bypass detection rules for a specific iptables variant (iptables or ip6tables).
    fn install_bypass_rules_for(
        &self,
        iptables_cmd: &str,
        host_ip: &str,
        proxy_port: &str,
        log_prefix: &str,
    ) -> Result<()> {
        // Rule 1: ACCEPT traffic to the proxy
        self.run_iptables(
            iptables_cmd,
            &[
                "-A",
                "OUTPUT",
                "-d",
                &format!("{host_ip}/32"),
                "-p",
                "tcp",
                "--dport",
                proxy_port,
                "-j",
                "ACCEPT",
            ],
        )?;

        // Rule 2: ACCEPT loopback traffic
        self.run_iptables(iptables_cmd, &["-A", "OUTPUT", "-o", "lo", "-j", "ACCEPT"])?;

        // Rule 3: ACCEPT established/related connections (response packets)
        self.run_iptables(
            iptables_cmd,
            &[
                "-A",
                "OUTPUT",
                "-m",
                "conntrack",
                "--ctstate",
                "ESTABLISHED,RELATED",
                "-j",
                "ACCEPT",
            ],
        )?;

        // Rule 4: LOG TCP SYN bypass attempts (rate-limited)
        // LOG rule failure is non-fatal — the REJECT rule still provides fast-fail.
        if let Err(e) = self.run_iptables(
            iptables_cmd,
            &[
                "-A",
                "OUTPUT",
                "-p",
                "tcp",
                "--syn",
                "-m",
                "limit",
                "--limit",
                "5/sec",
                "--limit-burst",
                "10",
                "-j",
                "LOG",
                "--log-prefix",
                log_prefix,
                "--log-uid",
            ],
        ) {
            warn!(
                error = %e,
                "Failed to install LOG rule for TCP (xt_LOG module may not be loaded); \
                 bypass REJECT rules will still be installed"
            );
        }

        // Rule 5: REJECT TCP bypass attempts (fast-fail)
        self.run_iptables(
            iptables_cmd,
            &[
                "-A",
                "OUTPUT",
                "-p",
                "tcp",
                "-j",
                "REJECT",
                "--reject-with",
                "icmp-port-unreachable",
            ],
        )?;

        // Rule 6: LOG UDP bypass attempts (rate-limited, covers DNS bypass)
        if let Err(e) = self.run_iptables(
            iptables_cmd,
            &[
                "-A",
                "OUTPUT",
                "-p",
                "udp",
                "-m",
                "limit",
                "--limit",
                "5/sec",
                "--limit-burst",
                "10",
                "-j",
                "LOG",
                "--log-prefix",
                log_prefix,
                "--log-uid",
            ],
        ) {
            warn!(
                error = %e,
                "Failed to install LOG rule for UDP; bypass REJECT rules will still be installed"
            );
        }

        // Rule 7: REJECT UDP bypass attempts (covers DNS bypass)
        self.run_iptables(
            iptables_cmd,
            &[
                "-A",
                "OUTPUT",
                "-p",
                "udp",
                "-j",
                "REJECT",
                "--reject-with",
                "icmp-port-unreachable",
            ],
        )?;

        Ok(())
    }

    /// Install IPv6 bypass detection rules.
    ///
    /// Similar to `install_bypass_rules_for` but omits the proxy ACCEPT rule
    /// (the proxy listens on an IPv4 address) and uses IPv6-appropriate
    /// REJECT types.
    fn install_bypass_rules_for_v6(&self, ip6tables_cmd: &str, log_prefix: &str) -> Result<()> {
        // ACCEPT loopback traffic
        self.run_iptables(ip6tables_cmd, &["-A", "OUTPUT", "-o", "lo", "-j", "ACCEPT"])?;

        // ACCEPT established/related connections
        self.run_iptables(
            ip6tables_cmd,
            &[
                "-A",
                "OUTPUT",
                "-m",
                "conntrack",
                "--ctstate",
                "ESTABLISHED,RELATED",
                "-j",
                "ACCEPT",
            ],
        )?;

        // LOG TCP SYN bypass attempts (rate-limited)
        if let Err(e) = self.run_iptables(
            ip6tables_cmd,
            &[
                "-A",
                "OUTPUT",
                "-p",
                "tcp",
                "--syn",
                "-m",
                "limit",
                "--limit",
                "5/sec",
                "--limit-burst",
                "10",
                "-j",
                "LOG",
                "--log-prefix",
                log_prefix,
                "--log-uid",
            ],
        ) {
            warn!(error = %e, "Failed to install IPv6 LOG rule for TCP");
        }

        // REJECT TCP bypass attempts
        self.run_iptables(
            ip6tables_cmd,
            &[
                "-A",
                "OUTPUT",
                "-p",
                "tcp",
                "-j",
                "REJECT",
                "--reject-with",
                "icmp6-port-unreachable",
            ],
        )?;

        // LOG UDP bypass attempts (rate-limited)
        if let Err(e) = self.run_iptables(
            ip6tables_cmd,
            &[
                "-A",
                "OUTPUT",
                "-p",
                "udp",
                "-m",
                "limit",
                "--limit",
                "5/sec",
                "--limit-burst",
                "10",
                "-j",
                "LOG",
                "--log-prefix",
                log_prefix,
                "--log-uid",
            ],
        ) {
            warn!(error = %e, "Failed to install IPv6 LOG rule for UDP");
        }

        // REJECT UDP bypass attempts
        self.run_iptables(
            ip6tables_cmd,
            &[
                "-A",
                "OUTPUT",
                "-p",
                "udp",
                "-j",
                "REJECT",
                "--reject-with",
                "icmp6-port-unreachable",
            ],
        )?;

        Ok(())
    }
}

impl Drop for NetworkNamespace {
    fn drop(&mut self) {
        debug!(namespace = %self.name, "Cleaning up network namespace");

        // Close the fd if we have one
        if let Some(fd) = self.ns_fd.take() {
            let _ = nix::unistd::close(fd);
        }

        // Delete the host-side veth (this also removes the peer in the namespace)
        if let Err(e) = run_ip(&["link", "delete", &self.veth_host]) {
            warn!(
                error = %e,
                veth = %self.veth_host,
                "Failed to delete veth interface"
            );
        }

        match &mut self.mode {
            NamespaceMode::Named => {
                // Delete the named namespace (removes bind-mount at /var/run/netns/)
                if let Err(e) = run_ip(&["netns", "delete", &self.name]) {
                    warn!(
                        error = %e,
                        namespace = %self.name,
                        "Failed to delete network namespace"
                    );
                }
            }
            NamespaceMode::Unshare { holder } => {
                // Kill the holder process — the kernel auto-cleans the namespace
                // when all references (fds, processes) are gone.
                let _ = holder.kill();
                let _ = holder.wait();
            }
        }

        info!(namespace = %self.name, "Network namespace cleaned up");
    }
}

/// Run an `ip` command on the host.
fn run_ip(args: &[&str]) -> Result<()> {
    debug!(command = %format!("ip {}", args.join(" ")), "Running ip command");

    let output = Command::new("ip").args(args).output().into_diagnostic()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!(
            "ip {} failed: {}",
            args.join(" "),
            stderr.trim()
        ));
    }

    Ok(())
}

/// Run an `ip netns exec` command inside a namespace.
fn run_ip_netns(netns: &str, args: &[&str]) -> Result<()> {
    let mut full_args = vec!["netns", "exec", netns, "ip"];
    full_args.extend(args);

    debug!(command = %format!("ip {}", full_args.join(" ")), "Running ip netns exec command");

    let output = Command::new("ip")
        .args(&full_args)
        .output()
        .into_diagnostic()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!(
            "ip netns exec {} ip {} failed: {}",
            netns,
            args.join(" "),
            stderr.trim()
        ));
    }

    Ok(())
}

/// Run an iptables command inside a network namespace.
fn run_iptables_netns(netns: &str, iptables_cmd: &str, args: &[&str]) -> Result<()> {
    let mut full_args = vec!["netns", "exec", netns, iptables_cmd];
    full_args.extend(args);

    debug!(
        command = %format!("ip {}", full_args.join(" ")),
        "Running iptables in namespace"
    );

    let output = Command::new("ip")
        .args(&full_args)
        .output()
        .into_diagnostic()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!(
            "ip netns exec {} {} failed: {}",
            netns,
            iptables_cmd,
            stderr.trim()
        ));
    }

    Ok(())
}

/// Run a command inside a PID-based network namespace via `nsenter`.
///
/// Used by the `Unshare` namespace mode where there is no named namespace
/// at `/var/run/netns/` for `ip netns exec` to use.
fn run_in_ns(pid: u32, cmd: &str, args: &[&str]) -> Result<()> {
    let pid_str = pid.to_string();
    let mut full_args = vec!["-t", &pid_str, "-n", "--", cmd];
    full_args.extend(args);

    debug!(command = %format!("nsenter {}", full_args.join(" ")), "Running command via nsenter");

    let output = Command::new("nsenter")
        .args(&full_args)
        .output()
        .into_diagnostic()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!(
            "nsenter -t {} -n -- {} {} failed: {}",
            pid,
            cmd,
            args.join(" "),
            stderr.trim()
        ));
    }

    Ok(())
}

/// Run an `ip` command inside a PID-based namespace via `nsenter`.
fn run_ip_in_ns(pid: u32, args: &[&str]) -> Result<()> {
    run_in_ns(pid, "ip", args)
}

/// Run an iptables command inside a PID-based namespace via `nsenter`.
fn run_iptables_in_ns(pid: u32, iptables_cmd: &str, args: &[&str]) -> Result<()> {
    run_in_ns(pid, iptables_cmd, args)
}

/// Well-known paths where iptables may be installed.
/// The sandbox container PATH often excludes `/usr/sbin`, so we probe
/// explicit paths rather than relying on `which`.
const IPTABLES_SEARCH_PATHS: &[&str] =
    &["/usr/sbin/iptables", "/sbin/iptables", "/usr/bin/iptables"];

/// Returns true if xt extension modules (e.g. xt_comment) cannot be used
/// via the given iptables binary.
///
/// Some kernels have nf_tables but lack the nft_compat bridge that allows
/// xt extension modules to be used through the nf_tables path (e.g. Jetson
/// Linux 5.15-tegra). This probe detects that condition by attempting to
/// insert a rule using the xt_comment extension. If it fails, xt extensions
/// are unavailable and the caller should fall back to iptables-legacy.
fn xt_extensions_unavailable(iptables_path: &str) -> bool {
    // Create a temporary probe chain. If this fails (e.g. no CAP_NET_ADMIN),
    // we can't determine availability — assume extensions are available.
    let created = Command::new(iptables_path)
        .args(["-t", "filter", "-N", "_xt_probe"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !created {
        return false;
    }

    // Attempt to insert a rule using xt_comment. Failure means nft_compat
    // cannot bridge xt extension modules on this kernel.
    let probe_ok = Command::new(iptables_path)
        .args([
            "-t",
            "filter",
            "-A",
            "_xt_probe",
            "-m",
            "comment",
            "--comment",
            "probe",
            "-j",
            "ACCEPT",
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    // Clean up — best-effort, ignore failures.
    let _ = Command::new(iptables_path)
        .args([
            "-t",
            "filter",
            "-D",
            "_xt_probe",
            "-m",
            "comment",
            "--comment",
            "probe",
            "-j",
            "ACCEPT",
        ])
        .output();
    let _ = Command::new(iptables_path)
        .args(["-t", "filter", "-X", "_xt_probe"])
        .output();

    !probe_ok
}

/// Find the iptables binary path, checking well-known locations.
///
/// If xt extension modules are unavailable via the standard binary and
/// `iptables-legacy` is available alongside it, the legacy binary is returned
/// instead. This ensures bypass-detection rules can be installed on kernels
/// where `nft_compat` is unavailable (e.g. Jetson Linux 5.15-tegra).
fn find_iptables() -> Option<String> {
    let standard_path = IPTABLES_SEARCH_PATHS
        .iter()
        .find(|path| std::path::Path::new(path).exists())
        .copied()?;

    if xt_extensions_unavailable(standard_path) {
        let legacy_path = standard_path.replace("iptables", "iptables-legacy");
        if std::path::Path::new(&legacy_path).exists() {
            debug!(
                legacy = legacy_path,
                "xt extensions unavailable; using iptables-legacy"
            );
            return Some(legacy_path);
        }
    }

    Some(standard_path.to_string())
}

/// Find the ip6tables binary path, deriving it from the iptables location.
fn find_ip6tables(iptables_path: &str) -> Option<String> {
    let ip6_path = iptables_path.replace("iptables", "ip6tables");
    if std::path::Path::new(&ip6_path).exists() {
        Some(ip6_path)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests require root and network namespace support
    // Run with: sudo cargo test -- --ignored

    #[test]
    #[ignore = "requires root privileges"]
    fn test_create_named_namespace() {
        let ns = NetworkNamespace::create_named().expect("Failed to create named namespace");
        let name = ns.name().to_string();

        // Verify named namespace exists at /var/run/netns/
        let ns_path = format!("/var/run/netns/{name}");
        assert!(
            std::path::Path::new(&ns_path).exists(),
            "Namespace bind-mount should exist"
        );

        assert!(matches!(ns.mode, NamespaceMode::Named));
        assert!(ns.ns_fd().is_some());

        assert_eq!(
            ns.host_ip().to_string(),
            format!("{SUBNET_PREFIX}.{HOST_IP_SUFFIX}")
        );
        assert_eq!(
            ns.sandbox_ip().to_string(),
            format!("{SUBNET_PREFIX}.{SANDBOX_IP_SUFFIX}")
        );

        drop(ns);

        assert!(
            !std::path::Path::new(&ns_path).exists(),
            "Namespace should be cleaned up"
        );
    }

    #[test]
    #[ignore = "requires root privileges"]
    fn test_create_unshare_namespace() {
        let ns =
            NetworkNamespace::create_via_unshare().expect("Failed to create unshare namespace");

        assert!(matches!(ns.mode, NamespaceMode::Unshare { .. }));
        assert!(ns.ns_fd().is_some());

        // Verify the holder process is alive
        if let NamespaceMode::Unshare { ref holder } = ns.mode {
            let holder_pid = holder.id();
            let proc_path = format!("/proc/{holder_pid}/ns/net");
            assert!(
                std::path::Path::new(&proc_path).exists(),
                "Holder process namespace should be accessible"
            );
        }

        assert_eq!(
            ns.host_ip().to_string(),
            format!("{SUBNET_PREFIX}.{HOST_IP_SUFFIX}")
        );
        assert_eq!(
            ns.sandbox_ip().to_string(),
            format!("{SUBNET_PREFIX}.{SANDBOX_IP_SUFFIX}")
        );

        // No /var/run/netns/ entry should exist
        let name = ns.name().to_string();
        let named_path = format!("/var/run/netns/{name}");
        assert!(
            !std::path::Path::new(&named_path).exists(),
            "Unshare mode should not create a named namespace"
        );

        drop(ns);
    }

    #[test]
    #[ignore = "requires root privileges"]
    fn test_create_auto_selects_mode() {
        // create() should succeed using whichever mode works
        let ns = NetworkNamespace::create().expect("Failed to create namespace (auto mode)");
        assert!(ns.ns_fd().is_some());
        drop(ns);
    }
}
