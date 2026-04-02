#!/usr/bin/env bash

# SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
# SPDX-License-Identifier: Apache-2.0

# Cleanup script for OpenShell Podman installation on macOS
# Removes all OpenShell containers, images, binaries, and configuration

set -e

echo "=== OpenShell Podman Cleanup Script ==="
echo ""

# Destroy OpenShell gateway (if it exists)
echo "Destroying OpenShell gateway..."
if command -v openshell &>/dev/null; then
    openshell gateway destroy --name openshell 2>/dev/null || true
fi

# Stop and remove any running OpenShell containers
echo "Stopping OpenShell containers..."
podman ps -a | grep openshell | awk '{print $1}' | xargs -r podman rm -f || true

# Remove OpenShell images
echo "Removing OpenShell images..."
podman images | grep -E "openshell|cluster" | awk '{print $3}' | xargs -r podman rmi -f || true

# Remove CLI binary
echo "Removing CLI binary..."
rm -f ~/.local/bin/openshell
if [ -f /usr/local/bin/openshell ]; then
    echo "Removing /usr/local/bin/openshell (requires sudo)..."
    sudo rm -f /usr/local/bin/openshell
fi

# Remove OpenShell configuration and data
echo "Removing OpenShell configuration..."
rm -rf ~/.openshell

# Remove build artifacts (from OpenShell directory)
echo "Removing build artifacts..."
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"
rm -rf target/
rm -rf deploy/docker/.build/

# Clean Podman cache
echo "Cleaning Podman build cache..."
podman system prune -af --volumes

echo ""
echo "=== Cleanup Complete ==="
echo ""
echo "To completely remove the OpenShell Podman machine:"
echo "  podman machine stop openshell"
echo "  podman machine rm openshell"
echo ""
read -p "Do you want to remove the OpenShell Podman machine now? (y/N) " -n 1 -r
echo
if [[ $REPLY =~ ^[Yy]$ ]]; then
    echo "Stopping and removing OpenShell Podman machine..."
    podman machine stop openshell 2>/dev/null || true
    podman machine rm -f openshell 2>/dev/null || true
    echo "OpenShell Podman machine removed."
else
    echo "Skipping Podman machine removal."
    echo "The machine is still available for future use."
fi
