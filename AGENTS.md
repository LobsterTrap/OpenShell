# Agent Instructions

This file is the primary instruction surface for agents contributing to OpenShell. It is injected into your context on every interaction — keep that in mind when proposing changes to it.

See [CONTRIBUTING.md](CONTRIBUTING.md) for build instructions, task reference, project structure, and the full agent skills table.

## Project Identity

OpenShell is built agent-first. We design systems and use agents to implement them — this is not vibe coding. The product provides safe, sandboxed runtimes for autonomous AI agents, and the project itself is built using the same agent-driven workflows it enables.

## Skills

Agent skills live in `.agents/skills/`. Your harness can discover and load them natively — do not rely on this file for a full inventory. The detailed skills table is in [CONTRIBUTING.md](CONTRIBUTING.md) (for humans).

## Workflow Chains

These pipelines connect skills into end-to-end workflows. Individual skill files don't describe these relationships.

- **Community inflow:** `triage-issue` → `create-spike` → `build-from-issue`
  - Triage assesses and classifies community-filed issues. Spike investigates unknowns. Build implements.
- **Internal development:** `create-spike` → `build-from-issue`
  - Spike explores feasibility, then build executes once `state:agent-ready` is applied by a human.
- **Security:** `review-security-issue` → `fix-security-issue`
  - Review produces a severity assessment and remediation plan. Fix implements it. Both require the `topic:security` label; fix also requires `state:agent-ready`.
- **Policy iteration:** `openshell-cli` → `generate-sandbox-policy`
  - CLI manages the sandbox lifecycle; policy generation authors the YAML constraints.

## Architecture Overview

| Path | Components | Purpose |
|------|-----------|---------|
| `crates/openshell-cli/` | CLI binary | User-facing command-line interface |
| `crates/openshell-server/` | Gateway server | Control-plane API, sandbox lifecycle, auth boundary |
| `crates/openshell-sandbox/` | Sandbox runtime | Container supervision, policy-enforced egress routing |
| `crates/openshell-policy/` | Policy engine | Filesystem, network, process, and inference constraints |
| `crates/openshell-router/` | Privacy router | Privacy-aware LLM routing |
| `crates/openshell-bootstrap/` | Cluster bootstrap | K3s cluster setup, image loading, mTLS PKI |
| `crates/openshell-core/` | Shared core | Common types, configuration, error handling |
| `crates/openshell-providers/` | Provider management | Credential provider backends |
| `crates/openshell-tui/` | Terminal UI | Ratatui-based dashboard for monitoring |
| `python/openshell/` | Python SDK | Python bindings and CLI packaging |
| `proto/` | Protobuf definitions | gRPC service contracts |
| `deploy/` | Docker, Helm, K8s | Dockerfiles, Helm chart, manifests |
| `.agents/skills/` | Agent skills | Workflow automation for development |
| `.agents/agents/` | Agent personas | Sub-agent definitions (e.g., reviewer, doc writer) |
| `architecture/` | Architecture docs | Design decisions and component documentation |

## Vouch System

- First-time external contributors must be vouched before their PRs are accepted. The `vouch-check` workflow auto-closes PRs from unvouched users.
- Org members and collaborators bypass the vouch gate automatically.
- Maintainers vouch users by commenting `/vouch` on a Vouch Request discussion. The `vouch-command` workflow appends the username to `.github/VOUCHED.td`.
- Skills that create PRs (`create-github-pr`, `build-from-issue`) should note this requirement when operating on behalf of external contributors.

## Issue and PR Conventions

- **Bug reports** must include an agent diagnostic section — proof that the reporter's agent investigated the issue before filing. See the issue template.
- **Feature requests** must include a design proposal, not just a "please build this" request. See the issue template.
- **PRs** must follow the PR template structure: Summary, Related Issue, Changes, Testing, Checklist.
- **PRs from unvouched external contributors** are automatically closed. See the Vouch System section above.
- **Security vulnerabilities** must NOT be filed as GitHub issues. Follow [SECURITY.md](SECURITY.md).
- Skills that create issues or PRs (`create-github-issue`, `create-github-pr`, `build-from-issue`) should produce output conforming to these templates.

## Plans

- Store plan documents in `architecture/plans`. This is git ignored so its for easier access for humans. When asked to create Spikes or issues, you can skip to GitHub issues. Only use the plans dir when you aren't writing data somewhere else specific.
- When asked to write a plan, write it there without asking for the location.

## Sandbox Infra Changes

- If you change sandbox infrastructure, ensure `mise run sandbox` succeeds.

## Commits

- Always use [Conventional Commits](https://www.conventionalcommits.org/) format for commit messages
- Format: `<type>(<scope>): <description>` (scope is optional)
- Common types: `feat`, `fix`, `docs`, `chore`, `refactor`, `test`, `ci`, `perf`
- Never mention Claude or any AI agent in commits (no author attribution, no Co-Authored-By, no references in commit messages)

## Pre-commit

- Run `mise run pre-commit` before committing.
- Install the git hook when working locally: `mise generate git-pre-commit --write --task=pre-commit`

## Testing

- `mise run pre-commit` — Lint, format, license headers. Run before every commit.
- `mise run test` — Unit test suite. Run after code changes.
- `mise run e2e` — End-to-end tests against a running cluster. Run for infrastructure, sandbox, or policy changes.
- `mise run ci` — Full local CI (lint + compile/type checks + tests). Run before opening a PR.

## Python

- Always use `uv` for Python commands (e.g., `uv pip install`, `uv run`, `uv venv`)

## Container Runtimes (Docker / Podman)

- Always prefer `mise` commands over direct docker/podman builds (e.g., `mise run docker:build` instead of `docker build`)
- The codebase supports both Docker and Podman. Podman is preferred when both are available. Override with `--container-runtime` or `OPENSHELL_CONTAINER_RUNTIME`.
- Bollard (the Rust Docker client library) connects to Podman via its Docker-compatible API — no separate Podman client is needed.
- When referencing host gateway aliases, use both `host.docker.internal` and `host.containers.internal` for cross-runtime compatibility.

### Debugging with Podman

When using Podman (especially on macOS where Podman runs in a VM), debugging requires accessing the Podman machine:

**Accessing the Podman VM:**
```bash
podman machine ssh
```

**Common debugging commands:**
```bash
# Check cluster logs via kubectl (inside podman machine or via ssh)
podman machine ssh -- "podman exec openshell-cluster-openshell kubectl logs -n openshell <pod-name>"

# Check running containers
podman machine ssh -- "podman ps -a"

# Check images and timestamps
podman machine ssh -- "podman images"

# Verify binary in cluster
podman machine ssh -- "podman exec openshell-cluster-openshell ls -lh /opt/openshell/bin/openshell-sandbox"

# Check for specific strings in binary
podman machine ssh -- "podman exec openshell-cluster-openshell strings /opt/openshell/bin/openshell-sandbox | grep <pattern>"

# Get sandbox pod logs
podman machine ssh -- "podman exec openshell-cluster-openshell kubectl logs -n openshell <sandbox-name> --container agent --tail 100"
```

**Important: Cross-compilation requirement**

Running `cargo build --release` on macOS produces a macOS binary, not a Linux binary. The cluster runs Linux containers, so using a macOS binary causes "exec format error".

- ✅ **Correct:** Use `mise run cluster:build:full` which handles cross-compilation
- ❌ **Incorrect:** `cargo build --release` then manually copying the binary

**Fast iteration workflow:**

After modifying Rust code in `crates/openshell-sandbox/`:

```bash
# Force clean rebuild to avoid cargo cache issues
cargo clean -p openshell-sandbox

# Full cluster rebuild (handles cross-compilation)
mise run cluster:build:full

# Recreate sandbox to pick up new binary
openshell sandbox delete <name>
openshell sandbox create --name <name> --provider <provider> --policy <policy> -- bash
```

**Common issues:**

- **"exec format error"**: Binary is for wrong architecture (macOS vs Linux)
- **Binary not updating**: Cargo is using cached artifacts - run `cargo clean -p openshell-sandbox`
- **Empty logs**: `RUST_LOG` environment variable not set in sandbox agent - logs are disabled by default
- **Changes not reflected**: Sandbox was created before cluster rebuild - always recreate sandboxes after deploying new binaries

## Cluster Infrastructure Changes

- If you change cluster bootstrap infrastructure (e.g., `openshell-bootstrap` crate, `deploy/docker/Dockerfile.images`, `cluster-entrypoint.sh`, `cluster-healthcheck.sh`, deploy logic in `openshell-cli`), update the `debug-openshell-cluster` skill in `.agents/skills/debug-openshell-cluster/SKILL.md` to reflect those changes.

## Documentation

- When making changes, update the relevant documentation in the `architecture/` directory.
- When changes affect user-facing behavior, also update the relevant pages under `docs/`.
- Follow the style guide in [docs/CONTRIBUTING.md](docs/CONTRIBUTING.md): active voice, no unnecessary bold, no em dash overuse, no filler introductions.
- Use the `update-docs` skill to scan recent commits and draft doc updates.

## Security

- Never commit secrets, API keys, or credentials. If a file looks like it contains secrets (`.env`, `credentials.json`, etc.), do not stage it.
- Do not run destructive operations (force push, hard reset, database drops) without explicit human confirmation.
- Scope changes to the issue at hand. Do not make unrelated changes in the same branch.
