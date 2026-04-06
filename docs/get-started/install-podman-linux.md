---
title:
  page: Set Up Podman on Linux
  nav: Podman (Linux)
description: Install and configure Podman on Linux for OpenShell in rootless or rootful mode.
topics:
- Generative AI
- Cybersecurity
tags:
- Podman
- Linux
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

# Set Up Podman on Linux

This guide walks through installing and configuring Podman on Linux for use with OpenShell. It covers both rootless mode (recommended for desktops and laptops) and rootful mode (simpler for headless servers).

## Prerequisites

- A systemd-based Linux distribution (Fedora, RHEL, CentOS Stream, Debian, Ubuntu)
- cgroup v2 enabled (default on Fedora 31+, Ubuntu 21.10+, RHEL 9+, Debian 11+)

Verify cgroup v2 is active:

```console
$ stat -fc %T /sys/fs/cgroup
cgroup2fs
```

If the output is `tmpfs` instead of `cgroup2fs`, your system uses cgroup v1 and you need to switch to cgroup v2. Refer to your distribution's documentation for instructions.

## Install Podman

::::{tab-set}

:::{tab-item} Fedora / RHEL / CentOS Stream

```console
$ sudo dnf install -y podman
```

:::

:::{tab-item} Debian / Ubuntu

```console
$ sudo apt-get update
$ sudo apt-get install -y podman
```

:::

::::

Verify the installation:

```console
$ podman --version
```

Podman 4.0 or later is required.

## Rootless Setup (Recommended)

Rootless Podman runs containers without root privileges. This is the default mode on Fedora and RHEL 9+ and the recommended configuration for desktops and laptops.

### Enable the Podman Socket

OpenShell communicates with Podman through a Unix socket. Enable the rootless socket for your user:

```console
$ systemctl --user enable --now podman.socket
```

Verify the socket exists:

```console
$ ls $XDG_RUNTIME_DIR/podman/podman.sock
```

### Configure Cgroup Delegation

OpenShell runs an embedded k3s cluster inside the gateway container. k3s needs to manage cgroup controllers for pod resource isolation. Without cgroup delegation, the gateway fails to start.

Create the systemd delegation configuration:

```console
$ sudo mkdir -p /etc/systemd/system/user@.service.d
$ sudo tee /etc/systemd/system/user@.service.d/delegate.conf <<'EOF'
[Service]
Delegate=cpu cpuset io memory pids
EOF
$ sudo systemctl daemon-reload
```

Log out and back in for the changes to take effect. Verify the delegation:

```console
$ cat /sys/fs/cgroup/user.slice/user-$(id -u).slice/cgroup.subtree_control
```

The output should include `cpuset cpu io memory pids`.

### Enable Login Lingering (Headless Systems)

:::{note}
**Headless/SSH systems only.** Skip this step on desktops and laptops where you log in directly. On those systems, your user session persists and systemd user services stay running.
:::

On headless systems accessed only over SSH, systemd terminates all user services when the last SSH session disconnects. This kills the Podman socket and any running gateway container. Login lingering tells systemd to keep your user services alive regardless of active sessions:

```console
$ loginctl enable-linger $USER
```

Verify:

```console
$ loginctl show-user $USER --property=Linger
Linger=yes
```

### Verify XDG_RUNTIME_DIR (Headless Systems)

:::{note}
**Headless/SSH systems only.** Desktop sessions set `XDG_RUNTIME_DIR` automatically via PAM. This step is only needed if you access the system exclusively over SSH.
:::

Some SSH configurations do not set `XDG_RUNTIME_DIR`, which prevents the rootless Podman socket from being found. Check whether it is set:

```console
$ echo $XDG_RUNTIME_DIR
```

If the output is empty, add the following to your `~/.bashrc` (or equivalent shell profile):

```bash
export XDG_RUNTIME_DIR=/run/user/$(id -u)
```

Then reload:

```console
$ source ~/.bashrc
```

### Verify Subuid and Subgid

Rootless containers require subordinate UID and GID ranges for user namespace mapping. Most distributions configure these automatically when a user account is created. Verify your entries exist:

```console
$ grep "^$USER:" /etc/subuid
$ grep "^$USER:" /etc/subgid
```

Each command should show a line like `youruser:100000:65536`. If either file is missing an entry for your user, add one:

::::{tab-set}

:::{tab-item} Fedora / RHEL

```console
$ sudo usermod --add-subuids 100000-165535 --add-subgids 100000-165535 $USER
```

:::

:::{tab-item} Debian / Ubuntu

```console
$ sudo usermod --add-subuids 100000-165535 --add-subgids 100000-165535 $USER
```

If `usermod` does not support `--add-subuids`, edit the files directly:

```console
$ echo "$USER:100000:65536" | sudo tee -a /etc/subuid
$ echo "$USER:100000:65536" | sudo tee -a /etc/subgid
```

:::

::::

After adding subuid/subgid ranges, restart the Podman socket:

```console
$ systemctl --user restart podman.socket
```

### Verify the Setup

Run a quick test to confirm everything works:

```console
$ podman info --format '{{.Host.CgroupVersion}}'
```

The output should be `v2`.

```console
$ podman run --rm docker.io/library/hello-world
```

If both commands succeed, your rootless Podman setup is ready.

### Start the Gateway

```console
$ openshell gateway start
```

OpenShell auto-detects the rootless Podman socket and uses it.

## Rootful Setup

:::{tip}
Rootful mode is simpler to configure, especially on headless systems, because it does not require cgroup delegation, login lingering, or `XDG_RUNTIME_DIR`. The tradeoff is that containers run as root.
:::

Rootful Podman runs containers as the system root user. Use this mode when rootless configuration is impractical or when you need features that require root privileges (such as certain GPU passthrough configurations).

### Enable the Rootful Socket

```console
$ sudo systemctl enable --now podman.socket
```

Verify the socket exists:

```console
$ ls /run/podman/podman.sock
```

### Set the Container Host

Tell OpenShell to use the rootful socket. You can do this per-command:

```console
$ openshell gateway start --container-runtime podman
```

Or set it permanently via environment variable in your shell profile (`~/.bashrc` or equivalent):

```bash
export CONTAINER_HOST=unix:///run/podman/podman.sock
export OPENSHELL_CONTAINER_RUNTIME=podman
```

### Start the Gateway

```console
$ openshell gateway start
```

## Container Runtime Selection

When you do not specify a runtime, the CLI auto-detects the available runtime by probing sockets and binaries. If both Docker and Podman are available, the CLI prefers Podman.

The CLI resolves the runtime in this order:

1. `--container-runtime` flag (highest priority)
2. `OPENSHELL_CONTAINER_RUNTIME` environment variable
3. Auto-detection (Podman preferred when both are available)

## Troubleshooting

### "failed to find cpuset cgroup"

The cgroup delegation is missing or incomplete. Re-run the delegation setup:

```console
$ sudo mkdir -p /etc/systemd/system/user@.service.d
$ sudo tee /etc/systemd/system/user@.service.d/delegate.conf <<'EOF'
[Service]
Delegate=cpu cpuset io memory pids
EOF
$ sudo systemctl daemon-reload
```

Log out and back in, then verify:

```console
$ cat /sys/fs/cgroup/user.slice/user-$(id -u).slice/cgroup.subtree_control
```

### Gateway exits immediately after start

Check the gateway logs:

```console
$ openshell doctor logs
```

Common causes:
- Missing cgroup delegation (see above)
- Insufficient memory (the gateway needs at least 2 GB available)
- Port 8080 already in use by another process

### Podman socket not found

If OpenShell reports that no container runtime is available:

For rootless mode, verify the socket is running:

```console
$ systemctl --user status podman.socket
$ ls $XDG_RUNTIME_DIR/podman/podman.sock
```

For rootful mode, verify the system socket:

```console
$ sudo systemctl status podman.socket
$ ls /run/podman/podman.sock
```

### Permission denied in rootless mode

If you see permission errors when starting containers, verify your subuid/subgid configuration:

```console
$ grep "^$USER:" /etc/subuid /etc/subgid
```

Both files must have entries for your user. See the "Verify Subuid and Subgid" section above.

### Gateway stops when SSH session disconnects

This happens on headless systems when login lingering is not enabled. Systemd terminates all user services when the last session ends, which kills the Podman socket and the gateway container.

Fix:

```console
$ loginctl enable-linger $USER
```

After enabling linger, restart the gateway. It will persist across SSH disconnects.

## Next Steps

- {doc}`quickstart` to create your first sandbox.
- {doc}`../sandboxes/manage-gateways` for gateway configuration options.
- {doc}`../reference/support-matrix` for the full requirements matrix.
