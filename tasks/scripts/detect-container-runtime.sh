#!/usr/bin/env bash

# SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
# SPDX-License-Identifier: Apache-2.0

# Detect container runtime: podman preferred over docker.
# Sets CONTAINER_RUNTIME to "podman" or "docker".
# Respects OPENSHELL_CONTAINER_RUNTIME override.
#
# Source this script at the top of any shell script that invokes docker/podman:
#   source "$(dirname "$0")/detect-container-runtime.sh"

detect_container_runtime() {
	# 1. Explicit override (validated)
	if [ -n "${OPENSHELL_CONTAINER_RUNTIME:-}" ]; then
		case "${OPENSHELL_CONTAINER_RUNTIME}" in
		docker | podman)
			CONTAINER_RUNTIME="$OPENSHELL_CONTAINER_RUNTIME"
			return
			;;
		*)
			echo "Error: OPENSHELL_CONTAINER_RUNTIME='${OPENSHELL_CONTAINER_RUNTIME}' is not valid." >&2
			echo "       Expected 'docker' or 'podman'." >&2
			exit 1
			;;
		esac
	fi

	# 2. Probe sockets first (a running daemon is a stronger signal than
	#    just having the binary on PATH). Matches the Rust detection order.
	local uid
	uid=$(id -u 2>/dev/null || true)
	if [ -S "${XDG_RUNTIME_DIR:-/run/user/${uid}}/podman/podman.sock" ] 2>/dev/null ||
		[ -S /run/podman/podman.sock ] ||
		[ -S /var/run/podman/podman.sock ]; then
		CONTAINER_RUNTIME=podman
		return
	fi
	if [ -S /var/run/docker.sock ]; then
		CONTAINER_RUNTIME=docker
		return
	fi

	# 3. Fall back to binary on PATH (podman preferred)
	if command -v podman &>/dev/null; then
		CONTAINER_RUNTIME=podman
		return
	fi

	if command -v docker &>/dev/null; then
		CONTAINER_RUNTIME=docker
		return
	fi

	echo "Error: No container runtime found. Install podman or docker." >&2
	exit 1
}

# Sets PODMAN_TLS_ARGS to ("--tls-verify=false") when using Podman with a
# local HTTP registry, or () otherwise.
# Usage: podman_local_tls_args "${image_ref}"
#        $CONTAINER_RUNTIME push ${PODMAN_TLS_ARGS[@]+"${PODMAN_TLS_ARGS[@]}"} ...
podman_local_tls_args() {
	PODMAN_TLS_ARGS=()
	local ref="${1:-}"
	if [[ "${CONTAINER_RUNTIME}" == "podman" ]] && [[ "${ref}" == 127.0.0.1:* || "${ref}" == localhost:* ]]; then
		PODMAN_TLS_ARGS=(--tls-verify=false)
	fi
}

# General utility: normalize a name to lowercase with hyphens only.
# Included here because all cluster scripts already source this file.
normalize_name() {
	echo "$1" | tr '[:upper:]' '[:lower:]' | sed 's/[^a-z0-9-]/-/g' | sed 's/--*/-/g' | sed 's/^-//;s/-$//'
}

# Auto-detect on source
detect_container_runtime
