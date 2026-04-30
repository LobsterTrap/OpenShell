#!/bin/sh
# SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
# SPDX-License-Identifier: Apache-2.0
#
# Tests for the registries.yaml generation logic in cluster-entrypoint.sh.
#
# Extracts the registry configuration block from the entrypoint and runs it
# in isolation with various combinations of environment variables, then
# asserts on the resulting registries.yaml content and log output.
#
# Usage: sh e2e/cluster-entrypoint/registries_test.sh

set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
ENTRYPOINT="$REPO_ROOT/deploy/docker/cluster-entrypoint.sh"

_PASS=0
_FAIL=0

# ---------------------------------------------------------------------------
# Assertions
# ---------------------------------------------------------------------------

pass() {
	_PASS=$((_PASS + 1))
	printf '  PASS: %s\n' "$1"
}

fail() {
	_FAIL=$((_FAIL + 1))
	printf '  FAIL: %s\n' "$1" >&2
	if [ -n "${2:-}" ]; then
		printf '        %s\n' "$2" >&2
	fi
}

assert_file_exists() {
	if [ -f "$1" ]; then
		pass "$2"
	else
		fail "$2" "file not found: $1"
	fi
}

assert_file_not_exists() {
	if [ ! -f "$1" ]; then
		pass "$2"
	else
		fail "$2" "file unexpectedly exists: $1"
	fi
}

assert_file_contains() {
	if grep -qF "$2" "$1" 2>/dev/null; then
		pass "$3"
	else
		fail "$3" "expected '$2' in $1"
	fi
}

assert_file_not_contains() {
	if ! grep -qF "$2" "$1" 2>/dev/null; then
		pass "$3"
	else
		fail "$3" "unexpected '$2' found in $1"
	fi
}

assert_output_contains() {
	if printf '%s' "$1" | grep -qF "$2"; then
		pass "$3"
	else
		fail "$3" "expected '$2' in output"
	fi
}

print_summary() {
	printf '\n=== Results: %d passed, %d failed ===\n' "$_PASS" "$_FAIL"
	[ "$_FAIL" -eq 0 ]
}

# ---------------------------------------------------------------------------
# Test harness
# ---------------------------------------------------------------------------
# Extracts the yaml_quote helper and the registries.yaml generation block
# from the real cluster-entrypoint.sh and runs them in isolation. This
# avoids executing the full entrypoint (DNS, iptables, k3s, etc.) while
# ensuring tests always exercise the actual production code.

WORK_DIR=""

setup() {
	WORK_DIR="$(mktemp -d)"
	export REGISTRIES_YAML="$WORK_DIR/registries.yaml"

	# Clear all registry-related env vars
	unset REGISTRY_HOST 2>/dev/null || true
	unset REGISTRY_ENDPOINT 2>/dev/null || true
	unset REGISTRY_INSECURE 2>/dev/null || true
	unset REGISTRY_USERNAME 2>/dev/null || true
	unset REGISTRY_PASSWORD 2>/dev/null || true
	unset COMMUNITY_REGISTRY_HOST 2>/dev/null || true
	unset COMMUNITY_REGISTRY_USERNAME 2>/dev/null || true
	unset COMMUNITY_REGISTRY_PASSWORD 2>/dev/null || true
	unset OPENSHELL_CONTAINER_REGISTRY 2>/dev/null || true
}

teardown() {
	echo "  Info: test artifacts left in $WORK_DIR"
}

# Run the registries.yaml generation logic in isolation.
# Extracts the yaml_quote function and the registry config block from the
# real cluster-entrypoint.sh, then runs them in a subshell.
# Captures stdout+stderr for log message assertions.
run_registry_config() {
	{
		echo '#!/bin/sh'
		echo 'set -e'
		# Extract yaml_quote helper
		sed -n '/^yaml_quote()/,/^}/p' "$ENTRYPOINT"
		# Extract registry config block (between the two section markers),
		# stripping the REGISTRIES_YAML= assignment so our test path is used.
		sed -n '/^# Generate k3s private registry configuration/,/^# Copy bundled Helm chart tarballs/p' "$ENTRYPOINT" \
			| sed '1d' | sed '$d' \
			| sed '/^REGISTRIES_YAML="/d'
	} >"$WORK_DIR/generate.sh"
	chmod +x "$WORK_DIR/generate.sh"
	sh "$WORK_DIR/generate.sh" 2>&1
}

# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

test_registry_host_and_container_registry() {
	printf 'TEST: REGISTRY_HOST set + OPENSHELL_CONTAINER_REGISTRY set\n'
	setup

	export REGISTRY_HOST="ghcr.io"
	export OPENSHELL_CONTAINER_REGISTRY="https://mirror.gcr.io"

	OUTPUT=$(run_registry_config)

	assert_file_exists "$REGISTRIES_YAML" "registries.yaml created"
	assert_file_contains "$REGISTRIES_YAML" '"ghcr.io"' "contains ghcr.io registry"
	assert_file_contains "$REGISTRIES_YAML" '"docker.io"' "contains docker.io registry"
	assert_file_contains "$REGISTRIES_YAML" 'https://mirror.gcr.io' "contains mirror.gcr.io endpoint"
	assert_output_contains "$OUTPUT" "Adding default container registry" "logs container registry addition"

	teardown
}

test_registry_host_without_container_registry() {
	printf 'TEST: REGISTRY_HOST set + OPENSHELL_CONTAINER_REGISTRY unset\n'
	setup

	export REGISTRY_HOST="ghcr.io"

	OUTPUT=$(run_registry_config)

	assert_file_exists "$REGISTRIES_YAML" "registries.yaml created"
	assert_file_contains "$REGISTRIES_YAML" '"ghcr.io"' "contains ghcr.io registry"
	assert_file_not_contains "$REGISTRIES_YAML" '"docker.io"' "does not contain docker.io registry"
	assert_output_contains "$OUTPUT" "unqualified image pulls will use docker.io directly" "logs info about direct docker.io pulls"

	teardown
}

test_no_registry_host_with_container_registry() {
	printf 'TEST: REGISTRY_HOST unset + OPENSHELL_CONTAINER_REGISTRY set\n'
	setup

	export OPENSHELL_CONTAINER_REGISTRY="https://mirror.gcr.io"

	OUTPUT=$(run_registry_config)

	assert_file_exists "$REGISTRIES_YAML" "registries.yaml created"
	assert_file_contains "$REGISTRIES_YAML" '"docker.io"' "contains docker.io registry"
	assert_file_contains "$REGISTRIES_YAML" 'https://mirror.gcr.io' "contains mirror.gcr.io endpoint"
	assert_file_not_contains "$REGISTRIES_YAML" '"ghcr.io"' "does not contain ghcr.io registry"
	assert_output_contains "$OUTPUT" "Configuring default container registry" "logs container registry configuration"
	assert_output_contains "$OUTPUT" "REGISTRY_HOST not set" "logs warning about missing REGISTRY_HOST"

	teardown
}

test_no_registry_host_no_container_registry() {
	printf 'TEST: REGISTRY_HOST unset + OPENSHELL_CONTAINER_REGISTRY unset\n'
	setup

	OUTPUT=$(run_registry_config)

	assert_file_not_exists "$REGISTRIES_YAML" "registries.yaml not created"
	assert_output_contains "$OUTPUT" "REGISTRY_HOST not set" "logs warning about missing REGISTRY_HOST"
	assert_output_contains "$OUTPUT" "unqualified image pulls will use docker.io directly" "logs info about direct docker.io pulls"

	teardown
}

test_container_registry_with_auth() {
	printf 'TEST: REGISTRY_HOST + OPENSHELL_CONTAINER_REGISTRY + auth credentials\n'
	setup

	export REGISTRY_HOST="ghcr.io"
	export REGISTRY_USERNAME="__token__"
	export REGISTRY_PASSWORD="ghp_faketoken123"
	export OPENSHELL_CONTAINER_REGISTRY="https://mirror.gcr.io"

	OUTPUT=$(run_registry_config)

	assert_file_exists "$REGISTRIES_YAML" "registries.yaml created"
	assert_file_contains "$REGISTRIES_YAML" '"docker.io"' "contains docker.io registry"
	assert_file_contains "$REGISTRIES_YAML" '"ghcr.io"' "contains ghcr.io registry"
	assert_file_contains "$REGISTRIES_YAML" "configs:" "contains configs section"
	assert_file_contains "$REGISTRIES_YAML" "__token__" "contains auth username"

	teardown
}

test_container_registry_valid_yaml_structure() {
	printf 'TEST: generated registries.yaml has valid YAML mirror structure\n'
	setup

	export REGISTRY_HOST="ghcr.io"
	export OPENSHELL_CONTAINER_REGISTRY="https://mirror.gcr.io"

	run_registry_config >/dev/null

	# Verify docker.io entry is indented under mirrors: (not a top-level key)
	if grep -q '^  "docker.io":' "$REGISTRIES_YAML"; then
		pass "docker.io entry is correctly indented under mirrors:"
	else
		fail "docker.io entry is correctly indented under mirrors:" \
			"expected 2-space indent, got: $(grep 'docker.io' "$REGISTRIES_YAML")"
	fi

	# Verify there is only one mirrors: top-level key
	mirrors_count=$(grep -c '^mirrors:' "$REGISTRIES_YAML")
	if [ "$mirrors_count" -eq 1 ]; then
		pass "exactly one mirrors: top-level key"
	else
		fail "exactly one mirrors: top-level key" "found $mirrors_count"
	fi

	teardown
}

# ---------------------------------------------------------------------------
# Runner
# ---------------------------------------------------------------------------

printf '=== cluster-entrypoint registries.yaml generation tests ===\n\n'

test_registry_host_and_container_registry;        echo ""
test_registry_host_without_container_registry;    echo ""
test_no_registry_host_with_container_registry;    echo ""
test_no_registry_host_no_container_registry;      echo ""
test_container_registry_with_auth;                echo ""
test_container_registry_valid_yaml_structure

print_summary
