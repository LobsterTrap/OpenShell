// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Container runtime preflight e2e tests.
//!
//! These tests verify that the CLI fails fast with actionable guidance when
//! the container runtime (Docker or Podman) is not available, instead of
//! starting a multi-minute deploy that eventually times out with a cryptic
//! error.
//!
//! The tests do NOT require a running gateway — they intentionally point the
//! runtime's socket env var at a non-existent path to simulate the runtime
//! being unavailable.

use std::process::Stdio;
use std::time::Instant;

use openshell_e2e::harness::binary::openshell_cmd;
use openshell_e2e::harness::output::strip_ansi;
use openshell_e2e::harness::runtime::container_runtime_binary;

/// Expected strings for the detected container runtime.
struct RuntimeExpectations {
    /// Display name: `"Docker"` or `"Podman"`.
    display_name: &'static str,
    /// Primary host env var: `"DOCKER_HOST"` or `"CONTAINER_HOST"`.
    host_env_var: &'static str,
    /// Verification command: `"docker info"` or `"podman info"`.
    verify_cmd: &'static str,
}

/// Return runtime-specific expected strings based on the detected runtime.
fn runtime_expectations() -> RuntimeExpectations {
    match container_runtime_binary() {
        "podman" => RuntimeExpectations {
            display_name: "Podman",
            host_env_var: "CONTAINER_HOST",
            verify_cmd: "podman info",
        },
        _ => RuntimeExpectations {
            display_name: "Docker",
            host_env_var: "DOCKER_HOST",
            verify_cmd: "docker info",
        },
    }
}

/// Run `openshell <args>` in an isolated environment where the container
/// runtime is guaranteed to be unreachable.
///
/// Forces `OPENSHELL_CONTAINER_RUNTIME` so detection is deterministic, then
/// points the runtime's socket env var at a non-existent path. Also sets
/// `XDG_RUNTIME_DIR` to the empty tmpdir so rootless Podman socket probing
/// finds nothing.
async fn run_without_runtime(args: &[&str]) -> (String, i32, std::time::Duration) {
    let tmpdir = tempfile::tempdir().expect("create isolated config dir");
    let runtime = container_runtime_binary();
    let start = Instant::now();

    let mut cmd = openshell_cmd();
    cmd.args(args)
        .env("XDG_CONFIG_HOME", tmpdir.path())
        .env("HOME", tmpdir.path())
        .env("XDG_RUNTIME_DIR", tmpdir.path())
        .env("OPENSHELL_CONTAINER_RUNTIME", runtime)
        .env(
            "DOCKER_HOST",
            "unix:///tmp/openshell-e2e-nonexistent.sock",
        )
        .env(
            "CONTAINER_HOST",
            "unix:///tmp/openshell-e2e-nonexistent.sock",
        )
        .env_remove("OPENSHELL_GATEWAY")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = cmd.output().await.expect("spawn openshell");
    let elapsed = start.elapsed();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let combined = format!("{stdout}{stderr}");
    let code = output.status.code().unwrap_or(-1);
    (combined, code, elapsed)
}

// -------------------------------------------------------------------
// gateway start: fails fast when the container runtime is unavailable
// -------------------------------------------------------------------

/// `openshell gateway start` with no runtime should fail within seconds
/// (not minutes) and produce a non-zero exit code.
#[tokio::test]
async fn gateway_start_fails_fast_without_runtime() {
    let (output, code, elapsed) = run_without_runtime(&["gateway", "start"]).await;

    assert_ne!(
        code, 0,
        "gateway start should fail when the runtime is unavailable, output:\n{output}"
    );

    // The preflight check should cause failure in under 30 seconds.
    // Before the preflight was added, this would time out after several minutes
    // waiting for k3s namespace readiness.
    assert!(
        elapsed.as_secs() < 30,
        "gateway start should fail fast (took {}s), output:\n{output}",
        elapsed.as_secs()
    );
}

/// When the container runtime is unavailable, the error output should
/// mention the runtime name so the user knows what to fix.
#[tokio::test]
async fn gateway_start_error_mentions_runtime() {
    let rt = runtime_expectations();
    let (output, code, _) = run_without_runtime(&["gateway", "start"]).await;

    assert_ne!(code, 0);
    let clean = strip_ansi(&output);
    let lower = clean.to_lowercase();

    assert!(
        lower.contains(&rt.display_name.to_lowercase()),
        "error output should mention '{}' so the user knows what to fix:\n{clean}",
        rt.display_name
    );
}

/// When the container runtime is unavailable, the error output should
/// include guidance about the host env var (DOCKER_HOST or CONTAINER_HOST).
#[tokio::test]
async fn gateway_start_error_mentions_host_env() {
    let rt = runtime_expectations();
    let (output, code, _) = run_without_runtime(&["gateway", "start"]).await;

    assert_ne!(code, 0);
    let clean = strip_ansi(&output);

    assert!(
        clean.contains(rt.host_env_var),
        "error output should mention {} for users with non-default socket paths:\n{clean}",
        rt.host_env_var
    );
}

/// When the container runtime is unavailable, the error output should
/// suggest a verification command like `docker info` or `podman info`.
#[tokio::test]
async fn gateway_start_error_suggests_verification() {
    let rt = runtime_expectations();
    let (output, code, _) = run_without_runtime(&["gateway", "start"]).await;

    assert_ne!(code, 0);
    let clean = strip_ansi(&output);

    assert!(
        clean.contains(rt.verify_cmd),
        "error output should suggest '{}' as a verification step:\n{clean}",
        rt.verify_cmd
    );
}

// -------------------------------------------------------------------
// gateway start --recreate: same preflight behavior
// -------------------------------------------------------------------

/// `openshell gateway start --recreate` should also fail fast when the
/// container runtime is unavailable (the recreate flag should not bypass
/// the check).
#[tokio::test]
async fn gateway_start_recreate_fails_fast_without_runtime() {
    let (output, code, elapsed) =
        run_without_runtime(&["gateway", "start", "--recreate"]).await;

    assert_ne!(
        code, 0,
        "gateway start --recreate should fail when the runtime is unavailable, output:\n{output}"
    );

    assert!(
        elapsed.as_secs() < 30,
        "gateway start --recreate should fail fast (took {}s)",
        elapsed.as_secs()
    );
}

// -------------------------------------------------------------------
// sandbox create with auto-bootstrap: same preflight behavior
// -------------------------------------------------------------------

/// `openshell sandbox create` triggers auto-bootstrap when no gateway
/// exists. With the runtime unavailable, it should fail fast with
/// actionable guidance rather than timing out.
#[tokio::test]
async fn sandbox_create_auto_bootstrap_fails_fast_without_runtime() {
    let rt = runtime_expectations();
    let (output, code, elapsed) =
        run_without_runtime(&["sandbox", "create", "--from", "openclaw"]).await;

    assert_ne!(
        code, 0,
        "sandbox create should fail when the runtime is unavailable, output:\n{output}"
    );

    // Auto-bootstrap path should also hit the preflight check quickly.
    assert!(
        elapsed.as_secs() < 30,
        "sandbox create should fail fast via auto-bootstrap preflight (took {}s), output:\n{output}",
        elapsed.as_secs()
    );

    let clean = strip_ansi(&output);
    let lower = clean.to_lowercase();
    assert!(
        lower.contains(&rt.display_name.to_lowercase()),
        "sandbox create error should mention {}:\n{clean}",
        rt.display_name
    );
}

// -------------------------------------------------------------------
// doctor check: validates system prerequisites
// -------------------------------------------------------------------

/// `openshell doctor check` with the runtime unavailable should fail fast
/// and report the runtime check as FAILED.
#[tokio::test]
async fn doctor_check_fails_without_runtime() {
    let (output, code, elapsed) = run_without_runtime(&["doctor", "check"]).await;

    assert_ne!(
        code, 0,
        "doctor check should fail when the runtime is unavailable, output:\n{output}"
    );

    assert!(
        elapsed.as_secs() < 10,
        "doctor check should complete quickly (took {}s)",
        elapsed.as_secs()
    );

    let clean = strip_ansi(&output);
    assert!(
        clean.contains("FAILED"),
        "doctor check should report the runtime as FAILED:\n{clean}"
    );
}

/// `openshell doctor check` output should include the runtime label
/// so the user knows what was tested.
#[tokio::test]
async fn doctor_check_output_shows_runtime_label() {
    let rt = runtime_expectations();
    let (output, _, _) = run_without_runtime(&["doctor", "check"]).await;
    let clean = strip_ansi(&output);

    assert!(
        clean.contains(rt.display_name),
        "doctor check output should include '{}' label:\n{clean}",
        rt.display_name
    );
}

/// `openshell doctor check` with the runtime unavailable should include
/// actionable guidance in the error output.
#[tokio::test]
async fn doctor_check_error_includes_guidance() {
    let rt = runtime_expectations();
    let (output, code, _) = run_without_runtime(&["doctor", "check"]).await;

    assert_ne!(code, 0);
    let clean = strip_ansi(&output);

    assert!(
        clean.contains(rt.host_env_var),
        "doctor check error should mention {}:\n{clean}",
        rt.host_env_var
    );
    assert!(
        clean.contains(rt.verify_cmd),
        "doctor check error should suggest '{}':\n{clean}",
        rt.verify_cmd
    );
}

/// When the container runtime IS available, `openshell doctor check`
/// should pass and report the version.
///
/// This test only runs when the runtime is reachable on the host. It
/// checks for both Docker and Podman sockets, skipping if neither exists.
#[tokio::test]
async fn doctor_check_passes_with_runtime() {
    let has_docker = std::path::Path::new("/var/run/docker.sock").exists();
    let has_podman_rootless = std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .map(|dir| std::path::PathBuf::from(dir).join("podman/podman.sock"))
        .is_some_and(|p| p.exists());
    let has_podman_rootful = std::path::Path::new("/run/podman/podman.sock").exists()
        || std::path::Path::new("/var/run/podman/podman.sock").exists();

    if !has_docker && !has_podman_rootless && !has_podman_rootful {
        eprintln!("skipping: no container runtime socket found");
        return;
    }

    let tmpdir = tempfile::tempdir().expect("create isolated config dir");
    let mut cmd = openshell_cmd();
    cmd.args(["doctor", "check"])
        .env("XDG_CONFIG_HOME", tmpdir.path())
        .env("HOME", tmpdir.path())
        .env_remove("OPENSHELL_GATEWAY")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = cmd.output().await.expect("spawn openshell");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let combined = format!("{stdout}{stderr}");
    let code = output.status.code().unwrap_or(-1);
    let clean = strip_ansi(&combined);

    assert_eq!(
        code, 0,
        "doctor check should pass when the runtime is available, output:\n{clean}"
    );
    assert!(
        clean.contains("All checks passed"),
        "doctor check should report success:\n{clean}"
    );
    assert!(
        clean.contains("ok"),
        "doctor check should show 'ok' for the runtime:\n{clean}"
    );
}
