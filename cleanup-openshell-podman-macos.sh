#!/usr/bin/env bash

# SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
# SPDX-License-Identifier: Apache-2.0

# Cleanup script for OpenShell Podman installation on macOS
# Removes all OpenShell containers, images, binaries, and configuration

set -e

echo "=== OpenShell Podman Cleanup Script ==="
echo ""

# Delete all sandboxes first (before destroying gateway)
echo "Deleting all sandboxes..."
if command -v openshell &>/dev/null; then
    # Get list of sandboxes and delete each one
    openshell sandbox list --no-header 2>/dev/null | awk '{print $1}' | while read -r sandbox; do
        if [ -n "$sandbox" ]; then
            echo "  Deleting sandbox: $sandbox"
            openshell sandbox delete "$sandbox" 2>/dev/null || true
        fi
    done
fi

# Destroy OpenShell gateway (if it exists)
echo "Destroying OpenShell gateway..."
if command -v openshell &>/dev/null; then
    openshell gateway destroy --name openshell 2>/dev/null || true
fi

# Stop and remove cluster container
echo "Stopping cluster container..."
podman stop openshell-cluster-openshell 2>/dev/null || true
podman rm openshell-cluster-openshell 2>/dev/null || true

# Stop and remove local registry container
echo "Stopping local registry..."
podman stop openshell-local-registry 2>/dev/null || true
podman rm openshell-local-registry 2>/dev/null || true

# Stop and remove any other OpenShell containers
echo "Cleaning up remaining OpenShell containers..."
podman ps -a | grep openshell | awk '{print $1}' | xargs -r podman rm -f 2>/dev/null || true

# Remove OpenShell images
echo "Removing OpenShell images..."
podman rmi localhost/openshell/cluster:dev 2>/dev/null || true
podman rmi localhost/openshell/gateway:dev 2>/dev/null || true
podman images | grep -E "openshell|127.0.0.1:5000/openshell" | awk '{print $3}' | xargs -r podman rmi -f 2>/dev/null || true

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
if command -v cargo &>/dev/null; then
    echo "  Running cargo clean..."
    cargo clean 2>/dev/null || true
fi
rm -rf deploy/docker/.build/ 2>/dev/null || true

# Clean Podman cache
echo "Cleaning Podman build cache..."
podman system prune -af --volumes

echo ""
echo "=== Cleanup Complete ==="
echo ""
echo "OpenShell containers, images, and configuration have been removed."
echo ""
echo "To reinstall OpenShell:"
echo "  1. source scripts/podman.env"
echo "  2. mise run cluster:build:full"
echo "  3. cargo install --path crates/openshell-cli --root ~/.local"
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
