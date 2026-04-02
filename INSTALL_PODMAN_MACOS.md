# Installing OpenShell with Podman on macOS (Apple Silicon)

Quick setup guide for running OpenShell with Podman on **macOS Apple Silicon (ARM64)**.

**Platform-specific**: This guide is for macOS only.

## Quick Start

```bash
# 1. Install Podman and mise
brew install podman mise

# 2. Run the automated setup script
bash scripts/setup-podman-macos.sh

# 3. Set environment variables (follow the script's instructions)
source ~/.zshrc  # if you added them to your profile

# 4. Build cluster image
mise run docker:build:cluster

# 5. Tag the image for local use
podman tag localhost/openshell/cluster:dev ghcr.io/nvidia/openshell/cluster:dev

# 6. Build and install CLI
cargo build --release -p openshell-cli
mkdir -p ~/.local/bin
cp target/release/openshell ~/.local/bin/

# 7. Create a sandbox
openshell sandbox create
```

## What Gets Installed

- **Podman**: Container runtime (Docker alternative)
- **mise**: Task runner used by OpenShell project for build tasks

## What the Setup Script Does

`scripts/setup-podman-macos.sh` automates Podman machine configuration:

- Creates dedicated `openshell` Podman machine (8GB RAM, 4 CPUs)
- Configures cgroup delegation (required for rootless k3s)
- Sets up environment variables
- Stops conflicting machines (only one can run at a time)
- Optionally adds env vars to your shell profile

## Building Without mise

If you prefer not to use mise, run the build script directly:

```bash
tasks/scripts/docker-build-image.sh cluster
```

## Cleanup

```bash
bash cleanup-openshell-podman-macos.sh
```

## Troubleshooting

### Environment variables not set

```bash
SOCKET=$(podman machine inspect openshell --format '{{.ConnectionInfo.PodmanSocket.Path}}')
export CONTAINER_HOST="unix://${SOCKET}"
export OPENSHELL_CONTAINER_RUNTIME=podman
```

### "Gateway not reachable"

```bash
openshell gateway destroy --name openshell
openshell sandbox create
```

### Build fails with memory errors

```bash
podman machine stop openshell
podman machine set openshell --memory 8192
podman machine start openshell
```

### "failed to find cpuset cgroup"

Verify cgroup delegation:

```bash
podman machine ssh openshell "cat /etc/systemd/system/user@.service.d/delegate.conf"
```

Should show `Delegate=cpu cpuset io memory pids`. If not, run `scripts/setup-podman-macos.sh` again.
