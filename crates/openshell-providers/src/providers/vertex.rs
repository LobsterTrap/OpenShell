// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

use crate::{
    DiscoveredProvider, ProviderDiscoverySpec, ProviderError, ProviderPlugin, RealDiscoveryContext,
    RuntimeError, RuntimeResult, SecretStore, TokenResponse, discover_with_spec,
};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

pub struct VertexProvider {
    client: Client,
}

impl VertexProvider {
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    /// Get the standard ADC file path
    fn get_standard_adc_path() -> Option<PathBuf> {
        let home = std::env::var("HOME").ok()?;
        Some(PathBuf::from(home).join(".config/gcloud/application_default_credentials.json"))
    }

    /// Try to read ADC from standard gcloud location
    fn read_adc_from_standard_path() -> Option<String> {
        let path = Self::get_standard_adc_path()?;
        std::fs::read_to_string(path).ok()
    }

    /// Validate ADC credentials by testing token exchange
    /// This is synchronous and blocks during provider creation
    fn validate_adc_sync(adc_json: &str) -> Result<(), ProviderError> {
        // Parse ADC JSON
        let adc: AdcCredentials = serde_json::from_str(adc_json).map_err(|e| {
            ProviderError::UnsupportedProvider(format!(
                "Invalid ADC format: {}. Expected Google Application Default Credentials JSON from 'gcloud auth application-default login'",
                e
            ))
        })?;

        // Test token exchange - use current runtime if available, otherwise create one
        let result = if let Ok(handle) = tokio::runtime::Handle::try_current() {
            // Already in a runtime - use block_in_place to avoid nested runtime error
            tokio::task::block_in_place(|| handle.block_on(Self::validate_adc_async(adc)))
        } else {
            // Not in a runtime - create one
            let runtime = tokio::runtime::Runtime::new().map_err(|e| {
                ProviderError::UnsupportedProvider(format!(
                    "Failed to create runtime for validation: {}",
                    e
                ))
            })?;
            runtime.block_on(Self::validate_adc_async(adc))
        };

        result
    }

    /// Async helper for ADC validation
    async fn validate_adc_async(adc: AdcCredentials) -> Result<(), ProviderError> {
        let client = Client::new();
        let params = [
            ("client_id", adc.client_id.as_str()),
            ("client_secret", adc.client_secret.as_str()),
            ("refresh_token", adc.refresh_token.as_str()),
            ("grant_type", "refresh_token"),
        ];

        let response = client
            .post("https://oauth2.googleapis.com/token")
            .form(&params)
            .send()
            .await
            .map_err(|e| {
                ProviderError::UnsupportedProvider(format!(
                    "Failed to connect to Google OAuth: {}. Check your internet connection.",
                    e
                ))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::UnsupportedProvider(format!(
                "ADC credentials rejected by Google OAuth (status {}): {}. Your credentials may be expired or invalid. Run: gcloud auth application-default login",
                status, body
            )));
        }

        // Successfully exchanged for token
        tracing::info!("✅ Verified Vertex ADC credentials with Google OAuth");
        Ok(())
    }

    /// Exchange ADC credentials for OAuth access token
    async fn exchange_adc_for_token(&self, adc: AdcCredentials) -> RuntimeResult<TokenResponse> {
        let params = [
            ("client_id", adc.client_id.as_str()),
            ("client_secret", adc.client_secret.as_str()),
            ("refresh_token", adc.refresh_token.as_str()),
            ("grant_type", "refresh_token"),
        ];

        let response = self
            .client
            .post("https://oauth2.googleapis.com/token")
            .form(&params)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(RuntimeError::AuthFailed(format!(
                "OAuth token request failed with status {}: {}",
                status, body
            )));
        }

        let token_response: GoogleTokenResponse = response.json().await?;

        Ok(TokenResponse {
            access_token: token_response.access_token.trim().to_string(),
            token_type: token_response.token_type,
            expires_in: token_response.expires_in,
            metadata: HashMap::new(),
        })
    }
}

impl Default for VertexProvider {
    fn default() -> Self {
        Self::new()
    }
}

pub const SPEC: ProviderDiscoverySpec = ProviderDiscoverySpec {
    id: "vertex",
    credential_env_vars: &["ANTHROPIC_VERTEX_PROJECT_ID"],
};

// Additional config keys for Vertex AI
const VERTEX_CONFIG_KEYS: &[&str] = &["ANTHROPIC_VERTEX_REGION"];

/// ADC (Application Default Credentials) format from gcloud
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AdcCredentials {
    client_id: String,
    client_secret: String,
    refresh_token: String,
    #[serde(rename = "type")]
    cred_type: String,
}

/// Google OAuth token response
#[derive(Debug, Deserialize)]
struct GoogleTokenResponse {
    access_token: String,
    token_type: String,
    expires_in: u64,
}

#[async_trait]
impl ProviderPlugin for VertexProvider {
    fn id(&self) -> &'static str {
        SPEC.id
    }

    fn discover_existing(&self) -> Result<Option<DiscoveredProvider>, ProviderError> {
        let mut discovered = discover_with_spec(&SPEC, &RealDiscoveryContext)?;

        // Add region config if present
        if let Some(ref mut provider) = discovered {
            for &key in VERTEX_CONFIG_KEYS {
                if let Ok(value) = std::env::var(key) {
                    provider.config.insert(key.to_string(), value);
                }
            }

            // Set CLAUDE_CODE_USE_VERTEX=1 to enable Vertex AI in claude CLI
            // Must be in credentials (not config) to be injected into sandbox environment
            provider
                .credentials
                .insert("CLAUDE_CODE_USE_VERTEX".to_string(), "1".to_string());

            // Try to discover ADC credentials
            // Priority:
            // 1. VERTEX_ADC environment variable (explicit override)
            // 2. Standard gcloud ADC path: ~/.config/gcloud/application_default_credentials.json
            let adc_result = if let Ok(adc) = std::env::var("VERTEX_ADC") {
                tracing::debug!("discovered VERTEX_ADC from environment variable");
                Some(adc)
            } else if let Some(adc) = Self::read_adc_from_standard_path() {
                tracing::debug!("discovered ADC from standard gcloud path");
                Some(adc)
            } else {
                None
            };

            match adc_result {
                Some(adc_json) => {
                    // Validate ADC by testing token exchange with Google OAuth
                    Self::validate_adc_sync(&adc_json)?;

                    provider
                        .credentials
                        .insert("VERTEX_ADC".to_string(), adc_json);
                    tracing::info!("✅ Validated and stored Vertex ADC credentials");
                }
                None => {
                    return Err(ProviderError::UnsupportedProvider(
                        "Vertex ADC credentials not found. Run one of:\n  \
                         1. gcloud auth application-default login (creates ~/.config/gcloud/application_default_credentials.json)\n  \
                         2. export VERTEX_ADC=\"$(cat /path/to/adc.json)\"\n  \
                         3. openshell provider create --name vertex --type vertex --credential VERTEX_ADC=\"$(cat /path/to/adc.json)\"".to_string()
                    ));
                }
            }
        }

        Ok(discovered)
    }

    fn credential_env_vars(&self) -> &'static [&'static str] {
        SPEC.credential_env_vars
    }

    async fn get_runtime_token(&self, store: &dyn SecretStore) -> RuntimeResult<TokenResponse> {
        tracing::debug!("fetching runtime token for vertex provider");

        // Get ADC from secret store
        let adc_json = store.get("VERTEX_ADC").await?;

        // Parse ADC and exchange for OAuth token
        let adc: AdcCredentials = serde_json::from_str(&adc_json)
            .map_err(|e| RuntimeError::InvalidResponse(format!("Invalid ADC format: {}", e)))?;

        tracing::info!("exchanging ADC for OAuth token");
        self.exchange_adc_for_token(adc).await
    }
}

#[cfg(test)]
mod tests {
    use super::SPEC;
    use crate::discover_with_spec;
    use crate::test_helpers::MockDiscoveryContext;

    #[test]
    fn discovers_vertex_env_credentials() {
        let ctx =
            MockDiscoveryContext::new().with_env("ANTHROPIC_VERTEX_PROJECT_ID", "my-gcp-project");
        let discovered = discover_with_spec(&SPEC, &ctx)
            .expect("discovery")
            .expect("provider");
        assert_eq!(
            discovered.credentials.get("ANTHROPIC_VERTEX_PROJECT_ID"),
            Some(&"my-gcp-project".to_string())
        );
    }
}
