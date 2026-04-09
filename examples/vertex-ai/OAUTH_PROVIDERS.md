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

## Provider Configuration

When creating a provider with OAuth credentials, you can configure auto-refresh behavior:

```bash
openshell provider create vertex \
  --type vertex \
  --credential VERTEX_ADC=/path/to/adc.json \
  --config auto_refresh=true \
  --config refresh_margin_seconds=300 \
  --config max_lifetime_seconds=7200
```

### Configuration Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `auto_refresh` | bool | `false` | Enable automatic token refresh for long-running sandboxes. **Must be explicitly enabled for security.** |
| `refresh_margin_seconds` | int64 | `300` | Refresh tokens this many seconds before expiry (e.g., 300 = 5 minutes). |
| `max_lifetime_seconds` | int64 | `86400` | Maximum sandbox lifetime in seconds. `-1` = infinite, `0` or unspecified = 24 hours, `>0` = custom limit. |

**Security defaults:**
- `auto_refresh: false` - Disabled by default. Sandboxes must be explicitly configured for long-running operation.
- `max_lifetime_seconds: 86400` - 24-hour default limit prevents infinite-running sandboxes.

## Sandbox Policy Configuration

Override provider defaults in sandbox policy:

```yaml
# sandbox-policy.yaml
version: 1
oauth_credentials:
  auto_refresh: true
  refresh_margin_seconds: 300
  max_lifetime_seconds: 7200  # 2 hours
```

Policy-level configuration takes precedence over provider config.

## How Auto-Refresh Works

### Gateway-Side Token Caching

1. **Provider Creation**: User creates provider with OAuth credential
2. **Gateway Startup**: Gateway creates TokenCache when first sandbox uses the provider
3. **Token Exchange**: TokenCache calls `get_runtime_token()` to exchange credential for OAuth token
4. **Caching**: Token cached in memory, valid for `expires_in` seconds
5. **Background Refresh**: Background task wakes every 55 minutes (for 1-hour tokens)
6. **Proactive Refresh**: Token refreshed 5 minutes before expiry (configurable via `refresh_margin_seconds`)
7. **Shared Cache**: All sandboxes using same provider share the same TokenCache

### Sandbox-Side Token Refresh (Future)

**Note: Sandbox-side refresh is not yet implemented. This describes the planned design.**

When `auto_refresh: true`, long-running sandboxes will periodically re-fetch credentials:

1. Sandbox receives initial token with `OAuthCredentialMetadata`:
   ```json
   {
     "expires_in": 3600,
     "auto_refresh": true,
     "refresh_margin_seconds": 300,
     "max_lifetime_seconds": 7200
   }
   ```

2. Sandbox spawns background task that periodically calls `GetSandboxProviderEnvironment`

3. Gateway returns fresh token from its TokenCache (no re-authentication needed)

4. Sandbox updates its SecretResolver with the new token

5. HTTP proxy seamlessly uses refreshed token for subsequent requests

6. Sandbox self-terminates when `max_lifetime_seconds` is reached

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
        let cache = TokenCache::new(provider, store, 300);

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
- ✅ Configurable refresh margin per provider (`refresh_margin_seconds`)
- ✅ Maximum sandbox lifetime limits (`max_lifetime_seconds`)
- ✅ Security-first defaults (`auto_refresh: false`)
- ✅ OAuth metadata in gRPC responses (`OAuthCredentialMetadata`)
- ✅ Sandbox policy overrides for OAuth configuration

## Future Enhancements

- ⏳ Sandbox-side periodic token refresh (background task in sandbox)
- ⏳ Token persistence across gateway restarts (encrypted at-rest storage)
- ⏳ Multi-region token caching (edge deployments)
- ⏳ Token metrics and monitoring (expiry alerts, refresh failures)
- ⏳ Per-sandbox token refresh tracking (observability)
