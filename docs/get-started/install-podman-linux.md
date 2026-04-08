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

## Quick Start

Run the automated setup script to handle all the steps in this guide:

::::{tab-set}

:::{tab-item} Rootless (Recommended)

```console
$ bash scripts/setup-podman-linux.sh
```

:::

:::{tab-item} Rootful

```console
$ bash scripts/setup-podman-linux.sh --rootful
```

:::

::::

The script detects your package manager, installs Podman if needed, configures the Podman socket and cgroup delegation, and handles headless-specific steps (login lingering, `XDG_RUNTIME_DIR`) when it detects no graphical session. The rest of this guide covers each step individually for users who prefer manual configuration.

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

### Headless Systems Prerequisites

:::{note}
**Headless/SSH systems only.** If you log in directly at a desktop session, skip ahead to [Enable the Podman Socket](#enable-the-podman-socket). Desktop sessions handle login lingering, `XDG_RUNTIME_DIR`, and the D-Bus session bus automatically via PAM.
:::

On headless systems (accessed only over SSH, or when switching users with `su`/`sudo su`), three things must be configured before `systemctl --user` will work. Complete these steps in order.

**Enable login lingering.** Without lingering, systemd does not start a user session manager for your account. This means there is no D-Bus session bus, no user socket directory, and `systemctl --user` cannot function:

```console
$ sudo loginctl enable-linger $USER
```

Verify lingering is active:

```console
$ loginctl show-user $USER --property=Linger
Linger=yes
```

Lingering also keeps your user services alive when all SSH sessions disconnect, which prevents the Podman socket and gateway container from being killed.

**Set XDG_RUNTIME_DIR and DBUS_SESSION_BUS_ADDRESS.** `systemctl --user` needs these two environment variables to find the user session bus. Direct SSH logins with PAM enabled set them automatically. If you access the system via `su -` or `sudo su`, they are not set. Check whether they exist:

```console
$ echo $XDG_RUNTIME_DIR
$ echo $DBUS_SESSION_BUS_ADDRESS
```

If either is empty, add the following to `~/.bashrc` (or equivalent shell profile):

```bash
export XDG_RUNTIME_DIR=/run/user/$(id -u)
export DBUS_SESSION_BUS_ADDRESS=unix:path=$XDG_RUNTIME_DIR/bus
```

Then reload:

```console
$ source ~/.bashrc
```

:::{warning}
`DBUS_SESSION_BUS_ADDRESS` references `$XDG_RUNTIME_DIR` in its value. You must set `XDG_RUNTIME_DIR` first. If you run `export DBUS_SESSION_BUS_ADDRESS=unix:path=$XDG_RUNTIME_DIR/bus` when `XDG_RUNTIME_DIR` is empty, the path expands to `unix:path=/bus`, which is invalid. Always source both exports together (e.g., `source ~/.bashrc`), or set `XDG_RUNTIME_DIR` on its own line before `DBUS_SESSION_BUS_ADDRESS`.

Login lingering must also be enabled before these variables are useful. The bus socket at `$XDG_RUNTIME_DIR/bus` is only created when systemd starts the user session manager, which requires lingering.
:::

### Enable the Podman Socket

OpenShell communicates with Podman through a Unix socket. Enable the rootless socket for your user:

```console
$ systemctl --user enable --now podman.socket
```

Verify the socket exists:

```console
$ ls $XDG_RUNTIME_DIR/podman/podman.sock
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

If OpenShell reports "Podman is installed but its API socket is not active," the Podman binary is present but the systemd socket unit that exposes its API is not running. This is common on fresh installs where `dnf install podman` or `apt install podman` does not enable the socket automatically.

For rootful mode, enable the system socket:

```console
$ sudo systemctl enable --now podman.socket
```

For rootless mode, enable the user socket:

```console
$ systemctl --user enable --now podman.socket
```

If this fails with `$DBUS_SESSION_BUS_ADDRESS and $XDG_RUNTIME_DIR not defined` or `No such file or directory`, see the [systemctl --user fails](#systemctl-user-fails-with-not-defined-or-no-such-file-or-directory) section below.

Verify the socket is active:

```console
$ systemctl --user status podman.socket   # rootless
$ sudo systemctl status podman.socket     # rootful
```

After enabling the socket, retry `openshell gateway start`.

### systemctl --user fails with "not defined" or "No such file or directory"

`systemctl --user` can fail with two related errors:

**Error: variables not defined**

```
Failed to connect to user scope bus via local transport: $DBUS_SESSION_BUS_ADDRESS and $XDG_RUNTIME_DIR not defined
```

Neither environment variable is set. This happens when you switch users with `su -` or `sudo su`, or when your SSH server does not run PAM session modules.

**Error: No such file or directory**

```
Failed to connect to user scope bus via local transport: No such file or directory
```

The variables are set but the D-Bus bus socket they point to does not exist. There are two common causes:

- **`XDG_RUNTIME_DIR` was empty when `DBUS_SESSION_BUS_ADDRESS` was set.** Because the `DBUS_SESSION_BUS_ADDRESS` export references `$XDG_RUNTIME_DIR`, running `export DBUS_SESSION_BUS_ADDRESS=unix:path=$XDG_RUNTIME_DIR/bus` when `XDG_RUNTIME_DIR` is empty produces `unix:path=/bus`, which does not exist. Check the current value:

  ```console
  $ echo $DBUS_SESSION_BUS_ADDRESS
  ```

  If it shows `unix:path=/bus` (missing the `/run/user/<uid>` prefix), this is the problem.

- **Login lingering is not enabled.** Without lingering, systemd does not start a user session manager, so the bus socket at `/run/user/<uid>/bus` is never created.

**Fix for both errors:**

Run these three steps in order. Each step depends on the one before it:

1. Enable login lingering. This starts the systemd user session manager, which creates the bus socket:

   ```console
   $ sudo loginctl enable-linger $USER
   ```

2. Set `XDG_RUNTIME_DIR` **first**, then `DBUS_SESSION_BUS_ADDRESS`. The second variable references the first, so the order matters:

   ```console
   $ export XDG_RUNTIME_DIR=/run/user/$(id -u)
   $ export DBUS_SESSION_BUS_ADDRESS=unix:path=$XDG_RUNTIME_DIR/bus
   ```

   To make this permanent, add both lines (in this order) to `~/.bashrc` or equivalent.

3. Verify and retry:

   ```console
   $ echo $DBUS_SESSION_BUS_ADDRESS
   unix:path=/run/user/1000/bus
   $ systemctl --user enable --now podman.socket
   ```

   The `echo` output should show the full path including `/run/user/<uid>`. If it shows `unix:path=/bus`, go back to step 2.

:::{tip}
If you SSH directly as the target user (rather than `su` from root), most distributions set these variables automatically via PAM. If you still see this error after a direct SSH login, check that `UsePAM yes` is set in `/etc/ssh/sshd_config`.
:::

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
