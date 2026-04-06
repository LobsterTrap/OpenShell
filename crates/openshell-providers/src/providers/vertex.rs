// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

use crate::{
    DiscoveredProvider, ProviderDiscoverySpec, ProviderError, ProviderPlugin, RealDiscoveryContext,
    discover_with_spec,
};

pub struct VertexProvider;

pub const SPEC: ProviderDiscoverySpec = ProviderDiscoverySpec {
    id: "vertex",
    credential_env_vars: &["ANTHROPIC_VERTEX_PROJECT_ID"],
};

// Additional config keys for Vertex AI
const VERTEX_CONFIG_KEYS: &[&str] = &["ANTHROPIC_VERTEX_REGION"];

/// Generate an OAuth token from GCP Application Default Credentials for Vertex AI.
///
/// Returns `None` if ADC is not configured or token generation fails.
async fn generate_oauth_token() -> Option<String> {
    // Try to find an appropriate token provider (checks ADC, service account, metadata server, etc.)
    let provider = gcp_auth::provider().await.ok()?;

    // Get token for Vertex AI scope
    // Vertex AI uses the Cloud Platform scope
    let scopes = &["https://www.googleapis.com/auth/cloud-platform"];
    let token = provider.token(scopes).await.ok()?;

    Some(token.as_str().to_string())
}

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

            // Generate OAuth token from Application Default Credentials
            // Try to generate token, but don't fail if we're in a nested runtime context
            let token = std::thread::spawn(|| {
                tokio::runtime::Runtime::new()
                    .ok()
                    .and_then(|rt| rt.block_on(generate_oauth_token()))
            })
            .join()
            .ok()
            .flatten();

            if let Some(token) = token {
                // Store the OAuth token as VERTEX_OAUTH_TOKEN
                // The inference router will use this as the Bearer token
                provider.credentials.insert("VERTEX_OAUTH_TOKEN".to_string(), token);
            }
        }

        Ok(discovered)
    }

    fn credential_env_vars(&self) -> &'static [&'static str] {
        SPEC.credential_env_vars
    }
}

#[cfg(test)]
mod tests {
    use super::SPEC;
    use crate::discover_with_spec;
    use crate::test_helpers::MockDiscoveryContext;

    #[test]
    fn discovers_vertex_env_credentials() {
        let ctx = MockDiscoveryContext::new()
            .with_env("ANTHROPIC_VERTEX_PROJECT_ID", "my-gcp-project");
        let discovered = discover_with_spec(&SPEC, &ctx)
            .expect("discovery")
            .expect("provider");
        assert_eq!(
            discovered.credentials.get("ANTHROPIC_VERTEX_PROJECT_ID"),
            Some(&"my-gcp-project".to_string())
        );
    }
}
