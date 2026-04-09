# Adding OAuth Auto-Refresh Support for New Providers

The OpenShell gateway includes a generic OAuth token auto-refresh system that works for any provider implementing the `ProviderPlugin` trait with `get_runtime_token()`.

## Current Supported Providers

- **Vertex AI** (`vertex`): VERTEX_ADC → Google OAuth token exchange

## Adding a New OAuth Provider

### 1. Implement ProviderPlugin

Create your provider in `crates/openshell-providers/src/providers/`:

```rust
// crates/openshell-providers/src/providers/my_oauth_provider.rs
use crate::{ProviderPlugin, SecretStore, TokenResponse, RuntimeResult};
use async_trait::async_trait;

pub struct MyOAuthProvider {
    client: reqwest::Client,
}

impl MyOAuthProvider {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl ProviderPlugin for MyOAuthProvider {
    fn id(&self) -> &'static str {
        "my-oauth-provider"
    }

    fn discover_existing(&self) -> Result<Option<DiscoveredProvider>, ProviderError> {
        // Auto-discover credentials from environment/filesystem
        // Store credentials in provider.credentials HashMap
        Ok(None)
    }

    async fn get_runtime_token(&self, store: &dyn SecretStore) -> RuntimeResult<TokenResponse> {
        // Fetch credential from store
        let credential = store.get("MY_OAUTH_CREDENTIAL").await?;

        // Exchange for OAuth token (e.g., AWS STS, Azure AD, etc.)
        let token = self.exchange_for_token(&credential).await?;

        Ok(TokenResponse {
            access_token: token,
            token_type: "Bearer".to_string(),
            expires_in: 3600, // 1 hour
            metadata: HashMap::new(),
        })
    }
}
```

### 2. Register Provider in Registry

Add to `crates/openshell-providers/src/lib.rs`:

```rust
impl ProviderRegistry {
    pub fn new() -> Self {
        let mut registry = Self::default();
        // ... existing providers
        registry.register(providers::my_oauth_provider::MyOAuthProvider::new());
        registry
    }
}
```

### 3. Enable TokenCache for Your Provider

Update `crates/openshell-server/src/grpc.rs`:

**Step 3a:** Add to `should_use_token_cache()`:

```rust
fn should_use_token_cache(provider_type: &str, credential_key: &str) -> bool {
    matches!(
        (provider_type, credential_key),
        ("vertex", "VERTEX_ADC")
        | ("my-oauth-provider", "MY_OAUTH_CREDENTIAL") // ← Add this line
    )
}
```

**Step 3b:** Add to `get_or_create_token_cache()`:

```rust
let provider_plugin: Arc<dyn ProviderPlugin> = match provider_type {
    "vertex" => {
        let _: serde_json::Value = serde_json::from_str(credential_value)?;
        Arc::new(openshell_providers::vertex::VertexProvider::new())
    }
    "my-oauth-provider" => {
        // Validate credential format if needed
        Arc::new(openshell_providers::my_oauth_provider::MyOAuthProvider::new())
    }
    _ => {
        return Err(format!("Unsupported OAuth provider type: {provider_type}"));
    }
};
```

### 4. Export Provider Module

Add to `crates/openshell-providers/src/lib.rs`:

```rust
pub mod my_oauth_provider {
    pub use crate::providers::my_oauth_provider::*;
}
```

### 5. Configure OAuth Header Injection

Add OAuth header injection to your sandbox policy for endpoints that require it:

```yaml
# sandbox-policy.yaml
version: 1

oauth_credentials:
  auto_refresh: true
  refresh_margin_seconds: 300

network_policies:
  my_oauth_api:
    name: my-oauth-api
    endpoints:
      - host: api.my-oauth-service.com
        port: 443
        protocol: rest        # Required for OAuth injection
        access: full
        oauth:
          token_env_var: MY_OAUTH_TOKEN    # Matches provider credential key
          header_format: "Bearer {token}"  # Or custom format
```

The `token_env_var` must match the credential key stored by your provider (e.g., `MY_OAUTH_CREDENTIAL` → token cached as `MY_OAUTH_TOKEN`).

## OAuth Configuration

OAuth auto-refresh behavior is configured **in the sandbox policy**, not at provider creation time. Provider creation is only for storing credentials.

### Sandbox Policy Configuration

Configure OAuth auto-refresh in your sandbox policy:

```yaml
# sandbox-policy.yaml
version: 1

# OAuth credential auto-refresh configuration
oauth_credentials:
  auto_refresh: true              # Enable automatic token refresh
  refresh_margin_seconds: 300     # Refresh 5 minutes before expiry
  max_lifetime_seconds: 7200      # Maximum sandbox lifetime: 2 hours
```

### Configuration Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `auto_refresh` | bool | `false` | Enable automatic token refresh. **Must be explicitly enabled for security.** |
| `refresh_margin_seconds` | int64 | `300` | Refresh tokens this many seconds before expiry (e.g., 300 = 5 minutes). |
| `max_lifetime_seconds` | int64 | `86400` | Maximum sandbox lifetime in seconds. `-1` = infinite, `0` or unspecified = 24 hours, `>0` = custom limit. |

**Security defaults:**
- `auto_refresh: false` - Disabled by default. Sandboxes must be explicitly configured for long-running operation.
- `max_lifetime_seconds: 86400` - 24-hour default limit prevents infinite-running sandboxes.

### Provider Creation

When creating a provider, only store the OAuth credential:

```bash
openshell provider create vertex \
  --type vertex \
  --credential VERTEX_ADC=/path/to/adc.json
```

Auto-refresh configuration is handled in the sandbox policy, not at provider creation time.

## OAuth Header Injection Configuration

Configure automatic OAuth token injection for specific endpoints in your sandbox policy:

```yaml
network_policies:
  my_api:
    name: my-api
    endpoints:
      - host: api.example.com
        port: 443
        protocol: rest        # Enable L7 HTTP inspection
        access: full          # Or use explicit rules
        oauth:
          token_env_var: MY_OAUTH_TOKEN    # Environment variable containing token
          header_format: "Bearer {token}"  # Authorization header format
```

### OAuth Injection Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `token_env_var` | string | required | Environment variable name containing the OAuth token (e.g., `VERTEX_ACCESS_TOKEN`). The proxy resolves this from the gateway `SecretResolver`. |
| `header_format` | string | `"Bearer {token}"` | Authorization header value template. Use `{token}` as placeholder. |

### How It Works

When a sandbox makes an HTTP request to an endpoint with `oauth` configuration:

1. **Policy Evaluation**: L7 proxy checks if endpoint has `oauth` field configured
2. **Token Resolution**: Proxy fetches token from environment variable via gateway `SecretResolver`
3. **Header Injection**: Proxy injects or replaces `Authorization` header using `header_format` template
4. **Request Forwarding**: Modified request forwarded to upstream with OAuth token

**Example for different OAuth formats:**

```yaml
# Standard Bearer token (default)
oauth:
  token_env_var: GITHUB_TOKEN
  header_format: "Bearer {token}"

# Custom OAuth scheme
oauth:
  token_env_var: CUSTOM_TOKEN
  header_format: "OAuth {token}"

# API key in custom header (non-standard but supported)
oauth:
  token_env_var: API_KEY
  header_format: "{token}"  # Just the token, no prefix
```

## How Auto-Refresh Works

### Architecture Overview

OpenShell uses a **proxy-driven token refresh** model where fresh tokens are fetched on-demand rather than stored in the sandbox:

```
┌──────────────┐         ┌──────────────┐         ┌──────────────┐
│   Gateway    │         │   Sandbox    │         │   Upstream   │
│  TokenCache  │         │    Proxy     │         │  API Server  │
└──────────────┘         └──────────────┘         └──────────────┘
       │                        │                         │
       │ 1. Fetch fresh token   │                         │
       │◄───────────────────────│                         │
       │                        │                         │
       │ 2. Return token        │                         │
       ├───────────────────────►│                         │
       │                        │ 3. Inject Authorization │
       │                        │    header               │
       │                        ├────────────────────────►│
       │                        │                         │
       │                        │ 4. Relay response       │
       │                        │◄────────────────────────│
       │                        │                         │
```

### Gateway-Side Token Caching

1. **Provider Creation**: User creates provider with OAuth credential (e.g., `VERTEX_ADC`)
2. **Gateway Startup**: Gateway creates `TokenCache` when first sandbox uses the provider
3. **Token Exchange**: `TokenCache` calls `get_runtime_token()` to exchange credential for OAuth token
4. **Caching**: Token cached in memory, valid for `expires_in` seconds
5. **Background Refresh** (when `auto_refresh: true`): Background task wakes periodically to refresh tokens
6. **Proactive Refresh**: Token refreshed N seconds before expiry (configurable via `refresh_margin_seconds`)
7. **Shared Cache**: All sandboxes using the same provider share the same `TokenCache`

### Proxy-Driven Token Refresh

When the sandbox makes an HTTP request to an OAuth-protected endpoint:

1. **Policy Lookup**: Proxy checks if endpoint has `oauth` configuration in sandbox policy
2. **Token Fetch**: Proxy fetches fresh token from gateway `TokenCache` via `SecretResolver`
3. **Header Injection**: Proxy injects/replaces `Authorization` header using `header_format` template
4. **Request Forward**: Request forwarded to upstream with valid OAuth token
5. **Seamless Refresh**: Gateway's background task ensures tokens are always fresh

**Key benefits:**
- Tokens never stored in sandbox (only fetched on-demand via gRPC)
- Gateway handles all token lifecycle management
- Sandbox proxy automatically uses latest token for each request
- No stale token failures even for long-running sandboxes

## Token Refresh Timing

For 1-hour OAuth tokens (3600 seconds):
- **Refresh margin**: 300 seconds (5 minutes)
- **Refresh interval**: 3600 - 300 = 3300 seconds (55 minutes)
- **Refresh trigger**: Token refreshed at T+55min (5 min before T+60min expiry)

For custom token lifetimes:
- Adjust `refresh_margin_seconds` in `TokenCache::new(provider, store, refresh_margin_seconds)`
- Default: 300 seconds (5 minutes)
- Minimum recommended: 60 seconds (1 minute)

## Example: AWS Bedrock Provider

```rust
// crates/openshell-providers/src/providers/bedrock.rs
pub struct BedrockProvider {
    client: reqwest::Client,
}

#[async_trait]
impl ProviderPlugin for BedrockProvider {
    fn id(&self) -> &'static str {
        "bedrock"
    }

    async fn get_runtime_token(&self, store: &dyn SecretStore) -> RuntimeResult<TokenResponse> {
        // Fetch AWS credentials
        let aws_access_key = store.get("AWS_ACCESS_KEY_ID").await?;
        let aws_secret_key = store.get("AWS_SECRET_ACCESS_KEY").await?;

        // Exchange for STS session token
        let sts_token = self.get_sts_session_token(&aws_access_key, &aws_secret_key).await?;

        Ok(TokenResponse {
            access_token: sts_token,
            token_type: "AWS4-HMAC-SHA256".to_string(),
            expires_in: 3600, // 1 hour
            metadata: HashMap::new(),
        })
    }
}
```

Then enable in gateway:

```rust
// should_use_token_cache()
("bedrock", "AWS_ACCESS_KEY_ID") | ("bedrock", "AWS_SECRET_ACCESS_KEY")

// get_or_create_token_cache()
"bedrock" => Arc::new(openshell_providers::bedrock::BedrockProvider::new())
```

## Testing

Add test in `crates/openshell-providers/src/providers/my_oauth_provider.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DatabaseStore, TokenCache};
    use std::sync::Arc;

    #[tokio::test]
    async fn test_token_exchange() {
        let mut creds = HashMap::new();
        creds.insert("MY_OAUTH_CREDENTIAL".to_string(), "test-credential".to_string());
        let store = Arc::new(DatabaseStore::new(creds));

        let provider = Arc::new(MyOAuthProvider::new());
        let cache = TokenCache::new(
            provider,
            store,
            300,   // refresh_margin_seconds
            true,  // auto_refresh
        );

        let token = cache.get_token("my-oauth-provider").await.unwrap();
        assert!(!token.is_empty());
    }
}
```

## Security Considerations

1. **Validate credentials** at provider creation time (in `discover_existing()`)
2. **Never log tokens** - only log token metadata (expiry time, etc.)
3. **Clear tokens on error** - TokenCache automatically handles cache invalidation
4. **Use HTTPS only** - All OAuth exchanges must use TLS
5. **Respect token expiry** - Always honor `expires_in` from OAuth provider
6. **Handle revocation** - Return `RuntimeError::AuthFailed` if token is revoked

## Implemented Features

- ✅ Gateway-side token caching with background refresh
- ✅ Proxy-driven token fetch (sandbox fetches fresh tokens on-demand from gateway)
- ✅ Generic OAuth header injection via endpoint-level `oauth` configuration
- ✅ Configurable refresh margin per provider (`refresh_margin_seconds`)
- ✅ Maximum sandbox lifetime limits (`max_lifetime_seconds`)
- ✅ Security-first defaults (`auto_refresh: false`)
- ✅ Policy-based OAuth configuration (no hardcoded provider logic)
- ✅ Support for custom `header_format` templates (Bearer, OAuth, custom schemes)

## Future Enhancements

- ⏳ Token persistence across gateway restarts (encrypted at-rest storage)
- ⏳ Multi-region token caching (edge deployments)
- ⏳ Token metrics and monitoring (expiry alerts, refresh failures)
- ⏳ Per-sandbox token refresh tracking (observability)
- ⏳ Token rotation support (graceful handling of multiple valid tokens)
