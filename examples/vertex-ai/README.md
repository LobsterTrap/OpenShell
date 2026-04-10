# Google Cloud Vertex AI Example

This example demonstrates how to use OpenShell with Google Cloud Vertex AI to run Claude models via GCP infrastructure.

## Credential Provider Architecture

OpenShell uses a **two-layer plugin architecture** for credential management:

**Layer 1: SecretStore (where credentials live)**
- Generic interface for retrieving raw credentials
- Current implementation: **DatabaseStore** - stores ADC in gateway database
- Future implementations: OneCLI, Vault, GCP Secret Manager, etc.

**Layer 2: ProviderPlugin (how to interpret credentials)**
- Provider-specific logic for exchanging credentials for tokens
- Current implementation: **VertexProvider** - exchanges ADC for OAuth tokens
- Future implementations: AnthropicProvider, OpenAIProvider, etc.

**TokenCache (orchestration layer)**
- Wraps ProviderPlugin + SecretStore
- Caches tokens in memory
- Policy-configurable auto-refresh (default: 5 min before expiry)
- Background task spawned only when `oauth_credentials.auto_refresh: true`

### Current Implementation

```
Provider Discovery
  └─> ~/.config/gcloud/application_default_credentials.json
       └─> Stored in gateway database (provider.credentials["VERTEX_ADC"])

Runtime Flow (Sandbox Startup)
  └─> Gateway reads sandbox policy oauth_credentials config
       └─> DatabaseStore.get("VERTEX_ADC") → ADC JSON
            └─> VertexProvider.get_runtime_token(store) → OAuth token
                 └─> TokenCache(auto_refresh, refresh_margin) → caches token
                      └─> Sandbox receives VERTEX_ACCESS_TOKEN env var

HTTP Request Flow (claude CLI → Vertex AI)
  └─> Proxy intercepts request to aiplatform.googleapis.com
       └─> Matches endpoint with oauth config from policy
            └─> Fetches current token from gateway TokenCache
                 └─> Injects Authorization: Bearer <token>
                      └─> Forwards to upstream with fresh token
```

**How it works:**

1. **Provider Discovery** - `openshell provider create --name vertex --type vertex --from-existing`
   - Auto-detects ADC from `~/.config/gcloud/application_default_credentials.json`
   - Stores ADC JSON in gateway database (`provider.credentials["VERTEX_ADC"]`)
   - Creates DatabaseStore wrapper around credentials HashMap

2. **Runtime Token Exchange** - When sandbox starts
   - Gateway reads sandbox policy `oauth_credentials` settings
   - DatabaseStore fetches ADC from provider.credentials
   - VertexProvider exchanges ADC for OAuth access token (valid 1 hour)
   - TokenCache caches token in memory (conditionally spawns background task)
   - Sandbox receives `VERTEX_ACCESS_TOKEN` as environment variable

3. **Auto-Refresh** - Gateway background task (policy-configured)
   - **Enabled when:** `oauth_credentials.auto_refresh: true` in sandbox policy
   - **Refresh timing:** `oauth_credentials.refresh_margin_seconds` before expiry (default: 300 = 5 min)
   - **Wake interval:** Token duration minus refresh margin (e.g., 55 min for 1-hour tokens)
   - **Updates:** Gateway TokenCache in memory (shared across all sandboxes)
   - **Disabled when:** `auto_refresh: false` or field omitted (default)

4. **OAuth Header Injection** - Proxy fetches fresh tokens on each request
   - **Configured via:** `oauth` field on endpoint in sandbox policy
   - **Example:** `oauth: {token_env_var: VERTEX_ACCESS_TOKEN, header_format: "Bearer {token}"}`
   - **Proxy behavior:** 
     1. Intercepts requests matching endpoint host/port
     2. Reads token from environment variable (initial token) OR
     3. Resolves token via SecretResolver (fetches from gateway TokenCache)
     4. Injects/replaces `Authorization: Bearer <token>` header
     5. Uses fresh token from gateway if auto-refresh enabled
   - **Key:** Proxy is responsible for fetching refreshed tokens, not sandbox
   - Generic mechanism - works for any OAuth provider (Vertex, AWS Bedrock, Azure, etc.)

**Security Model:**
- ✅ ADC stored in gateway database (encrypted at rest)
- ✅ OAuth tokens cached in memory only (cleared on restart)
- ✅ Sandboxes receive short-lived tokens (1 hour expiry)
- ✅ Tokens visible to sandbox processes but expire quickly
- ✅ Auto-refresh optional (policy-configured, disabled by default)

**Future SecretStore Implementations:**

Adding a new secret store only requires implementing the `SecretStore` trait:

```rust
#[async_trait]
pub trait SecretStore: Send + Sync {
    async fn get(&self, key: &str) -> SecretResult<String>;
    async fn health_check(&self) -> SecretResult<()>;
    fn name(&self) -> &'static str;
}
```

Planned implementations:
- 🔜 **OneCliStore** - AES-256-GCM encrypted credential gateway
- 🔜 **GcpSecretManagerStore** - team secrets in GCP
- 🔜 **VaultStore** - HashiCorp Vault integration
- 🔜 **AwsSecretsManagerStore** - AWS-native secret storage
- 🔜 **BitwardenStore** - password manager integration

**Note:** OS Keychain and GCP Workload Identity were considered but don't work for containerized gateway deployments (which is the primary use case). Network-based secret stores are the focus for future releases.

## Quick Start

### Auto-Discovery from ADC File (Recommended)

OpenShell automatically discovers your Application Default Credentials from the standard gcloud location.

**Prerequisites:**
- Google Cloud SDK (`gcloud`) installed
- Vertex AI API enabled in your GCP project

**Setup:**

```bash
# 1. Authenticate with Google Cloud
gcloud auth application-default login
# This creates: ~/.config/gcloud/application_default_credentials.json

# 2. Configure environment
export ANTHROPIC_VERTEX_PROJECT_ID=your-gcp-project-id
export ANTHROPIC_VERTEX_REGION=us-east5

# 3. Create provider (auto-discovers ADC file)
openshell provider create --name vertex --type vertex --from-existing
# ✅ Stores ADC in gateway database

# 4. Create sandbox
openshell sandbox create --name vertex-test \
  --provider vertex \
  --policy examples/vertex-ai/sandbox-policy.yaml

# 5. Inside sandbox
claude  # Automatically uses Vertex AI
```

**Complete Flow:**
```
1. Provider Discovery (openshell provider create)
   ~/.config/gcloud/application_default_credentials.json
        ↓ (auto-detected & validated)
   Gateway Database (provider.credentials["VERTEX_ADC"])

2. Sandbox Startup (openshell sandbox create)
   Sandbox requests credentials from Gateway
        ↓ (gRPC: GetSandboxProviderEnvironment with policy)
   Gateway reads oauth_credentials from sandbox policy
        ↓ (auto_refresh, refresh_margin_seconds, max_lifetime_seconds)
   Gateway exchanges ADC for OAuth token
        ↓ (POST https://oauth2.googleapis.com/token)
   Gateway creates TokenCache with policy settings
        ↓ (conditionally spawns background task if auto_refresh: true)
   Gateway sends OAuth token to Sandbox
        ↓ (VERTEX_ACCESS_TOKEN environment variable)
   Sandbox stores token in memory
        ↓ (accessible to proxy for header injection)

3. HTTP Request (claude CLI → Vertex AI)
   Claude CLI makes request to aiplatform.googleapis.com
        ↓ (HTTP/HTTPS request)
   Sandbox proxy intercepts request
        ↓ (matches endpoint host:port from policy)
   Proxy finds oauth config on endpoint
        ↓ (oauth: {token_env_var: VERTEX_ACCESS_TOKEN, header_format: "Bearer {token}"})
   Proxy fetches current token
        ↓ (tries env var first, then resolves from gateway TokenCache)
   Proxy injects Authorization header
        ↓ (Authorization: Bearer <fresh token from gateway>)
   Request forwarded to Vertex AI with real token

4. Background Refresh (if auto_refresh: true)
   Gateway TokenCache wakes up at scheduled interval
        ↓ (e.g., every 55 minutes for 1-hour tokens)
   Checks if token needs refresh (within margin of expiry)
        ↓ (e.g., 5 minutes before expiration)
   Re-exchanges ADC for fresh OAuth token
        ↓ (POST https://oauth2.googleapis.com/token)
   Updates cached token in gateway memory
        ↓ (new expiry time, e.g., +1 hour)
   Next proxy request fetches fresh token
        ↓ (proxy gets updated token from gateway TokenCache)
   Sandbox continues without restart
        ↓ (proxy handles token refresh transparently)
```

### Manual Credential Injection

If your ADC file is in a different location:

```bash
# Option 1: Set environment variable
export VERTEX_ADC="$(cat /path/to/your/adc.json)"
openshell provider create --name vertex --type vertex --from-existing

# Option 2: Inline credential
openshell provider create --name vertex --type vertex \
  --credential VERTEX_ADC="$(cat /path/to/your/adc.json)"
```

## What's Included

- **`sandbox-policy.yaml`**: Network policy allowing Google OAuth and Vertex AI endpoints
  - Supports major GCP regions (us-east5, us-central1, us-west1, europe-west1, europe-west4, asia-northeast1)
  - Enables direct Claude CLI usage
  - Enables `inference.local` routing

## Security Model

### Credential Storage

**What OpenShell stores:**
- ✅ ADC files in gateway database (encrypted at rest)
- ✅ Provider metadata (project ID, region)

**What OpenShell NEVER stores:**
- ❌ OAuth access tokens in database
- ❌ Credentials in sandboxes
- ❌ Credentials in plaintext

**OAuth tokens:**
- Generated on-demand by gateway during sandbox startup
- Valid for ~1 hour (Google's default)
- Exchanged fresh on each sandbox creation
- Never persisted to disk

**Sandboxes receive environment variables:**
```bash
# Inside sandbox environment (what processes see)
VERTEX_ADC='{"type":"...","project_id":"..."}' # ← Full ADC JSON (for Claude CLI to write to file)
VERTEX_ACCESS_TOKEN=ya29.c.a0Aa...            # ← OAuth token (for proxy header injection)
ANTHROPIC_VERTEX_PROJECT_ID=your-project      # ← Public metadata (direct value)
ANTHROPIC_VERTEX_REGION=us-east5              # ← Public metadata (direct value)
CLAUDE_CODE_USE_VERTEX=1                      # ← Boolean flag (direct value)
```

**Security considerations:**
- `VERTEX_ADC`: Full ADC JSON visible to all processes (needed for Claude CLI auto-detection)
- `VERTEX_ACCESS_TOKEN`: OAuth token visible to all processes (short-lived, 1 hour expiry)
- Both are injected by gateway at sandbox startup, cleared when sandbox terminates
- OAuth tokens are refreshed in background when `oauth_credentials.auto_refresh: true`

**On every HTTP request:**
1. OpenShell proxy intercepts request to `aiplatform.googleapis.com`
2. Matches endpoint configuration from policy (host:port)
3. Finds `oauth` config: `{token_env_var: VERTEX_ACCESS_TOKEN, header_format: "Bearer {token}"}`
4. **Proxy fetches current token:**
   - First tries environment variable: `$VERTEX_ACCESS_TOKEN` (initial token)
   - If auto-refresh enabled: resolves via SecretResolver (fetches from gateway TokenCache)
   - Gets fresh token even after background refresh (no sandbox restart needed)
5. Injects/replaces `Authorization` header: `Authorization: Bearer ya29.c.a0Aa...`
6. Forwards request to Vertex AI with real OAuth token

**Benefits:**
- **Proxy-driven refresh:** Proxy fetches fresh tokens from gateway on each request
- **No sandbox restart:** Background refresh updates gateway cache, proxy fetches automatically
- **Short-lived exposure:** Initial token in environment variable, but expires in 1 hour
- **Centralized management:** Gateway TokenCache manages refresh, sandboxes just consume
- **Secure storage:** ADC stored in gateway database (never exposed to untrusted networks)
- **Generic mechanism:** Works for any OAuth provider (AWS Bedrock, Azure OpenAI, etc.)

### Token Auto-Refresh

**By default**, OAuth tokens are **NOT** auto-refreshed. Sandboxes must restart after ~1 hour when tokens expire.

**For long-running sandboxes**, enable auto-refresh in the **sandbox policy**:

```yaml
# examples/vertex-ai/sandbox-policy.yaml
version: 1

# OAuth credential auto-refresh configuration
oauth_credentials:
  auto_refresh: true              # Enable automatic token refresh (default: false)
  refresh_margin_seconds: 300     # Refresh 5 minutes before expiry (default: 300)
  max_lifetime_seconds: 7200      # Maximum sandbox lifetime: 2 hours (default: 86400 = 24h, -1 = infinite)

network_policies:
  # ... rest of policy
```

**How it works:**
- Gateway reads `oauth_credentials` from sandbox policy at startup
- Creates TokenCache with configured settings in gateway memory
- Conditionally spawns background task only when `auto_refresh: true`
- Background task wakes up at `token_duration - refresh_margin_seconds` (e.g., 55 min for 1-hour tokens)
- Refreshes tokens proactively before expiration
- Updates TokenCache in gateway memory (shared across sandboxes)
- **Key:** Proxy fetches fresh token from gateway on each request (via SecretResolver)
- Sandbox receives initial token as environment variable at startup
- No sandbox restart needed - proxy transparently uses refreshed tokens

**Configuration options:**

| Field | Default | Description |
|-------|---------|-------------|
| `auto_refresh` | `false` | **Must be explicitly enabled.** Allows sandboxes to run longer than token lifetime. |
| `refresh_margin_seconds` | `300` | Refresh tokens 5 minutes before expiry. |
| `max_lifetime_seconds` | `86400` | Maximum sandbox lifetime. `-1` = infinite, `0` = 24h default, `>0` = custom. |

**How gateway auto-refresh works:**

**Without auto-refresh (default):**
```
T+0:00 - Sandbox starts → Gateway exchanges ADC for OAuth token
         ↓ (token valid for ~1 hour, cached in gateway TokenCache)
T+0:00 - Sandbox receives VERTEX_ACCESS_TOKEN environment variable (initial token)
T+0:30 - HTTP request → Proxy fetches token from env var
         ↓ (injects Authorization: Bearer <token>)
T+1:00 - Token expires in gateway cache
T+1:01 - HTTP request → Proxy fetches expired token
         ↓ (HTTP 401 Unauthorized from Vertex AI)
         ↓ (sandbox must be restarted to get fresh token)
```

**With auto-refresh enabled (`oauth_credentials.auto_refresh: true`):**
```
T+0:00 - Sandbox starts → Gateway exchanges ADC for OAuth token
         ↓ (token valid for ~1 hour, background task spawned in gateway)
T+0:00 - Sandbox receives VERTEX_ACCESS_TOKEN environment variable (initial token)
T+0:30 - HTTP request → Proxy fetches token from env var
         ↓ (injects Authorization: Bearer <token>)
T+0:55 - Gateway background refresh → Exchanges ADC for new token
         ↓ (new token valid until T+1:55, updates gateway TokenCache)
T+1:00 - HTTP request → Proxy resolves token via SecretResolver
         ↓ (fetches fresh token from gateway TokenCache)
         ↓ (injects Authorization: Bearer <refreshed-token>)
         ↓ (seamless, no restart needed)
T+1:50 - Gateway background refresh → Exchanges for new token again
         ↓ (continues until max_lifetime_seconds reached)
T+2:00 - Sandbox reaches max_lifetime (if configured) → self-terminates
```

**Features:**

- ✅ **Gateway-side refresh:** TokenCache in gateway refreshes tokens in background
- ✅ **Proxy-driven fetch:** Proxy fetches fresh token from gateway on each request
- ✅ **Auto-refresh:** Background task spawned when `auto_refresh: true` in policy
- ✅ **Configurable timing:** `refresh_margin_seconds` (default: 300 = 5 min)
- ✅ **Lifetime limits:** `max_lifetime_seconds` (default: 86400 = 24h, -1 = infinite)
- ✅ **No restarts:** Proxy transparently uses refreshed tokens, no sandbox restart
- ✅ **Seamless updates:** Refresh happens before expiry, no service interruption

## GKE Deployment

### 1. Create GCP Service Account

```bash
# Create service account for OpenShell gateway
gcloud iam service-accounts create openshell-gateway \
  --project=$ANTHROPIC_VERTEX_PROJECT_ID \
  --display-name="OpenShell Gateway"

# Grant Vertex AI permissions
gcloud projects add-iam-policy-binding $ANTHROPIC_VERTEX_PROJECT_ID \
  --member="serviceAccount:openshell-gateway@${ANTHROPIC_VERTEX_PROJECT_ID}.iam.gserviceaccount.com" \
  --role="roles/aiplatform.user"
```

### 2. Configure Workload Identity

```bash
# Link Kubernetes SA to GCP SA
gcloud iam service-accounts add-iam-policy-binding \
  openshell-gateway@${ANTHROPIC_VERTEX_PROJECT_ID}.iam.gserviceaccount.com \
  --role roles/iam.workloadIdentityUser \
  --member "serviceAccount:${ANTHROPIC_VERTEX_PROJECT_ID}.svc.id.goog[openshell/openshell-gateway]"
```

### 3. Deploy Gateway

```yaml
# gateway-deployment.yaml
apiVersion: v1
kind: ServiceAccount
metadata:
  name: openshell-gateway
  namespace: openshell
  annotations:
    iam.gke.io/gcp-service-account: openshell-gateway@YOUR_PROJECT.iam.gserviceaccount.com
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: openshell-gateway
  namespace: openshell
spec:
  template:
    spec:
      serviceAccountName: openshell-gateway
      containers:
      - name: gateway
        image: quay.io/itdove/gateway:dev
        env:
        - name: ANTHROPIC_VERTEX_PROJECT_ID
          value: "your-gcp-project-id"
        - name: ANTHROPIC_VERTEX_REGION
          value: "us-east5"
```

```bash
kubectl apply -f gateway-deployment.yaml
```

### 4. Verify Workload Identity

```bash
# Check that gateway can access GCP metadata service
kubectl exec -n openshell deployment/openshell-gateway -- \
  curl -H "Metadata-Flavor: Google" \
  http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token

# Should return:
# {"access_token":"ya29.xxx","expires_in":3600,"token_type":"Bearer"}
```

## Advanced Configuration

### Token Exchange On Demand

OAuth tokens are exchanged fresh on each sandbox startup. This means:

- **Short-lived credentials:** Tokens expire in ~1 hour
- **No background refresh:** Gateway exchanges tokens synchronously
- **Automatic retry:** Sandbox restart gets fresh token automatically
- **Network required:** Token exchange requires internet access during sandbox startup

**For production deployments:**

Consider using short-lived sandboxes (< 1 hour) to minimize credential exposure. This aligns with security best practices and ensures tokens never expire during active sessions.

**For development workflows:**

Long-running sandboxes (> 1 hour) will require restart to refresh tokens. Use `openshell sandbox restart <name>` when you see 401 Unauthorized errors.

### Multiple Credential Storage (Future)

**Current implementation:**

ADC credentials are stored in the OpenShell gateway database.

**Future feature - pluggable secret stores:**

Support for external secret management:

1. **GCP Secret Manager** - Team secrets (future)
2. **HashiCorp Vault** - Multi-cloud (future)
3. **GKE Workload Identity** - Keyless authentication (future)
4. **AWS Secrets Manager** - AWS deployments (future)

These will allow enterprise deployments to avoid storing credentials in the OpenShell database entirely.

## Troubleshooting

### "ADC credentials rejected by Google OAuth" errors

**Cause:** ADC credentials have expired or been revoked.

Google Application Default Credentials (ADC) can expire after extended periods of inactivity (typically months). When this happens, token exchange will fail.

**Solution:**

```bash
# Re-authenticate with Google Cloud
gcloud auth application-default login

# Update the provider with fresh credentials
openshell provider create --name vertex --type vertex --from-existing

# Or delete and recreate
openshell provider delete vertex
openshell provider create --name vertex --type vertex --from-existing
```

**How to tell if credentials are expired:**
- Provider creation succeeds but sandbox requests fail with "invalid_grant"
- Error message: "ADC credentials rejected by Google OAuth (status 400)"

**Prevention:**
- Credentials are validated when you create the provider
- If credentials expire later (days/weeks/months), re-run `gcloud auth application-default login`

### "Vertex ADC credentials not found" errors

**Cause:** No ADC file found during provider creation.

**Solution:**

```bash
# Generate ADC file
gcloud auth application-default login

# Verify it was created
ls ~/.config/gcloud/application_default_credentials.json

# Create provider
openshell provider create --name vertex --type vertex --from-existing
```

### "Authentication failed" errors (GKE/Cloud Run)

**Cause:** Gateway cannot fetch tokens from GCP metadata service.

**Solution:**

1. **Verify Workload Identity is configured:**
   ```bash
   kubectl get sa openshell-gateway -n openshell -o yaml | grep iam.gke.io
   # Should show: iam.gke.io/gcp-service-account: openshell-gateway@PROJECT.iam.gserviceaccount.com
   ```

2. **Check gateway can access metadata service:**
   ```bash
   kubectl exec -n openshell deployment/openshell-gateway -- \
     curl -H "Metadata-Flavor: Google" \
     http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token
   ```

3. **Verify GCP service account has permissions:**
   ```bash
   gcloud projects get-iam-policy $ANTHROPIC_VERTEX_PROJECT_ID \
     --flatten="bindings[].members" \
     --filter="bindings.members:serviceAccount:openshell-gateway@*"
   # Should show: roles/aiplatform.user
   ```

4. **Check gateway logs:**
   ```bash
   kubectl logs -n openshell deployment/openshell-gateway | grep -i "credential\|token\|workload"
   ```

### "Project not found" errors

**Cause:** Invalid or inaccessible GCP project ID.

**Solution:**

1. Verify project exists and you have access:
   ```bash
   gcloud projects describe $ANTHROPIC_VERTEX_PROJECT_ID
   ```

2. Check Vertex AI API is enabled:
   ```bash
   gcloud services list --enabled --project=$ANTHROPIC_VERTEX_PROJECT_ID | grep aiplatform
   ```

3. Enable if needed:
   ```bash
   gcloud services enable aiplatform.googleapis.com --project=$ANTHROPIC_VERTEX_PROJECT_ID
   ```

### "Region not supported" errors

**Cause:** Vertex AI endpoint for your region not in network policy.

**Solution:** Add region to `sandbox-policy.yaml`:

```yaml
- host: your-region-aiplatform.googleapis.com
  port: 443
  protocol: rest
  access: full
  oauth:
    token_env_var: VERTEX_ACCESS_TOKEN
    header_format: "Bearer {token}"
```

Supported regions: us-central1, us-east5, us-west1, europe-west1, europe-west4, asia-northeast1, asia-southeast1

### Tokens not refreshing

**Cause:** Auto-refresh not enabled in sandbox policy, or background task failing.

**Solution:**

1. **Verify auto-refresh is enabled in sandbox policy:**
   ```yaml
   # sandbox-policy.yaml must have:
   oauth_credentials:
     auto_refresh: true              # Required for background refresh
     refresh_margin_seconds: 300     # Optional (default: 300)
   ```

2. **Check gateway logs for background refresh:**
   ```bash
   # Gateway logs should show (only when auto_refresh: true):
   # "background refresh triggered"
   # "background refresh succeeded"
   kubectl logs -n openshell deployment/openshell-gateway | grep "refresh"
   
   # If you see "Auto-refresh disabled for token cache", check your policy
   ```

3. **Verify no network issues:**
   ```bash
   # Test OAuth endpoint from gateway pod
   kubectl exec -n openshell deployment/openshell-gateway -- \
     curl -v https://oauth2.googleapis.com/token
   ```

4. **Check for errors in logs:**
   ```bash
   kubectl logs -n openshell deployment/openshell-gateway | grep -i "refresh failed"
   ```

## Documentation

For detailed setup instructions and configuration options, see:

- [Credential Provider Plugin Architecture](../../credential-provider-plugin-architecture.md)
- [Provider Management](../../docs/sandboxes/manage-providers.md)
- [Inference Routing](../../docs/inference/configure.md)

## Architecture

### Two-Layer Plugin System

```
┌─────────────────────────────────────────────────────────────┐
│ Layer 1: Secret Store (where credentials live)             │
│                                                             │
│  ┌───────────────┐ ┌────────────────┐ ┌─────────────────┐ │
│  │ OS Keychain   │ │ Workload       │ │ GCP Secret      │ │
│  │ macOS/Linux/  │ │ Identity       │ │ Manager         │ │
│  │ Windows       │ │ (GKE metadata) │ │ (team secrets)  │ │
│  └───────────────┘ └────────────────┘ └─────────────────┘ │
│         │                  │                    │          │
│         └──────────────────┴────────────────────┘          │
│                            │                                │
│                    SecretStore trait                        │
│                   (generic get/health_check)                │
└─────────────────────────────┬───────────────────────────────┘
                              │ Raw secret string
                              ▼
┌─────────────────────────────────────────────────────────────┐
│ Layer 2: Provider Plugin (how to interpret credentials)    │
│                                                             │
│  ┌──────────────────────────────────────────────────────┐  │
│  │ VertexProvider                                       │  │
│  │   - Reads ADC JSON from store                        │  │
│  │   - Exchanges for OAuth token                        │  │
│  │   - Knows Google OAuth endpoint                      │  │
│  └──────────────────────────────────────────────────────┘  │
│                              │                              │
│                   ProviderPlugin trait                      │
│                 (get_runtime_token method)                  │
└─────────────────────────────┬───────────────────────────────┘
                              │ TokenResponse
                              ▼
┌─────────────────────────────────────────────────────────────┐
│ TokenCache (policy-configured, in gateway)                  │
│   - Caches tokens in memory (~1 hour)                       │
│   - Conditionally spawns background task                    │
│   - Config: oauth_credentials from sandbox policy           │
│   - Wraps: ProviderPlugin + SecretStore                     │
│   - Background refresh updates cache every 55 min           │
└─────────────────────────────┬───────────────────────────────┘
                              │ Initial token at startup
                              ▼
┌─────────────────────────────────────────────────────────────┐
│ Sandbox Environment                                         │
│   VERTEX_ACCESS_TOKEN=ya29.c.a0Aa... (initial token)       │
└─────────────────────────────┬───────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│ OpenShell Proxy (L7 HTTP Inspection)                        │
│   1. Intercepts HTTP to aiplatform.googleapis.com          │
│   2. Matches endpoint with oauth config from policy         │
│   3. Fetches token:                                         │
│      - First: tries $VERTEX_ACCESS_TOKEN (env var)          │
│      - Then: resolves via SecretResolver (from gateway)     │
│   4. Gets fresh token from gateway TokenCache               │
│   5. Injects Authorization: Bearer <token>                  │
│   6. Forwards to Vertex AI                                  │
└─────────────────────────────┬───────────────────────────────┘
                              │ HTTP with real token
                              ▼
                    Vertex AI Endpoint
```

### Local Development Flow (OS Keychain)

```
macOS Keychain                      OpenShell Gateway
    │                                     │
    │ 1. OsKeychainStore.get("vertex")   │
    ├───────────────────────────────────>│
    │                                     │
    │ 2. Returns: ADC JSON                │
    │<───────────────────────────────────┤
    │                                     │
                                          │ 3. VertexProvider.get_runtime_token(adc)
                                          ├────────────────────────────────>
                                          │                                 │
                                          │                          Google OAuth
                                          │                                 │
                                          │ 4. Returns: OAuth token         │
                                          │<────────────────────────────────┤
                                          │
                                          │ 5. TokenCache stores + returns
                                          │
                                    Sandbox gets token
```

### Production Flow (Workload Identity)

```
GCP Metadata Service                OpenShell Gateway
    │                                     │
    │ 1. WorkloadIdentityStore.get()     │
    ├───────────────────────────────────>│
    │                                     │
    │ 2. Returns: OAuth token (JSON)      │
    │<───────────────────────────────────┤
    │                                     │
                                          │ 3. VertexProvider.get_runtime_token()
                                          │    Detects Workload Identity format
                                          │    Returns token directly (no exchange)
                                          │
                                          │ 4. TokenCache stores + returns
                                          │
                                    Sandbox gets token
```

## Migration from ADC Upload Approach

**Old approach (deprecated):**
```bash
# DON'T DO THIS - old method
openshell sandbox create --provider vertex \
  --upload ~/.config/gcloud/:.config/gcloud/  # ❌ No longer needed
```

**New approach:**
```bash
# DO THIS - credential provider plugins
openshell sandbox create --provider vertex  # ✅ No upload flag
```

**Why the change:**
- ❌ Old: ADC credentials stored in sandbox filesystem
- ✅ New: Only short-lived OAuth tokens (1 hour expiry)
- ❌ Old: Manual token refresh needed (restart sandbox)
- ✅ New: Optional automatic background refresh (policy-configured)
- ❌ Old: Each sandbox manages tokens independently
- ✅ New: Centralized token management at gateway
- ❌ Old: Compromised sandbox = compromised long-lived credentials
- ✅ New: Compromised sandbox = short-lived token (max 1 hour)

**If you're using the old approach:**
1. Remove `--upload ~/.config/gcloud/` from sandbox creation
2. Deploy gateway with Workload Identity (see GKE Deployment section)
3. Existing sandboxes will continue to work until recreated
