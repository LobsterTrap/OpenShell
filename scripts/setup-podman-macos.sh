#!/usr/bin/env bash

# SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
# SPDX-License-Identifier: Apache-2.0

# Automated setup script for Podman on macOS
# Handles Podman machine initialization, cgroup delegation, and environment setup

set -euo pipefail

MACHINE_NAME="${PODMAN_MACHINE_NAME:-openshell}"
MEMORY="${PODMAN_MEMORY:-12288}"
CPUS="${PODMAN_CPUS:-4}"

echo "=== OpenShell Podman Setup for macOS ==="
echo ""

# Check if Podman is installed
if ! command -v podman &>/dev/null; then
    echo "❌ Podman is not installed."
    echo ""
    echo "Install with: brew install podman"
    exit 1
fi

echo "✓ Podman is installed ($(podman --version))"

# Check if machine already exists
if podman machine list --format '{{.Name}}' | grep -q "^${MACHINE_NAME}$"; then
    echo ""
    echo "⚠️  Podman machine '${MACHINE_NAME}' already exists."
    read -p "Do you want to recreate it? This will delete the existing machine. (y/N) " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        echo "Stopping and removing existing machine..."
        podman machine stop "${MACHINE_NAME}" 2>/dev/null || true
        podman machine rm -f "${MACHINE_NAME}"
    else
        echo "Using existing machine. Verifying configuration..."
        EXISTING=true
    fi
fi

# Initialize machine if needed
if [[ "${EXISTING:-false}" != "true" ]]; then
    echo ""
    echo "Initializing Podman machine '${MACHINE_NAME}' with ${MEMORY}MB RAM and ${CPUS} CPUs..."
    podman machine init "${MACHINE_NAME}" --memory "${MEMORY}" --cpus "${CPUS}"
fi

# Check for other running machines
RUNNING_MACHINES=$(podman machine list --format '{{.Name}} {{.Running}}' | grep -v "^${MACHINE_NAME} " | grep "true" | awk '{print $1}' || true)

if [ -n "${RUNNING_MACHINES}" ]; then
    echo ""
    echo "⚠️  Other Podman machines are currently running:"
    echo "${RUNNING_MACHINES}" | while read -r machine; do
        echo "  - ${machine}"
    done
    echo ""
    echo "Only one Podman machine can run at a time."
    read -p "Stop these machines to continue? (y/N) " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        echo "Stopping other Podman machines..."
        echo "${RUNNING_MACHINES}" | while read -r machine; do
            echo "  Stopping ${machine}..."
            podman machine stop "${machine}" 2>/dev/null || true
        done
    else
        echo "Cannot start '${MACHINE_NAME}' while other machines are running."
        echo "Please stop them manually and run this script again."
        exit 1
    fi
fi

# Start the machine
echo ""
echo "Starting Podman machine '${MACHINE_NAME}'..."
if ! podman machine list --format '{{.Name}} {{.Running}}' | grep "^${MACHINE_NAME} " | grep -q "true"; then
    podman machine start "${MACHINE_NAME}"
fi

# Set as default
echo "Setting '${MACHINE_NAME}' as default connection..."
podman system connection default "${MACHINE_NAME}"

# Configure cgroup delegation (CRITICAL for rootless k3s)
echo ""
echo "Configuring cgroup delegation for rootless k3s..."
podman machine ssh "${MACHINE_NAME}" 'echo "[Service]
Delegate=cpu cpuset io memory pids" | sudo tee /etc/systemd/system/user@.service.d/delegate.conf' >/dev/null

podman machine ssh "${MACHINE_NAME}" "sudo systemctl daemon-reload"

# Restart for cgroup changes to take effect
echo "Restarting machine for cgroup changes..."
podman machine stop "${MACHINE_NAME}"
podman machine start "${MACHINE_NAME}"

# Get socket path
SOCKET_PATH=$(podman machine inspect "${MACHINE_NAME}" --format '{{.ConnectionInfo.PodmanSocket.Path}}')

echo ""
echo "=== Setup Complete ==="
echo ""
echo "Podman machine '${MACHINE_NAME}' is ready!"
echo ""
echo "Next steps:"
echo "  1. Set up environment: source scripts/podman.env"
echo "  2. Build and deploy cluster: mise run cluster:build:full"
echo "  3. Install CLI: cargo install --path crates/openshell-cli --root ~/.local"
echo "  4. Verify installation: openshell gateway info"
echo ""
echo "To make the environment persistent, add to your shell profile (~/.zshrc):"
echo "  source $(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/scripts/podman.env"
