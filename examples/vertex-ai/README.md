# Google Cloud Vertex AI Example

This example demonstrates how to use OpenShell with Google Cloud Vertex AI to run Claude models via GCP infrastructure.

## Quick Start

```bash
# Configure GCP credentials
export ANTHROPIC_VERTEX_PROJECT_ID=your-gcp-project-id
gcloud auth application-default login

# Create provider
openshell provider create --name vertex --type vertex --from-existing

# Create sandbox with policy
openshell sandbox create --name vertex-test --provider vertex \
  --upload ~/.config/gcloud/:.config/gcloud/ \
  --policy examples/vertex-ai/sandbox-policy.yaml

# Inside sandbox
claude  # Automatically uses Vertex AI
```

## What's Included

- **`sandbox-policy.yaml`**: Network policy allowing Google OAuth and Vertex AI endpoints
  - Supports major GCP regions (us-east5, us-central1, us-west1, europe-west1, europe-west4, asia-northeast1)
  - Enables direct Claude CLI usage
  - Enables `inference.local` routing

## Documentation

For detailed setup instructions, troubleshooting, and configuration options, see:

- [Vertex AI Provider Configuration](../../docs/inference/configure.md#google-cloud-vertex-ai)
- [Provider Management](../../docs/sandboxes/manage-providers.md)
- [Inference Routing](../../docs/inference/configure.md)

## Adding Regions

To support additional GCP regions, add them to `sandbox-policy.yaml`:

```yaml
- host: asia-southeast1-aiplatform.googleapis.com
  port: 443
```
