#!/usr/bin/env bash

# SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
# SPDX-License-Identifier: Apache-2.0

# Unified cluster entrypoint: bootstrap if no cluster is running, then
# incremental deploy.

set -euo pipefail

# shellcheck source=detect-container-runtime.sh
source "$(dirname "$0")/detect-container-runtime.sh"

CLUSTER_NAME=${CLUSTER_NAME:-$(basename "$PWD")}
CLUSTER_NAME=$(normalize_name "${CLUSTER_NAME}")
CONTAINER_NAME="openshell-cluster-${CLUSTER_NAME}"

if ! $CONTAINER_RUNTIME ps -q --filter "name=${CONTAINER_NAME}" | grep -q .; then
	echo "No running cluster found. Bootstrapping..."
	exec tasks/scripts/cluster-bootstrap.sh fast
fi

# Container is running but not healthy — tear it down and re-bootstrap.
if ! $CONTAINER_RUNTIME ps -q --filter "name=^${CONTAINER_NAME}$" --filter "health=healthy" | grep -q .; then
	echo "Cluster container '${CONTAINER_NAME}' is running but not healthy. Recreating..."
	exec tasks/scripts/cluster-bootstrap.sh fast
fi

exec tasks/scripts/cluster-deploy-fast.sh "$@"
