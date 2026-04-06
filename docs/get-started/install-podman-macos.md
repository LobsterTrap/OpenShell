---
title:
  page: Set Up Podman on macOS
  nav: Podman (macOS)
description: Install and configure Podman on macOS for OpenShell using Podman Machine.
topics:
- Generative AI
- Cybersecurity
tags:
- Podman
- macOS
- Installation
- Container Runtime
content:
  type: how_to
  difficulty: technical_beginner
  audience:
  - engineer
  - data_scientist
---

<!--
  SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
  SPDX-License-Identifier: Apache-2.0
-->

# Set Up Podman on macOS

This guide walks through installing and configuring Podman on macOS (Apple Silicon) for use with OpenShell. Podman on macOS runs containers inside a Linux virtual machine managed by Podman Machine.

## Quick Start

```console
$ brew install podman mise
$ bash scripts/setup-podman-macos.sh
$ source ~/.zshrc
$ mise run docker:build:cluster
$ podman tag localhost/openshell/cluster:dev ghcr.io/lobstertrap/openshell/cluster:dev
$ cargo build --release -p openshell-cli
$ mkdir -p ~/.local/bin
$ cp target/release/openshell ~/.local/bin/
$ openshell sandbox create
```

## Install Podman and mise

Install Podman (container runtime) and mise (task runner used by the OpenShell project):

```console
$ brew install podman mise
```

## Run the Automated Setup Script

The `scripts/setup-podman-macos.sh` script automates Podman Machine configuration:

- Creates a dedicated `openshell` Podman machine (8 GB RAM, 4 CPUs)
- Configures cgroup delegation (required for the embedded k3s cluster)
- Sets up environment variables
- Stops conflicting machines (only one can run at a time)
- Optionally adds environment variables to your shell profile

```console
$ bash scripts/setup-podman-macos.sh
```

Follow the script's instructions to set environment variables. If you chose to add them to your shell profile:

```console
$ source ~/.zshrc
```

## Build the Cluster Image

Using mise:

```console
$ mise run docker:build:cluster
```

Or directly without mise:

```console
$ tasks/scripts/docker-build-image.sh cluster
```

Tag the image for local use:

```console
$ podman tag localhost/openshell/cluster:dev ghcr.io/lobstertrap/openshell/cluster:dev
```

## Build and Install the CLI

```console
$ cargo build --release -p openshell-cli
$ mkdir -p ~/.local/bin
$ cp target/release/openshell ~/.local/bin/
```

## Create a Sandbox

```console
$ openshell sandbox create
```

## Cleanup

To remove all OpenShell resources and optionally the Podman machine:

```console
$ bash cleanup-openshell-podman-macos.sh
```

## Troubleshooting

### Environment variables not set

If OpenShell cannot find the Podman socket, set the environment variables manually:

```console
$ SOCKET=$(podman machine inspect openshell --format '{{.ConnectionInfo.PodmanSocket.Path}}')
$ export CONTAINER_HOST="unix://${SOCKET}"
$ export OPENSHELL_CONTAINER_RUNTIME=podman
```

### "Gateway not reachable"

Destroy the existing gateway and recreate:

```console
$ openshell gateway destroy --name openshell
$ openshell sandbox create
```

### Build fails with memory errors

Increase the Podman machine memory allocation:

```console
$ podman machine stop openshell
$ podman machine set openshell --memory 8192
$ podman machine start openshell
```

### "failed to find cpuset cgroup"

Verify cgroup delegation inside the Podman machine:

```console
$ podman machine ssh openshell "cat /etc/systemd/system/user@.service.d/delegate.conf"
```

The output should show `Delegate=cpu cpuset io memory pids`. If not, run `scripts/setup-podman-macos.sh` again.

## Next Steps

- {doc}`quickstart` to create your first sandbox.
- {doc}`../sandboxes/manage-gateways` for gateway configuration options.
- {doc}`../reference/support-matrix` for the full requirements matrix.
