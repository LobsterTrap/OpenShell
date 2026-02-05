//! SSH connection and proxy utilities.

use miette::{IntoDiagnostic, Result, WrapErr};
use navigator_core::proto::{CreateSshSessionRequest, navigator_client::NavigatorClient};
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use std::io::IsTerminal;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use webpki_roots::TLS_SERVER_ROOTS;

/// Connect to a sandbox via SSH.
pub async fn sandbox_connect(server: &str, id: &str) -> Result<()> {
    let mut client = NavigatorClient::connect(server.to_string())
        .await
        .into_diagnostic()?;
    let response = client
        .create_ssh_session(CreateSshSessionRequest {
            sandbox_id: id.to_string(),
        })
        .await
        .into_diagnostic()?;
    let session = response.into_inner();

    let exe = std::env::current_exe()
        .into_diagnostic()
        .wrap_err("failed to resolve navigator executable")?;
    let exe_command = shell_escape(&exe.to_string_lossy());

    let gateway_url = format!(
        "{}://{}:{}{}",
        session.gateway_scheme, session.gateway_host, session.gateway_port, session.connect_path
    );
    let proxy_command = format!(
        "{exe_command} ssh-proxy --gateway {} --sandbox-id {} --token {}",
        gateway_url, session.sandbox_id, session.token,
    );

    let mut command = std::process::Command::new("ssh");
    command
        .arg("-tt")
        .arg("-o")
        .arg(format!("ProxyCommand={proxy_command}"))
        .arg("-o")
        .arg("StrictHostKeyChecking=no")
        .arg("-o")
        .arg("UserKnownHostsFile=/dev/null")
        .arg("-o")
        .arg("GlobalKnownHostsFile=/dev/null")
        .arg("-o")
        .arg("RequestTTY=force")
        .arg("-o")
        .arg("SetEnv=TERM=xterm-256color")
        .arg("sandbox")
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());

    if std::io::stdin().is_terminal() {
        #[cfg(unix)]
        {
            let err = command.exec();
            return Err(miette::miette!("failed to exec ssh: {err}"));
        }
    }

    let status = tokio::task::spawn_blocking(move || command.status())
        .await
        .into_diagnostic()?
        .into_diagnostic()?;

    if !status.success() {
        return Err(miette::miette!("ssh exited with status {status}"));
    }

    Ok(())
}

fn shell_escape(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }

    let safe = value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'/' | b'-' | b'_'));
    if safe {
        return value.to_string();
    }

    let escaped = value.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

/// Run the SSH proxy, connecting stdin/stdout to the gateway.
pub async fn sandbox_ssh_proxy(gateway_url: &str, sandbox_id: &str, token: &str) -> Result<()> {
    let url: url::Url = gateway_url
        .parse()
        .into_diagnostic()
        .wrap_err("invalid gateway URL")?;

    let scheme = url.scheme();
    let gateway_host = url
        .host_str()
        .ok_or_else(|| miette::miette!("gateway URL missing host"))?;
    let gateway_port = url
        .port_or_known_default()
        .ok_or_else(|| miette::miette!("gateway URL missing port"))?;
    let connect_path = url.path();

    let mut stream: Box<dyn ProxyStream> =
        connect_gateway(scheme, gateway_host, gateway_port).await?;

    let request = format!(
        "CONNECT {connect_path} HTTP/1.1\r\nHost: {gateway_host}\r\nX-Sandbox-Id: {sandbox_id}\r\nX-Sandbox-Token: {token}\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .into_diagnostic()?;

    let status = read_connect_status(&mut stream).await?;
    if status != 200 {
        return Err(miette::miette!(
            "gateway CONNECT failed with status {status}"
        ));
    }

    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();

    tokio::try_join!(
        tokio::io::copy(&mut stdin, &mut writer),
        tokio::io::copy(&mut reader, &mut stdout)
    )
    .into_diagnostic()?;

    Ok(())
}

async fn connect_gateway(scheme: &str, host: &str, port: u16) -> Result<Box<dyn ProxyStream>> {
    let tcp = TcpStream::connect((host, port)).await.into_diagnostic()?;
    if scheme.eq_ignore_ascii_case("https") {
        let mut root_store = RootCertStore::empty();
        root_store.extend(TLS_SERVER_ROOTS.iter().cloned());
        let config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(config));
        let server_name = ServerName::try_from(host.to_string())
            .map_err(|_| miette::miette!("invalid server name: {host}"))?;
        let tls = connector
            .connect(server_name, tcp)
            .await
            .into_diagnostic()?;
        Ok(Box::new(tls))
    } else {
        Ok(Box::new(tcp))
    }
}

async fn read_connect_status(stream: &mut dyn ProxyStream) -> Result<u16> {
    let mut buf = Vec::new();
    let mut temp = [0u8; 1024];
    loop {
        let n = stream.read(&mut temp).await.into_diagnostic()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&temp[..n]);
        if buf.windows(4).any(|win| win == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 8192 {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let line = text.lines().next().unwrap_or("");
    let status = line
        .split_whitespace()
        .nth(1)
        .unwrap_or("0")
        .parse::<u16>()
        .unwrap_or(0);
    Ok(status)
}

trait ProxyStream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T> ProxyStream for T where T: AsyncRead + AsyncWrite + Unpin + Send {}
