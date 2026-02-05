//! Navigator CLI - command-line interface for Navigator.

use clap::{CommandFactory, Parser, Subcommand};
use miette::Result;

use navigator_cli::run;

/// Navigator CLI - agent execution and management.
#[derive(Parser, Debug)]
#[command(name = "navigator")]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
struct Cli {
    /// Increase verbosity (-v, -vv, -vvv).
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    /// Cluster address to connect to.
    #[arg(
        long,
        short,
        default_value = "http://127.0.0.1:8080",
        global = true,
        env = "NAVIGATOR_CLUSTER"
    )]
    cluster: String,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Manage cluster.
    Cluster {
        #[command(subcommand)]
        command: ClusterCommands,
    },

    /// Manage sandboxes.
    Sandbox {
        #[command(subcommand)]
        command: SandboxCommands,
    },

    /// SSH proxy (used by `ProxyCommand`).
    SshProxy {
        /// Gateway URL (e.g., https://gw.example.com:443/proxy/connect).
        #[arg(long)]
        gateway: String,

        /// Sandbox id.
        #[arg(long)]
        sandbox_id: String,

        /// SSH session token.
        #[arg(long)]
        token: String,
    },
}

#[derive(Subcommand, Debug)]
enum ClusterCommands {
    /// Show server status and information.
    Status,

    /// Manage local development cluster lifecycle.
    Admin {
        #[command(subcommand)]
        command: ClusterAdminCommands,
    },
}

#[derive(Subcommand, Debug)]
enum ClusterAdminCommands {
    /// Provision or start a local cluster.
    Deploy {
        /// Cluster name.
        #[arg(long, default_value = "navigator")]
        name: String,

        /// Write stored kubeconfig into local kubeconfig.
        #[arg(long)]
        update_kube_config: bool,

        /// Print stored kubeconfig to stdout.
        #[arg(long)]
        get_kubeconfig: bool,
    },

    /// Stop a local cluster (preserves state).
    Stop {
        /// Cluster name.
        #[arg(long, default_value = "navigator")]
        name: String,
    },

    /// Destroy a local cluster and its state.
    Destroy {
        /// Cluster name.
        #[arg(long, default_value = "navigator")]
        name: String,
    },
}

#[derive(Subcommand, Debug)]
enum SandboxCommands {
    /// Create a sandbox.
    Create,

    /// Fetch a sandbox by id.
    Get {
        /// Sandbox id.
        id: String,
    },

    /// List sandboxes.
    List {
        /// Maximum number of sandboxes to return.
        #[arg(long, default_value_t = 100)]
        limit: u32,

        /// Offset into the sandbox list.
        #[arg(long, default_value_t = 0)]
        offset: u32,

        /// Print only sandbox ids (one per line).
        #[arg(long)]
        ids: bool,
    },

    /// Delete a sandbox by id.
    Delete {
        /// Sandbox ids.
        #[arg(required = true, num_args = 1.., value_name = "ID")]
        ids: Vec<String>,
    },

    /// Connect to a sandbox.
    Connect {
        /// Sandbox id.
        id: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Set up logging based on verbosity
    let log_level = match cli.verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level)),
        )
        .init();

    match cli.command {
        Some(Commands::Cluster { command }) => match command {
            ClusterCommands::Status => {
                run::cluster_status(&cli.cluster).await?;
            }
            ClusterCommands::Admin { command } => match command {
                ClusterAdminCommands::Deploy {
                    name,
                    update_kube_config,
                    get_kubeconfig,
                } => {
                    run::cluster_admin_deploy(&name, update_kube_config, get_kubeconfig).await?;
                }
                ClusterAdminCommands::Stop { name } => {
                    run::cluster_admin_stop(&name).await?;
                }
                ClusterAdminCommands::Destroy { name } => {
                    run::cluster_admin_destroy(&name).await?;
                }
            },
        },
        Some(Commands::Sandbox { command }) => match command {
            SandboxCommands::Create => {
                run::sandbox_create(&cli.cluster).await?;
            }
            SandboxCommands::Get { id } => {
                run::sandbox_get(&cli.cluster, &id).await?;
            }
            SandboxCommands::List { limit, offset, ids } => {
                run::sandbox_list(&cli.cluster, limit, offset, ids).await?;
            }
            SandboxCommands::Delete { ids } => {
                run::sandbox_delete(&cli.cluster, &ids).await?;
            }
            SandboxCommands::Connect { id } => {
                run::sandbox_connect(&cli.cluster, &id).await?;
            }
        },
        Some(Commands::SshProxy {
            gateway,
            sandbox_id,
            token,
        }) => {
            run::sandbox_ssh_proxy(&gateway, &sandbox_id, &token).await?;
        }
        None => {
            Cli::command().print_help().expect("Failed to print help");
        }
    }

    Ok(())
}
