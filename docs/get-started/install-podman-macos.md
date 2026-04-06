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
brew install podman mise
bash scripts/setup-podman-macos.sh
source scripts/podman.env
mise run cluster:build:full
cargo install --path crates/openshell-cli --root ~/.local
openshell sandbox create
```

## Install Podman and mise

Install Podman (container runtime) and mise (task runner used by the OpenShell project):

```console
brew install podman mise
```

## Run the Automated Setup Script

The `scripts/setup-podman-macos.sh` script automates Podman Machine configuration:

- Creates a dedicated `openshell` Podman machine (8 GB RAM, 4 CPUs)
- Configures cgroup delegation (required for the embedded k3s cluster)
- Stops conflicting machines (only one can run at a time, with user confirmation)

```console
bash scripts/setup-podman-macos.sh
```

## Set Up Environment Variables

Source the `scripts/podman.env` file to configure your shell for local development:

```console
source scripts/podman.env
```

This sets:
- `CONTAINER_HOST` - Podman socket path
- `OPENSHELL_CONTAINER_RUNTIME=podman` - Use Podman runtime
- `OPENSHELL_IMAGE_REPO_BASE=127.0.0.1:5000/openshell` - Local registry for component images
- `OPENSHELL_REGISTRY_HOST=127.0.0.1:5000` - Registry host
- `OPENSHELL_REGISTRY_INSECURE=true` - Allow HTTP registry
- `OPENSHELL_CLUSTER_IMAGE=localhost/openshell/cluster:dev` - Local cluster image

To make these persistent, add to your shell profile (`~/.zshrc` or `~/.bashrc`):

```console
echo "source $(pwd)/scripts/podman.env" >> ~/.zshrc
```

## Build and Deploy the Cluster

Build images, set up the local registry, and deploy the k3s cluster:

```console
mise run cluster:build:full
```

This command:
- Builds the gateway and cluster images
- Starts a local container registry at `127.0.0.1:5000`
- Pushes the gateway image to the local registry
- Bootstraps a k3s cluster inside a Podman container
- Deploys and starts the OpenShell gateway

**Note:** This command builds the images AND starts the gateway in one step. The gateway will be running when the command completes.

Or run the script directly:

```console
tasks/scripts/cluster-bootstrap.sh build
```

## Build and Install the CLI

**Note:** If you ran `mise run cluster:build:full` above, a debug version of the CLI was automatically compiled. You can use it directly from the project directory without installing:

```console
./target/debug/openshell --version
```

For a release-optimized binary that works system-wide:

```console
cargo install --path crates/openshell-cli --root ~/.local
```

## Create a Sandbox

The gateway is now running. Create a sandbox to test it:

```console
openshell sandbox create
```

Verify the gateway is healthy:

```console
openshell gateway info
```

## Cleanup

To remove all OpenShell resources and optionally the Podman machine:

```console
bash cleanup-openshell-podman-macos.sh
```

## Troubleshooting

### Environment variables not set

If OpenShell cannot find the Podman socket, source the environment file:

```console
source scripts/podman.env
```

### "Gateway not reachable"

Destroy the existing gateway and recreate:

```console
openshell gateway destroy --name openshell
openshell sandbox create
```

### Build fails with memory errors

Increase the Podman machine memory allocation:

```console
podman machine stop openshell
podman machine set openshell --memory 8192
podman machine start openshell
```

### "failed to find cpuset cgroup"

Verify cgroup delegation inside the Podman machine:

```console
podman machine ssh openshell "cat /etc/systemd/system/user@.service.d/delegate.conf"
```

The output should show `Delegate=cpu cpuset io memory pids`. If not, run `scripts/setup-podman-macos.sh` again.

## Next Steps

- {doc}`quickstart` to create your first sandbox.
- {doc}`../sandboxes/manage-gateways` for gateway configuration options.
- {doc}`../reference/support-matrix` for the full requirements matrix.
