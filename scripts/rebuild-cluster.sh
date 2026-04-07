#!/usr/bin/env bash

# SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
# SPDX-License-Identifier: Apache-2.0

# Quick rebuild script for development
# Restarts the cluster container with the latest code changes

set -euo pipefail

echo "=== OpenShell Quick Rebuild ==="
echo ""

# Stop and remove cluster container
echo "Stopping cluster container..."
podman stop openshell-cluster-openshell 2>/dev/null || true
podman rm openshell-cluster-openshell 2>/dev/null || true

# Remove old cluster image
echo "Removing old cluster image..."
podman rmi localhost/openshell/cluster:dev 2>/dev/null || true

# Rebuild and start cluster
echo "Rebuilding cluster with latest code..."
mise run cluster:build:full

echo ""
echo "=== Rebuild Complete ==="
echo ""
echo "Next steps:"
echo "  1. Recreate provider: openshell provider create --name <name> --type <type> --from-existing"
echo "  2. Configure inference: openshell inference set --provider <name> --model <model>"
echo "  3. Recreate sandboxes: openshell sandbox create ..."
echo ""
