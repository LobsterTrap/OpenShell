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
- Auto-refreshes every 55 minutes (for 1-hour tokens)

### Current Implementation

```
Provider Discovery
  └─> ~/.config/gcloud/application_default_credentials.json
       └─> Stored in gateway database (provider.credentials["VERTEX_ADC"])

Runtime Flow
  └─> DatabaseStore.get("VERTEX_ADC") → ADC JSON
       └─> VertexProvider.get_runtime_token(store) → exchanges for OAuth
            └─> TokenCache → caches + auto-refreshes
                 └─> Sandbox → gets placeholder, proxy injects real token
```

**How it works:**

1. **Provider Discovery** - `openshell provider create --name vertex --type vertex --from-existing`
   - Auto-detects ADC from `~/.config/gcloud/application_default_credentials.json`
   - Stores ADC JSON in gateway database (`provider.credentials["VERTEX_ADC"]`)
   - Creates DatabaseStore wrapper around credentials HashMap

2. **Runtime Token Exchange** - When sandbox makes a request
   - DatabaseStore fetches ADC from provider.credentials
   - VertexProvider exchanges ADC for OAuth access token (valid 1 hour)
   - TokenCache caches token in memory with auto-refresh at 55 min mark
   - Proxy injects fresh token into outbound request

3. **Auto-Refresh** - Background task
   - Wakes up every 55 minutes (token duration - refresh margin)
   - Proactively refreshes tokens 5 minutes before expiration
   - Sandboxes work indefinitely without manual intervention

**Security Model:**
- ✅ ADC stored in gateway database (encrypted at rest)
- ✅ OAuth tokens cached in memory only (cleared on restart)
- ✅ Sandboxes receive placeholders, never real tokens
- ✅ Tokens expire in 1 hour (short-lived)
- ✅ Auto-refresh prevents expiration during long sessions

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

# 3a. (Optional) Enable auto-refresh for long-running sandboxes
openshell provider update vertex \
  --config auto_refresh=true \
  --config max_lifetime_seconds=7200  # 2 hours

# 4. Create sandbox
openshell sandbox create --name vertex-test \
  --provider vertex \
  --policy examples/vertex-ai/sandbox-policy.yaml

# 5. Inside sandbox
claude  # Automatically uses Vertex AI
```

**How it works:**
```
1. Provider Discovery (openshell provider create)
   ~/.config/gcloud/application_default_credentials.json
        ↓ (auto-detected & validated)
   Gateway Database (provider.credentials["VERTEX_ADC"])

2. Sandbox Startup (openshell sandbox create)
   Sandbox requests credentials from Gateway
        ↓ (gRPC: GetSandboxProviderEnvironment)
   Gateway exchanges ADC for OAuth token
        ↓ (POST https://oauth2.googleapis.com/token)
   Gateway sends OAuth token to Sandbox
        ↓ (valid for ~1 hour)
   Sandbox stores token as placeholder
        ↓ (VERTEX_ADC=openshell:resolve:env:VERTEX_ADC)

3. HTTP Request (claude CLI → Vertex AI)
   Sandbox proxy intercepts HTTP request
        ↓ (detects placeholder in headers)
   Proxy resolves placeholder to OAuth token
        ↓ (from memory, received at startup)
   Request forwarded to Vertex AI with real token
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

**Sandboxes receive placeholders:**
```bash
# Inside sandbox environment (what processes see)
VERTEX_ADC=openshell:resolve:env:VERTEX_ADC  # ← Placeholder (resolved by proxy)
ANTHROPIC_VERTEX_PROJECT_ID=your-project      # ← Public metadata (direct value)
ANTHROPIC_VERTEX_REGION=us-east5              # ← Public metadata (direct value)
CLAUDE_CODE_USE_VERTEX=1                      # ← Boolean flag (direct value)
```

**On every HTTP request:**
1. OpenShell proxy intercepts request
2. Detects placeholder: `openshell:resolve:env:VERTEX_ADC`
3. Resolves placeholder to OAuth token (received at sandbox startup)
4. Proxy replaces placeholder with real OAuth token
5. Request forwarded to Vertex AI

**Benefits:**
- Even if sandbox process is compromised, attacker only sees placeholder
- Even if proxy memory is dumped, tokens expire in 1 hour
- No long-lived credentials stored in sandbox
- GCP can revoke access instantly (just update IAM)
- Sandboxes automatically get fresh tokens on each restart

### Token Auto-Refresh

**By default**, OAuth tokens are refreshed in the gateway but sandboxes must restart after ~1 hour when tokens expire.

**For long-running sandboxes**, enable auto-refresh:

```bash
# Enable auto-refresh when creating provider
openshell provider create --name vertex --type vertex --from-existing \
  --config auto_refresh=true \
  --config refresh_margin_seconds=300 \
  --config max_lifetime_seconds=7200  # 2 hours

# Or update existing provider
openshell provider update vertex \
  --config auto_refresh=true \
  --config max_lifetime_seconds=86400  # 24 hours
```

**Configuration options:**

| Field | Default | Description |
|-------|---------|-------------|
| `auto_refresh` | `false` | **Must be explicitly enabled.** Allows sandboxes to run longer than token lifetime. |
| `refresh_margin_seconds` | `300` | Refresh tokens 5 minutes before expiry. |
| `max_lifetime_seconds` | `86400` | Maximum sandbox lifetime. `-1` = infinite, `0` = 24h default, `>0` = custom. |

**How gateway auto-refresh works:**

```
T+0:00 - Sandbox starts → Gateway exchanges ADC for OAuth token
         ↓ (token valid for ~1 hour, cached in gateway)
T+0:00 - Sandbox receives OAuth token in VERTEX_ADC placeholder
T+0:30 - HTTP requests → Proxy resolves placeholder to cached OAuth token
T+0:55 - Background refresh → Gateway exchanges for new token proactively
         ↓ (new token valid until T+1:55, old token still valid until T+1:00)
T+1:00 - HTTP requests → Proxy uses refreshed token (seamless for gateway)
T+1:50 - Background refresh → Gateway refreshes again
         ↓ (continues indefinitely)
```

**Current limitations:**

- ✅ Gateway caches and auto-refreshes tokens every 55 minutes
- ✅ All sandboxes using same provider share the same TokenCache
- ⏳ **Sandbox-side refresh not yet implemented** - sandboxes receive initial token only
- ⏳ Long-running sandboxes (>1 hour) will fail after initial token expires

**When sandbox refresh is implemented (planned):**

- ✅ No sandbox restarts required - tokens refresh automatically in sandbox too
- ✅ No service interruption - refresh happens 5 minutes before expiry
- ✅ Long-running sandboxes work up to `max_lifetime_seconds`
- ✅ Sandboxes self-terminate when max lifetime is reached (prevents infinite sandboxes)

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
```

Supported regions: us-central1, us-east5, us-west1, europe-west1, europe-west4, asia-northeast1, asia-southeast1

### Tokens not refreshing

**Cause:** Background refresh task not running or failing.

**Solution:**

1. **Check TokenCache is enabled:**
   ```bash
   # Gateway logs should show:
   # "background refresh triggered"
   # "background refresh succeeded"
   kubectl logs -n openshell deployment/openshell-gateway | grep "refresh"
   ```

2. **Verify no network issues:**
   ```bash
   # Test metadata service from gateway pod
   kubectl exec -n openshell deployment/openshell-gateway -- \
     curl -v -H "Metadata-Flavor: Google" \
     http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token
   ```

3. **Check for errors in logs:**
   ```bash
   kubectl logs -n openshell deployment/openshell-gateway | grep -i error
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
│ TokenCache                                                  │
│   - Caches tokens (1 hour)                                  │
│   - Auto-refreshes at ~55 min mark                          │
│   - Background refresh task                                 │
│   - Wraps: ProviderPlugin + SecretStore                     │
└─────────────────────────────┬───────────────────────────────┘
                              │ Fresh token
                              ▼
┌─────────────────────────────────────────────────────────────┐
│ OpenShell Proxy                                             │
│   1. Detects placeholder: openshell:resolve:env:X           │
│   2. Calls TokenCache.get_token("vertex")                   │
│   3. Gets fresh token (cached, auto-refreshed)              │
│   4. Replaces placeholder with real token                   │
│   5. Forwards to Vertex AI                                  │
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
- ❌ Old: Credentials stored in sandbox filesystem
- ✅ New: No credentials in sandbox (only placeholders)
- ❌ Old: Manual token refresh needed
- ✅ New: Automatic background refresh
- ❌ Old: Each sandbox manages tokens independently
- ✅ New: Centralized token management at gateway
- ❌ Old: Compromised sandbox = compromised credentials
- ✅ New: Compromised sandbox = only has placeholder

**If you're using the old approach:**
1. Remove `--upload ~/.config/gcloud/` from sandbox creation
2. Deploy gateway with Workload Identity (see GKE Deployment section)
3. Existing sandboxes will continue to work until recreated
