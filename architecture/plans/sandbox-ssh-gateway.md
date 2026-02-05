# Add SSH Access via Multiplexed Gateway

**Summary**
Enable SSH access to running sandboxes through the existing multiplexed gateway port using a protocol-aware HTTP proxy. The gateway listens on HTTPS and upgrades or CONNECTs specific requests into raw TCP tunnels that proxy SSH bytes to an embedded SSH server inside `navigator-sandbox`. The plan identifies required API/config changes and outlines implementation steps.

**Goals**

1. Allow `navigator sandbox connect <id>` to open an interactive shell via SSH.
2. Reuse the gateway port (gRPC/HTTP multiplex) without adding a new public listener.
3. Tie access to sandbox identity and lifecycle (deny when not Ready).
4. Keep audit logs and clear failure modes for connection attempts.

**Non-Goals**

1. Full multi-user identity management or long-lived SSH keys.

**Solution**
**HTTP CONNECT/Upgrade tunnel (protocol-aware proxy)**

- Flow:
  - Client uses ProxyCommand helper to open HTTPS to the gateway.
  - Gateway accepts an HTTP CONNECT or Upgrade request (path/header-based routing).
  - Gateway resolves sandbox target (header/host) and opens a TCP stream to the sandbox SSH listener.
  - Gateway pipes raw bytes between client and pod using `tokio::io::copy_bidirectional`.
- Pros:
  - Keeps a single HTTPS entry point; no raw SSH on the edge.
  - Reuses existing HTTP/TLS stack (Hyper/Axum + Rustls).
  - Straightforward multiplexing with existing gRPC/HTTP routing.
- Cons:
  - Requires an embedded SSH server in `navigator-sandbox` and key management.
  - CLI must manage ProxyCommand or a helper binary.
  - Needs pod routing/registry logic in the gateway.

**Important API Changes**

1. `proto/navigator.proto`
   - Add RPCs for SSH session bootstrap:
     - `CreateSshSession(CreateSshSessionRequest) returns (CreateSshSessionResponse)`
     - Optional: `RevokeSshSession(RevokeSshSessionRequest) returns (RevokeSshSessionResponse)`
   - Response includes a short-lived token, gateway host/port, target selector (sandbox id), and optional host key fingerprint.

**Design Notes**

- **Protocol-aware proxy**: add an HTTP handler that supports CONNECT or Upgrade to raw TCP. This lives alongside existing gRPC/health routes and keeps TLS termination in the current stack.
- **Routing**: route by `Host` header, SNI, or a custom header (e.g., `x-sandbox-id`). Resolve to pod IP via a registry that is either static or backed by a Kubernetes watcher (labels like `navigator.ai/sandbox-id`).
- **Pod access**: `navigator-sandbox` runs an embedded SSH server that listens on a dedicated port inside the pod. The gateway opens a TCP stream to that port.
- **Auth model**: issue a sandbox-scoped token at sandbox creation time and validate it on the HTTP tunnel request (header or query). Store token metadata in the DB (no TTL); include sandbox ID and optional allowed command list. The embedded SSH server accepts any key; the gateway enforces auth.
- **Gateway-to-sandbox handshake**: before starting the SSH handshake, the gateway sends a small preface (magic + token + nonce + timestamp + HMAC). The embedded SSH server validates using a shared secret or JWT verification key, replies `OK`, then starts SSH. This keeps the client-visible SSH protocol unchanged while preventing direct pod access.
- **Byte streaming**: use `tokio::io::copy_bidirectional` for efficient, symmetric copying.
- **Observability**: log connect/disconnect, duration, sandbox id, and auth outcome; surface errors as gRPC status for `CreateSshSession`.

**Implementation Steps**

1. **Proto + CLI**
   - Add SSH session bootstrap RPCs in `proto/navigator.proto`.
   - Generate new protobufs and wire into `crates/navigator-core`.
   - Implement a small CLI ProxyCommand helper (e.g., `navigator sandbox ssh-proxy`) and wire `navigator sandbox connect` to it:
     - Call `CreateSshSession` to fetch the sandbox-scoped token and gateway info.
     - Spawn `ssh -o ProxyCommand='navigator sandbox ssh-proxy --gateway ... --token ... --sandbox ...'`.
2. **Gateway tunnel handler**
   - Add an HTTP handler for CONNECT or Upgrade (Axum/Hyper) under a dedicated path (e.g., `/connect/ssh`).
   - Validate token and sandbox id before upgrading to raw TCP.
   - Resolve sandbox -> pod IP/port from the registry; dial and pipe bytes.
3. **Routing registry**
   - Add a registry backed by K8s watch (labels like `navigator.ai/sandbox-id`).
   - Keep a static fallback map for dev/local clusters.
4. **`navigator-sandbox` SSH daemon**
   - Add an embedded SSH server (e.g., `russh`) that listens on a dedicated port.
   - For each SSH session, spawn a sandboxed shell using the existing policy and a PTY.
   - Configure host keys and auth behavior (accept-any-key).
   - Implement the gateway handshake preface and reject unauthenticated connections.
5. **Config + Secrets**
   - Add gateway config for sandbox SSH port and header/routing strategy.
   - Add sandbox config for SSH listen address, host keys, and handshake secret or JWT key.

**Test Cases and Scenarios**

1. Connect to a Ready sandbox and run `whoami` (interactive shell).
2. Deny connection when sandbox is not Ready or does not exist.
3. Session expires after TTL (auth rejected).
4. TLS-enabled gateway: gRPC/HTTP still work; SSH routed correctly.
5. Concurrent SSH sessions to different sandboxes without cross-talk.
6. Clean shutdown: gateway closes exec stream when SSH client disconnects.

**Assumptions and Defaults**

- Sandbox ID is the canonical routing key for SSH sessions.
- Default shell is `/bin/bash` unless overridden by config.
- Session tokens live for the lifetime of the sandbox and are revoked on deletion.
- Embedded SSH accepts any key; gateway auth + handshake is the gate.
- No SFTP in the initial milestone; interactive shell only.
- Revisit auth design later (token rotation, TTLs, and per-user controls).
