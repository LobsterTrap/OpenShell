# Google Cloud Vertex AI Example

This example demonstrates how to use OpenShell with Google Cloud Vertex AI to run Claude models via GCP infrastructure.

## ⚠️ Critical Requirement

Vertex AI sandboxes **MUST** upload GCP credentials to generate OAuth tokens:

```bash
--upload ~/.config/gcloud/:.config/gcloud/
```

Without this upload, token generation will fail and sandboxes cannot connect to Vertex AI.

## Quick Start

```bash
# 1. Configure GCP credentials
export ANTHROPIC_VERTEX_PROJECT_ID=your-gcp-project-id
gcloud auth application-default login

# 2. Create provider
openshell provider create --name vertex --type vertex --from-existing

# 3. Create sandbox with credentials uploaded
openshell sandbox create --name vertex-test --provider vertex \
  --upload ~/.config/gcloud/:.config/gcloud/ \  # ← REQUIRED
  --policy examples/vertex-ai/sandbox-policy.yaml

# 4. Inside sandbox
claude  # Automatically uses Vertex AI
```

## What's Included

- **`sandbox-policy.yaml`**: Network policy allowing Google OAuth and Vertex AI endpoints
  - Supports major GCP regions (us-east5, us-central1, us-west1, europe-west1, europe-west4, asia-northeast1)
  - Enables direct Claude CLI usage
  - Enables `inference.local` routing

## Security Model

### Credential Injection

Vertex AI uses selective credential injection for CLI tool compatibility:

**Directly injected (visible in `/proc/<pid>/environ`):**
- `ANTHROPIC_VERTEX_PROJECT_ID` - Not sensitive (public project ID, visible in API URLs)
- `CLAUDE_CODE_USE_VERTEX` - Configuration flag (boolean)
- `ANTHROPIC_VERTEX_REGION` - Public metadata (region name)

**Generated in sandbox (not stored in gateway database):**
- OAuth access tokens - Generated on-demand from uploaded ADC file, automatically refreshed

**Trade-off:** Direct injection required for Claude CLI compatibility (cannot use HTTP proxy placeholders). Risk is low since no secrets are exposed via environment variables.

## Troubleshooting

### "Authentication failed" or "invalid credentials"

**Cause:** Sandbox cannot generate OAuth tokens (ADC file not uploaded or missing).

**Solution:**
1. Verify ADC exists on host:
   ```bash
   ls -la ~/.config/gcloud/application_default_credentials.json
   ```

2. If missing, configure ADC:
   ```bash
   gcloud auth application-default login
   ```

3. Ensure sandbox creation includes upload:
   ```bash
   openshell sandbox create --provider vertex \
     --upload ~/.config/gcloud/:.config/gcloud/  # ← Required
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

## Documentation

For detailed setup instructions and configuration options, see:

- [Vertex AI Provider Configuration](../../docs/inference/configure.md#google-cloud-vertex-ai)
- [Provider Management](../../docs/sandboxes/manage-providers.md)
- [Inference Routing](../../docs/inference/configure.md)

## Adding Regions

To support additional GCP regions, add them to `sandbox-policy.yaml`:

```yaml
- host: asia-southeast1-aiplatform.googleapis.com
  port: 443
```
