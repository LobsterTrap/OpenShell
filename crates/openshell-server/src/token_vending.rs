// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! `OAuth2` token vending service.
//!
//! Performs `OAuth2` token exchanges, caches access tokens per provider, and
//! deduplicates concurrent refresh requests. Used by
//! `resolve_provider_environment()` to transparently return current access
//! tokens for `OAuth2` providers.

use std::collections::HashMap;
use std::sync::Arc;

use openshell_core::proto::Provider;
use tokio::sync::Mutex;
use tracing::{debug, error, info};

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors returned by the token vending service.
#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("OAuth2 token exchange failed: {0}")]
    Exchange(String),

    #[error("OAuth2 provider misconfigured: {0}")]
    Config(String),

    #[error("HTTP request to token endpoint failed: {0}")]
    Http(String),

    #[error("token endpoint returned error: {status} — {body}")]
    EndpointError { status: u16, body: String },
}

// ---------------------------------------------------------------------------
// Token result
// ---------------------------------------------------------------------------

/// Result of a successful token acquisition.
#[derive(Debug, Clone)]
pub struct TokenResult {
    /// The current access token value.
    pub access_token: String,
    /// Seconds until this token expires.
    pub expires_in_secs: u32,
    /// If the `IdP` returned a rotated refresh token, it is captured here
    /// so the caller can persist it.
    pub new_refresh_token: Option<String>,
}

// ---------------------------------------------------------------------------
// Cached token
// ---------------------------------------------------------------------------

/// Cached access token with expiry metadata.
#[derive(Debug, Clone)]
struct CachedToken {
    access_token: String,
    /// Absolute expiry time (monotonic).
    expires_at: tokio::time::Instant,
    /// Original TTL in seconds as reported by the `IdP`.
    ttl_secs: u32,
}

impl CachedToken {
    /// Returns `true` if the token is still usable (with safety margin).
    fn is_valid(&self) -> bool {
        let margin = std::cmp::max(60, u64::from(self.ttl_secs / 10));
        let margin_dur = std::time::Duration::from_secs(margin);
        tokio::time::Instant::now() + margin_dur < self.expires_at
    }
}

// ---------------------------------------------------------------------------
// Per-provider state
// ---------------------------------------------------------------------------

/// Per-provider token state, protected by a Tokio mutex for async refresh.
struct ProviderTokenState {
    cached: Option<CachedToken>,
}

// ---------------------------------------------------------------------------
// Token vending service
// ---------------------------------------------------------------------------

/// Token vending service. One per gateway process.
///
/// Caches access tokens per provider and deduplicates concurrent refresh
/// requests via a per-provider `Mutex`.
pub struct TokenVendingService {
    /// Per-provider token state, keyed by provider ID.
    state: dashmap::DashMap<String, Arc<Mutex<ProviderTokenState>>>,
    /// Shared HTTP client for token endpoint requests.
    http_client: reqwest::Client,
}

impl std::fmt::Debug for TokenVendingService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenVendingService")
            .field("cached_providers", &self.state.len())
            .finish()
    }
}

impl TokenVendingService {
    /// Create a new token vending service.
    #[must_use]
    pub fn new() -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build reqwest client");
        Self {
            state: dashmap::DashMap::new(),
            http_client,
        }
    }

    /// Get a current access token for an `OAuth2` provider.
    ///
    /// Returns the cached token if still valid (with safety margin).
    /// Otherwise performs a token refresh, caches the result, and returns it.
    /// Concurrent callers for the same provider are deduplicated — only one
    /// HTTP request to the `IdP` is made; others await the result.
    pub async fn get_or_refresh(&self, provider: &Provider) -> Result<TokenResult, TokenError> {
        let provider_id = &provider.id;

        // Get or create per-provider state.
        let state = self
            .state
            .entry(provider_id.clone())
            .or_insert_with(|| Arc::new(Mutex::new(ProviderTokenState { cached: None })))
            .clone();

        // Acquire the per-provider mutex. This serializes concurrent refresh
        // requests for the same provider — only the first caller performs the
        // HTTP request; others await the mutex and get the cached result.
        let mut guard = state.lock().await;

        // Check cache under the lock.
        if let Some(ref cached) = guard.cached
            && cached.is_valid()
        {
            debug!(
                provider_id = %provider_id,
                ttl_remaining_secs = cached.expires_at.duration_since(tokio::time::Instant::now()).as_secs(),
                "Returning cached OAuth2 access token"
            );
            return Ok(TokenResult {
                access_token: cached.access_token.clone(),
                expires_in_secs: cached.ttl_secs,
                new_refresh_token: None,
            });
        }

        // Cache miss or expired — perform token exchange.
        let config = OAuth2Config::from_provider(provider)?;
        let result = self.exchange_token(&config).await?;

        info!(
            provider_id = %provider_id,
            provider_name = %provider.name,
            expires_in_secs = result.expires_in_secs,
            refresh_token_rotated = result.new_refresh_token.is_some(),
            "OAuth2 token exchange completed"
        );

        // Cache the new token.
        guard.cached = Some(CachedToken {
            access_token: result.access_token.clone(),
            expires_at: tokio::time::Instant::now()
                + std::time::Duration::from_secs(u64::from(result.expires_in_secs)),
            ttl_secs: result.expires_in_secs,
        });

        Ok(result)
    }

    /// Perform the actual HTTP token exchange.
    async fn exchange_token(&self, config: &OAuth2Config) -> Result<TokenResult, TokenError> {
        let mut params = HashMap::new();
        params.insert("client_id", config.client_id.as_str());
        params.insert("client_secret", config.client_secret.as_str());
        params.insert("grant_type", config.grant_type.as_str());

        if let Some(ref refresh_token) = config.refresh_token {
            params.insert("refresh_token", refresh_token.as_str());
        }

        if let Some(ref scopes) = config.scopes {
            params.insert("scope", scopes.as_str());
        }

        debug!(
            token_endpoint = %config.token_endpoint,
            grant_type = %config.grant_type,
            "Sending OAuth2 token exchange request"
        );

        // Explicitly request JSON. Some OAuth2 providers (notably GitHub)
        // default to application/x-www-form-urlencoded responses without this.
        let response = self
            .http_client
            .post(&config.token_endpoint)
            .header("Accept", "application/json")
            .form(&params)
            .send()
            .await
            .map_err(|e| TokenError::Http(e.to_string()))?;

        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|e| TokenError::Http(format!("failed to read response body: {e}")))?;

        if status != 200 {
            // Truncate body for logging safety. Find a char boundary at
            // or before byte 256 to avoid panicking on multi-byte UTF-8.
            let truncated = if body.len() > 256 {
                let end = (0..=256)
                    .rev()
                    .find(|&i| body.is_char_boundary(i))
                    .unwrap_or(0);
                format!("{}...", &body[..end])
            } else {
                body
            };
            error!(
                token_endpoint = %config.token_endpoint,
                status,
                response_body = %truncated,
                "OAuth2 token endpoint returned error"
            );
            return Err(TokenError::EndpointError {
                status,
                body: truncated,
            });
        }

        let token_response: OAuth2TokenResponse = serde_json::from_str(&body)
            .map_err(|e| TokenError::Exchange(format!("failed to parse token response: {e}")))?;

        let access_token = token_response.access_token.ok_or_else(|| {
            TokenError::Exchange("token response missing 'access_token' field".into())
        })?;

        if access_token.is_empty() {
            return Err(TokenError::Exchange(
                "token response contains empty 'access_token'".into(),
            ));
        }

        // Fail-fast: reject access tokens containing header-injection
        // characters (CR, LF, NUL). The downstream `SecretResolver` also
        // validates during placeholder resolution, but catching it here
        // provides a clearer error and avoids caching a poisoned token.
        validate_token_value(&access_token)?;

        // Validate rotated refresh token if present. A poisoned refresh
        // token would be persisted to the store and break future refreshes.
        let new_refresh_token = match token_response.refresh_token {
            Some(rt) => {
                validate_token_value(&rt).map_err(|_| {
                    TokenError::Exchange(
                        "rotated refresh token contains prohibited control characters".into(),
                    )
                })?;
                Some(rt)
            }
            None => None,
        };

        // Default to 3600s if the IdP doesn't specify expires_in.
        let expires_in = token_response.expires_in.unwrap_or(3600);

        Ok(TokenResult {
            access_token,
            expires_in_secs: expires_in,
            new_refresh_token,
        })
    }
}

impl Default for TokenVendingService {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// OAuth2 config extraction
// ---------------------------------------------------------------------------

/// Parsed `OAuth2` configuration from a Provider record.
struct OAuth2Config {
    token_endpoint: String,
    grant_type: String,
    client_id: String,
    client_secret: String,
    refresh_token: Option<String>,
    scopes: Option<String>,
}

impl OAuth2Config {
    fn from_provider(provider: &Provider) -> Result<Self, TokenError> {
        let config = &provider.config;
        let credentials = &provider.credentials;

        let token_endpoint = config
            .get("oauth_token_endpoint")
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| {
                TokenError::Config("missing oauth_token_endpoint in provider config".into())
            })?
            .clone();

        let grant_type = config
            .get("oauth_grant_type")
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| {
                TokenError::Config("missing oauth_grant_type in provider config".into())
            })?
            .clone();

        let client_id = credentials
            .get("OAUTH_CLIENT_ID")
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| {
                TokenError::Config("missing OAUTH_CLIENT_ID in provider credentials".into())
            })?
            .clone();

        let client_secret = credentials
            .get("OAUTH_CLIENT_SECRET")
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| {
                TokenError::Config("missing OAUTH_CLIENT_SECRET in provider credentials".into())
            })?
            .clone();

        let refresh_token = credentials.get("OAUTH_REFRESH_TOKEN").cloned();
        let scopes = config.get("oauth_scopes").cloned();

        Ok(Self {
            token_endpoint,
            grant_type,
            client_id,
            client_secret,
            refresh_token,
            scopes,
        })
    }
}

// ---------------------------------------------------------------------------
// OAuth2 token response (RFC 6749 §5.1)
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
struct OAuth2TokenResponse {
    access_token: Option<String>,
    #[allow(dead_code)]
    token_type: Option<String>,
    expires_in: Option<u32>,
    refresh_token: Option<String>,
    #[allow(dead_code)]
    scope: Option<String>,
}

// ---------------------------------------------------------------------------
// Helper functions for provider classification
// ---------------------------------------------------------------------------

/// Reject access tokens containing header-injection characters.
///
/// Mirrors the `validate_resolved_secret()` check in the sandbox's
/// `SecretResolver`, but applied at the token vending layer for fail-fast
/// behavior. Prevents caching a poisoned token that would be rejected
/// downstream on every request.
fn validate_token_value(value: &str) -> Result<(), TokenError> {
    if value
        .bytes()
        .any(|b| b == b'\r' || b == b'\n' || b == b'\0')
    {
        return Err(TokenError::Exchange(
            "access token contains prohibited control characters (CR, LF, or NUL)".into(),
        ));
    }
    Ok(())
}

/// Determine if a provider uses `OAuth2` based on its config map.
pub fn is_oauth2_provider(provider: &Provider) -> bool {
    provider
        .config
        .get("auth_method")
        .is_some_and(|v| v == "oauth2")
}

/// `OAuth2` credential keys that must never be injected into sandbox env.
pub fn is_oauth2_internal_credential(key: &str) -> bool {
    matches!(
        key,
        "OAUTH_CLIENT_ID" | "OAUTH_CLIENT_SECRET" | "OAUTH_REFRESH_TOKEN"
    )
}

/// Derive the access token env var key for an `OAuth2` provider.
pub fn oauth_access_token_key(provider: &Provider) -> String {
    provider
        .config
        .get("oauth_access_token_env")
        .filter(|v| !v.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| format!("{}_ACCESS_TOKEN", provider.r#type.to_ascii_uppercase()))
}

/// Validate OAuth2-specific configuration at provider creation time.
///
/// Returns `Ok(())` for non-OAuth2 providers (no-op).
#[allow(clippy::result_large_err)]
pub fn validate_oauth2_config(provider: &Provider) -> Result<(), tonic::Status> {
    let config = &provider.config;

    if !is_oauth2_provider(provider) {
        return Ok(());
    }

    // Required config keys.
    for key in &["oauth_token_endpoint", "oauth_grant_type"] {
        if config.get(*key).is_none_or(|v| v.trim().is_empty()) {
            return Err(tonic::Status::invalid_argument(format!(
                "OAuth2 provider requires config key '{key}'"
            )));
        }
    }

    let token_endpoint = config.get("oauth_token_endpoint").unwrap();
    if !token_endpoint.starts_with("https://") {
        return Err(tonic::Status::invalid_argument(
            "oauth_token_endpoint must use HTTPS",
        ));
    }

    let grant_type = config.get("oauth_grant_type").unwrap();
    match grant_type.as_str() {
        "refresh_token" => {
            for key in &[
                "OAUTH_CLIENT_ID",
                "OAUTH_CLIENT_SECRET",
                "OAUTH_REFRESH_TOKEN",
            ] {
                if !provider.credentials.contains_key(*key) {
                    return Err(tonic::Status::invalid_argument(format!(
                        "OAuth2 refresh_token grant requires credential '{key}'"
                    )));
                }
            }
        }
        "client_credentials" => {
            for key in &["OAUTH_CLIENT_ID", "OAUTH_CLIENT_SECRET"] {
                if !provider.credentials.contains_key(*key) {
                    return Err(tonic::Status::invalid_argument(format!(
                        "OAuth2 client_credentials grant requires credential '{key}'"
                    )));
                }
            }
        }
        other => {
            return Err(tonic::Status::invalid_argument(format!(
                "unsupported oauth_grant_type: '{other}' (expected 'refresh_token' or 'client_credentials')"
            )));
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_oauth2_provider(grant_type: &str, include_refresh: bool) -> Provider {
        let mut credentials = HashMap::new();
        credentials.insert("OAUTH_CLIENT_ID".into(), "test-client-id".into());
        credentials.insert("OAUTH_CLIENT_SECRET".into(), "test-client-secret".into());
        if include_refresh {
            credentials.insert("OAUTH_REFRESH_TOKEN".into(), "test-refresh-token".into());
        }

        let mut config = HashMap::new();
        config.insert("auth_method".into(), "oauth2".into());
        config.insert("oauth_grant_type".into(), grant_type.into());
        config.insert(
            "oauth_token_endpoint".into(),
            "https://auth.example.com/oauth/token".into(),
        );

        Provider {
            id: "test-id".into(),
            name: "test-oauth".into(),
            r#type: "github".into(),
            credentials,
            config,
        }
    }

    fn make_static_provider() -> Provider {
        let mut credentials = HashMap::new();
        credentials.insert("GITHUB_TOKEN".into(), "ghp-static-token".into());

        Provider {
            id: "static-id".into(),
            name: "test-static".into(),
            r#type: "github".into(),
            credentials,
            config: HashMap::new(),
        }
    }

    // --- is_oauth2_provider ---

    #[test]
    fn detects_oauth2_provider() {
        let provider = make_oauth2_provider("refresh_token", true);
        assert!(is_oauth2_provider(&provider));
    }

    #[test]
    fn static_provider_is_not_oauth2() {
        let provider = make_static_provider();
        assert!(!is_oauth2_provider(&provider));
    }

    #[test]
    fn empty_config_is_not_oauth2() {
        let provider = Provider {
            id: String::new(),
            name: "empty".into(),
            r#type: "generic".into(),
            credentials: HashMap::new(),
            config: HashMap::new(),
        };
        assert!(!is_oauth2_provider(&provider));
    }

    // --- is_oauth2_internal_credential ---

    #[test]
    fn identifies_internal_credentials() {
        assert!(is_oauth2_internal_credential("OAUTH_CLIENT_ID"));
        assert!(is_oauth2_internal_credential("OAUTH_CLIENT_SECRET"));
        assert!(is_oauth2_internal_credential("OAUTH_REFRESH_TOKEN"));
    }

    #[test]
    fn non_oauth_keys_are_not_internal() {
        assert!(!is_oauth2_internal_credential("GITHUB_TOKEN"));
        assert!(!is_oauth2_internal_credential("ANTHROPIC_API_KEY"));
        assert!(!is_oauth2_internal_credential("OAUTH_ACCESS_TOKEN"));
    }

    // --- oauth_access_token_key ---

    #[test]
    fn derives_access_token_key_from_type() {
        let provider = make_oauth2_provider("refresh_token", true);
        assert_eq!(oauth_access_token_key(&provider), "GITHUB_ACCESS_TOKEN");
    }

    #[test]
    fn uses_custom_access_token_key_when_configured() {
        let mut provider = make_oauth2_provider("refresh_token", true);
        provider
            .config
            .insert("oauth_access_token_env".into(), "MY_CUSTOM_TOKEN".into());
        assert_eq!(oauth_access_token_key(&provider), "MY_CUSTOM_TOKEN");
    }

    #[test]
    fn ignores_empty_custom_access_token_key() {
        let mut provider = make_oauth2_provider("refresh_token", true);
        provider
            .config
            .insert("oauth_access_token_env".into(), "  ".into());
        assert_eq!(oauth_access_token_key(&provider), "GITHUB_ACCESS_TOKEN");
    }

    // --- validate_oauth2_config ---

    #[test]
    fn validates_refresh_token_grant() {
        let provider = make_oauth2_provider("refresh_token", true);
        assert!(validate_oauth2_config(&provider).is_ok());
    }

    #[test]
    fn validates_client_credentials_grant() {
        let provider = make_oauth2_provider("client_credentials", false);
        assert!(validate_oauth2_config(&provider).is_ok());
    }

    #[test]
    fn skips_validation_for_static_providers() {
        let provider = make_static_provider();
        assert!(validate_oauth2_config(&provider).is_ok());
    }

    #[test]
    fn rejects_missing_token_endpoint() {
        let mut provider = make_oauth2_provider("refresh_token", true);
        provider.config.remove("oauth_token_endpoint");
        let err = validate_oauth2_config(&provider).unwrap_err();
        assert!(err.message().contains("oauth_token_endpoint"));
    }

    #[test]
    fn rejects_http_token_endpoint() {
        let mut provider = make_oauth2_provider("refresh_token", true);
        provider.config.insert(
            "oauth_token_endpoint".into(),
            "http://insecure.example.com/token".into(),
        );
        let err = validate_oauth2_config(&provider).unwrap_err();
        assert!(err.message().contains("HTTPS"));
    }

    #[test]
    fn rejects_missing_grant_type() {
        let mut provider = make_oauth2_provider("refresh_token", true);
        provider.config.remove("oauth_grant_type");
        let err = validate_oauth2_config(&provider).unwrap_err();
        assert!(err.message().contains("oauth_grant_type"));
    }

    #[test]
    fn rejects_unsupported_grant_type() {
        let provider = make_oauth2_provider("implicit", false);
        let err = validate_oauth2_config(&provider).unwrap_err();
        assert!(err.message().contains("implicit"));
    }

    #[test]
    fn rejects_refresh_token_grant_without_refresh_token() {
        let provider = make_oauth2_provider("refresh_token", false);
        let err = validate_oauth2_config(&provider).unwrap_err();
        assert!(err.message().contains("OAUTH_REFRESH_TOKEN"));
    }

    #[test]
    fn rejects_missing_client_id() {
        let mut provider = make_oauth2_provider("client_credentials", false);
        provider.credentials.remove("OAUTH_CLIENT_ID");
        let err = validate_oauth2_config(&provider).unwrap_err();
        assert!(err.message().contains("OAUTH_CLIENT_ID"));
    }

    #[test]
    fn rejects_missing_client_secret() {
        let mut provider = make_oauth2_provider("client_credentials", false);
        provider.credentials.remove("OAUTH_CLIENT_SECRET");
        let err = validate_oauth2_config(&provider).unwrap_err();
        assert!(err.message().contains("OAUTH_CLIENT_SECRET"));
    }

    // --- OAuth2Config::from_provider ---

    #[test]
    fn extracts_config_from_provider() {
        let provider = make_oauth2_provider("refresh_token", true);
        let config = OAuth2Config::from_provider(&provider).unwrap();
        assert_eq!(
            config.token_endpoint,
            "https://auth.example.com/oauth/token"
        );
        assert_eq!(config.grant_type, "refresh_token");
        assert_eq!(config.client_id, "test-client-id");
        assert_eq!(config.client_secret, "test-client-secret");
        assert_eq!(config.refresh_token, Some("test-refresh-token".into()));
        assert!(config.scopes.is_none());
    }

    #[test]
    fn extracts_scopes_when_present() {
        let mut provider = make_oauth2_provider("client_credentials", false);
        provider
            .config
            .insert("oauth_scopes".into(), "api read_user".into());
        let config = OAuth2Config::from_provider(&provider).unwrap();
        assert_eq!(config.scopes, Some("api read_user".into()));
    }

    // --- TokenVendingService (unit-level) ---

    #[tokio::test]
    async fn token_vending_service_constructs() {
        let _service = TokenVendingService::new();
    }

    // --- validate_token_value ---

    #[test]
    fn validates_clean_token() {
        assert!(validate_token_value("gho_abc123XYZ").is_ok());
    }

    #[test]
    fn rejects_token_with_newline() {
        let err = validate_token_value("token\ninjection").unwrap_err();
        assert!(matches!(err, TokenError::Exchange(_)));
    }

    #[test]
    fn rejects_token_with_carriage_return() {
        let err = validate_token_value("token\rinjection").unwrap_err();
        assert!(matches!(err, TokenError::Exchange(_)));
    }

    #[test]
    fn rejects_token_with_null_byte() {
        let err = validate_token_value("token\0injection").unwrap_err();
        assert!(matches!(err, TokenError::Exchange(_)));
    }

    #[test]
    fn cached_token_validity() {
        let valid = CachedToken {
            access_token: "tok".into(),
            expires_at: tokio::time::Instant::now() + std::time::Duration::from_secs(3600),
            ttl_secs: 3600,
        };
        assert!(valid.is_valid());

        let expired = CachedToken {
            access_token: "tok".into(),
            expires_at: tokio::time::Instant::now(),
            ttl_secs: 3600,
        };
        assert!(!expired.is_valid());
    }

    // --- Integration tests (mock HTTP server) ---

    #[tokio::test]
    async fn exchange_token_with_mock_server() {
        use tokio::net::TcpListener;

        // Start a minimal mock HTTPS-less HTTP server for testing.
        // In production, token endpoints are HTTPS. For unit tests we
        // bypass the HTTPS requirement by constructing OAuth2Config directly.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Spawn mock server that returns a valid token response.
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = tokio::io::AsyncReadExt::read(&mut stream, &mut buf)
                .await
                .unwrap();
            let request = String::from_utf8_lossy(&buf[..n]);
            assert!(request.contains("grant_type=client_credentials"));
            assert!(request.contains("client_id=test-id"));
            assert!(request.contains("client_secret=test-secret"));

            let body =
                r#"{"access_token":"mock-access-token","token_type":"Bearer","expires_in":3600}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
                .await
                .unwrap();
        });

        let service = TokenVendingService::new();
        let config = OAuth2Config {
            token_endpoint: format!("http://127.0.0.1:{}", addr.port()),
            grant_type: "client_credentials".into(),
            client_id: "test-id".into(),
            client_secret: "test-secret".into(),
            refresh_token: None,
            scopes: None,
        };

        let result = service.exchange_token(&config).await.unwrap();
        assert_eq!(result.access_token, "mock-access-token");
        assert_eq!(result.expires_in_secs, 3600);
        assert!(result.new_refresh_token.is_none());
    }

    #[tokio::test]
    async fn exchange_token_with_refresh_rotation() {
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buf)
                .await
                .unwrap();

            let body = r#"{"access_token":"new-access","token_type":"Bearer","expires_in":7200,"refresh_token":"new-refresh"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
                .await
                .unwrap();
        });

        let service = TokenVendingService::new();
        let config = OAuth2Config {
            token_endpoint: format!("http://127.0.0.1:{}", addr.port()),
            grant_type: "refresh_token".into(),
            client_id: "test-id".into(),
            client_secret: "test-secret".into(),
            refresh_token: Some("old-refresh".into()),
            scopes: None,
        };

        let result = service.exchange_token(&config).await.unwrap();
        assert_eq!(result.access_token, "new-access");
        assert_eq!(result.expires_in_secs, 7200);
        assert_eq!(result.new_refresh_token, Some("new-refresh".into()));
    }

    #[tokio::test]
    async fn exchange_token_error_response() {
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buf)
                .await
                .unwrap();

            let body = r#"{"error":"invalid_grant","error_description":"refresh token revoked"}"#;
            let response = format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
                .await
                .unwrap();
        });

        let service = TokenVendingService::new();
        let config = OAuth2Config {
            token_endpoint: format!("http://127.0.0.1:{}", addr.port()),
            grant_type: "refresh_token".into(),
            client_id: "test-id".into(),
            client_secret: "test-secret".into(),
            refresh_token: Some("revoked-token".into()),
            scopes: None,
        };

        let err = service.exchange_token(&config).await.unwrap_err();
        assert!(matches!(err, TokenError::EndpointError { status: 400, .. }));
    }

    #[tokio::test]
    async fn caching_returns_same_token() {
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Server only handles one connection — second call must hit cache.
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buf)
                .await
                .unwrap();

            let body = r#"{"access_token":"cached-token","token_type":"Bearer","expires_in":3600}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes())
                .await
                .unwrap();
        });

        let service = TokenVendingService::new();
        let provider = Provider {
            id: "cache-test".into(),
            name: "cache-test".into(),
            r#type: "github".into(),
            credentials: [
                ("OAUTH_CLIENT_ID".into(), "id".into()),
                ("OAUTH_CLIENT_SECRET".into(), "secret".into()),
            ]
            .into_iter()
            .collect(),
            config: [
                ("auth_method".into(), "oauth2".into()),
                ("oauth_grant_type".into(), "client_credentials".into()),
                (
                    "oauth_token_endpoint".into(),
                    format!("http://127.0.0.1:{}", addr.port()),
                ),
            ]
            .into_iter()
            .collect(),
        };

        let result1 = service.get_or_refresh(&provider).await.unwrap();
        assert_eq!(result1.access_token, "cached-token");

        // Second call should return cached token (server has closed).
        let result2 = service.get_or_refresh(&provider).await.unwrap();
        assert_eq!(result2.access_token, "cached-token");
    }
}
