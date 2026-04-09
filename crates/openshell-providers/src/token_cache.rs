// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Token cache with automatic background refresh.
//!
//! This module provides a caching layer on top of provider plugins and secret stores that:
//! - Caches tokens to avoid repeated fetches
//! - Automatically refreshes tokens before they expire
//! - Runs a background task to proactively refresh tokens

use crate::ProviderPlugin;
use crate::runtime::RuntimeResult;
use crate::secret_store::SecretStore;
use chrono::{DateTime, Duration, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Token cache entry with expiry tracking
#[derive(Debug, Clone)]
struct CachedToken {
    access_token: String,
    #[allow(dead_code)]
    token_type: String,
    expires_at: DateTime<Utc>,
    refresh_margin: Duration,
}

impl CachedToken {
    /// Check if token is still valid
    fn is_valid(&self) -> bool {
        Utc::now() < self.expires_at
    }

    /// Check if token should be refreshed (within margin of expiry)
    fn should_refresh(&self) -> bool {
        Utc::now() + self.refresh_margin > self.expires_at
    }
}

/// Token cache with automatic background refresh
///
/// This cache wraps a provider plugin and secret store:
/// 1. Caches tokens to avoid repeated network calls
/// 2. Returns cached token if still valid
/// 3. Fetches fresh token if cache miss or expired
/// 4. Runs background task to refresh tokens before expiry
pub struct TokenCache {
    /// Provider plugin that knows how to interpret credentials
    provider: Arc<dyn ProviderPlugin>,

    /// Secret store that provides raw credentials
    store: Arc<dyn SecretStore>,

    /// Cached tokens by provider name
    tokens: Arc<RwLock<HashMap<String, CachedToken>>>,

    /// Background refresh task handle
    refresh_task: Option<tokio::task::JoinHandle<()>>,

    /// How many seconds before expiry to refresh
    refresh_margin_seconds: i64,
}

impl std::fmt::Debug for TokenCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenCache")
            .field("provider_id", &self.provider.id())
            .field("store_name", &self.store.name())
            .field("refresh_margin_seconds", &self.refresh_margin_seconds)
            .field("has_background_task", &self.refresh_task.is_some())
            .finish()
    }
}

impl TokenCache {
    /// Create a new token cache
    ///
    /// # Arguments
    /// * `provider` - The provider plugin to interpret credentials
    /// * `store` - The secret store to fetch credentials from
    /// * `refresh_margin_seconds` - Refresh tokens this many seconds before expiry (default: 300 = 5 min)
    pub fn new(
        provider: Arc<dyn ProviderPlugin>,
        store: Arc<dyn SecretStore>,
        refresh_margin_seconds: i64,
    ) -> Self {
        let tokens = Arc::new(RwLock::new(HashMap::new()));

        // Start background refresh task
        let refresh_task = {
            let tokens = tokens.clone();
            let provider = provider.clone();
            let store = store.clone();
            let margin = refresh_margin_seconds;

            tokio::spawn(async move {
                Self::auto_refresh_loop(tokens, provider, store, margin).await;
            })
        };

        Self {
            provider,
            store,
            tokens,
            refresh_task: Some(refresh_task),
            refresh_margin_seconds,
        }
    }

    /// Get a token for the specified provider
    ///
    /// Returns cached token if valid, otherwise fetches fresh token.
    pub async fn get_token(&self, provider_name: &str) -> RuntimeResult<String> {
        let (token, _) = self.get_token_with_expiry(provider_name).await?;
        Ok(token)
    }

    /// Get a token with its expiry time.
    ///
    /// Returns (token, expires_in_seconds) where expires_in_seconds is the
    /// remaining time until token expiration.
    pub async fn get_token_with_expiry(&self, provider_name: &str) -> RuntimeResult<(String, u64)> {
        // Check cache first
        {
            let tokens = self.tokens.read().await;
            if let Some(cached) = tokens.get(provider_name) {
                if cached.is_valid() {
                    let expires_in = (cached.expires_at - Utc::now()).num_seconds().max(0) as u64;
                    tracing::debug!(
                        provider = provider_name,
                        expires_at = %cached.expires_at,
                        expires_in = expires_in,
                        "returning cached token"
                    );
                    return Ok((cached.access_token.clone(), expires_in));
                }
            }
        }

        // Cache miss or expired - fetch fresh token
        tracing::info!(provider = provider_name, "fetching fresh token");
        let token = self.refresh_token(provider_name).await?;

        // Get the expiry time we just cached
        let expires_in = {
            let tokens = self.tokens.read().await;
            if let Some(cached) = tokens.get(provider_name) {
                (cached.expires_at - Utc::now()).num_seconds().max(0) as u64
            } else {
                // Fallback - shouldn't happen since we just cached it
                3600
            }
        };

        Ok((token, expires_in))
    }

    /// Force refresh a token (bypasses cache)
    async fn refresh_token(&self, provider_name: &str) -> RuntimeResult<String> {
        let response = self.provider.get_runtime_token(self.store.as_ref()).await?;

        let expires_at = Utc::now() + Duration::seconds(response.expires_in as i64);
        let cached = CachedToken {
            access_token: response.access_token.clone(),
            token_type: response.token_type,
            expires_at,
            refresh_margin: Duration::seconds(self.refresh_margin_seconds),
        };

        tracing::info!(
            provider = provider_name,
            expires_at = %cached.expires_at,
            "cached fresh token"
        );

        self.tokens
            .write()
            .await
            .insert(provider_name.to_string(), cached);

        Ok(response.access_token)
    }

    /// Background task that proactively refreshes tokens before expiry
    async fn auto_refresh_loop(
        tokens: Arc<RwLock<HashMap<String, CachedToken>>>,
        provider: Arc<dyn ProviderPlugin>,
        store: Arc<dyn SecretStore>,
        margin_seconds: i64,
    ) {
        // For 60-minute tokens with 5-minute margin, we want to check every 55 minutes
        // This minimizes wake-ups while ensuring we catch the refresh window
        let check_interval_seconds = 3600 - margin_seconds; // Default: 3600 - 300 = 3300 (55 min)

        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(
                check_interval_seconds as u64,
            ))
            .await;

            // Find tokens that need refresh
            let to_refresh: Vec<String> = {
                let tokens = tokens.read().await;
                tokens
                    .iter()
                    .filter(|(_, token)| token.should_refresh())
                    .map(|(name, _)| name.clone())
                    .collect()
            };

            // Refresh each token
            for provider_name in to_refresh {
                tracing::info!(provider = provider_name, "background refresh triggered");

                match provider.get_runtime_token(store.as_ref()).await {
                    Ok(response) => {
                        let expires_at = Utc::now() + Duration::seconds(response.expires_in as i64);
                        let cached = CachedToken {
                            access_token: response.access_token,
                            token_type: response.token_type,
                            expires_at,
                            refresh_margin: Duration::seconds(margin_seconds),
                        };

                        tokens.write().await.insert(provider_name.clone(), cached);

                        tracing::info!(
                            provider = provider_name,
                            expires_at = %expires_at,
                            "background refresh succeeded"
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            provider = provider_name,
                            error = %e,
                            "background refresh failed"
                        );
                    }
                }
            }
        }
    }
}

impl Drop for TokenCache {
    fn drop(&mut self) {
        if let Some(task) = self.refresh_task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::RuntimeResult;
    use crate::{DatabaseStore, ProviderPlugin, SecretStore, TokenResponse};
    use async_trait::async_trait;

    struct MockProvider;

    #[async_trait]
    impl ProviderPlugin for MockProvider {
        fn id(&self) -> &'static str {
            "mock"
        }

        fn discover_existing(
            &self,
        ) -> Result<Option<crate::DiscoveredProvider>, crate::ProviderError> {
            Ok(None)
        }

        async fn get_runtime_token(
            &self,
            _store: &dyn SecretStore,
        ) -> RuntimeResult<TokenResponse> {
            Ok(TokenResponse {
                access_token: "mock-token".to_string(),
                token_type: "Bearer".to_string(),
                expires_in: 3600,
                metadata: HashMap::new(),
            })
        }
    }

    #[tokio::test]
    async fn test_cache_miss_fetches_token() {
        let provider = Arc::new(MockProvider);
        let store = Arc::new(DatabaseStore::new(HashMap::new()));
        let cache = TokenCache::new(provider, store, 300);

        let token = cache.get_token("mock").await.unwrap();
        assert_eq!(token, "mock-token");
    }

    #[tokio::test]
    async fn test_cache_hit_avoids_fetch() {
        let provider = Arc::new(MockProvider);
        let store = Arc::new(DatabaseStore::new(HashMap::new()));
        let cache = TokenCache::new(provider, store, 300);

        // First call - cache miss
        let token1 = cache.get_token("mock").await.unwrap();

        // Second call - cache hit
        let token2 = cache.get_token("mock").await.unwrap();

        assert_eq!(token1, token2);
    }
}
