#!/usr/bin/env bash

# SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
# SPDX-License-Identifier: Apache-2.0

# Automated setup script for Podman on macOS
# Handles Podman machine initialization, cgroup delegation, and environment setup

set -euo pipefail

MACHINE_NAME="${PODMAN_MACHINE_NAME:-openshell}"
MEMORY="${PODMAN_MEMORY:-8192}"
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

# Stop any other running machines
echo ""
echo "Stopping other Podman machines (only one can run at a time)..."
for machine in $(podman machine list --format '{{.Name}}' --noheading); do
    if [[ "$machine" != "${MACHINE_NAME}" ]]; then
        if podman machine list --format '{{.Name}} {{.Running}}' | grep "^${machine} " | grep -q "true"; then
            echo "  Stopping ${machine}..."
            podman machine stop "${machine}" 2>/dev/null || true
        fi
    fi
done

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
echo "Environment variables (add to your shell profile):"
echo "  export CONTAINER_HOST=\"unix://${SOCKET_PATH}\""
echo "  export OPENSHELL_CONTAINER_RUNTIME=podman"
echo ""
echo "To set them now, run:"
echo "  export CONTAINER_HOST=\"unix://${SOCKET_PATH}\""
echo "  export OPENSHELL_CONTAINER_RUNTIME=podman"
echo ""

# Offer to add to shell profile
read -p "Add environment variables to ~/.zshrc? (y/N) " -n 1 -r
echo
if [[ $REPLY =~ ^[Yy]$ ]]; then
    if ! grep -q "CONTAINER_HOST.*${MACHINE_NAME}" ~/.zshrc 2>/dev/null; then
        echo "" >> ~/.zshrc
        echo "# OpenShell Podman environment" >> ~/.zshrc
        echo "export CONTAINER_HOST=\"unix://${SOCKET_PATH}\"" >> ~/.zshrc
        echo "export OPENSHELL_CONTAINER_RUNTIME=podman" >> ~/.zshrc
        echo "✓ Added to ~/.zshrc"
        echo "  Run: source ~/.zshrc"
    else
        echo "⚠️  Environment variables already in ~/.zshrc"
    fi
fi

echo ""
echo "Next steps:"
echo "  1. Source your shell profile or set environment variables"
echo "  2. Build cluster image: mise run docker:build:cluster"
echo "  3. Build CLI: cargo build --release -p openshell-cli"
echo "  4. Install CLI: cp target/release/openshell ~/.local/bin/"
