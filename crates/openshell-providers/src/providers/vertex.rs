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

            // NOTE: We do NOT generate/store VERTEX_OAUTH_TOKEN here.
            // OAuth tokens are short-lived (~1 hour) and storing them leads to stale token pollution.
            // Instead, sandboxes generate fresh tokens on-demand from the uploaded ADC file
            // (requires --upload ~/.config/gcloud/:.config/gcloud/ when creating sandbox).
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
