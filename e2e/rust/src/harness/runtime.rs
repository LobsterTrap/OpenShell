// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Container runtime detection for E2E tests.

/// Return the container runtime binary name ("podman" or "docker").
///
/// Reads `OPENSHELL_CONTAINER_RUNTIME` once and caches the result.
pub fn container_runtime_binary() -> &'static str {
    static RUNTIME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        std::env::var("OPENSHELL_CONTAINER_RUNTIME").unwrap_or_else(|_| {
            // Match bootstrap detection: prefer podman when available
            if std::process::Command::new("podman")
                .arg("--version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
            {
                "podman".to_string()
            } else {
                "docker".to_string()
            }
        })
    })
}
