// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Vertex AI provider-specific sandbox runtime behavior.
//!
//! ## OAuth Token Interception for Claude CLI Compatibility
//!
//! This module implements a workaround to enable Claude CLI to work with Vertex AI
//! without requiring users to manually authenticate via `gcloud auth application-default login`
//! inside the sandbox.
//!
//! ### The Problem
//!
//! Claude CLI expects valid Application Default Credentials (ADC) from Google Cloud:
//! 1. Reads ADC file from ~/.config/gcloud/application_default_credentials.json
//! 2. Attempts to exchange refresh token with oauth2.googleapis.com
//! 3. Uses returned access token for Vertex AI API requests
//!
//! ### Our Solution
//!
//! We inject **fake** ADC credentials via `create_fake_vertex_adc()` and intercept
//! the token exchange:
//!
//! 1. **Fake ADC credentials** are written to the expected path
//! 2. Claude CLI reads these fake credentials
//! 3. Claude CLI sends POST /token to oauth2.googleapis.com
//! 4. **We intercept this request** and return a fake OAuth success response
//! 5. Claude CLI proceeds to make Vertex API requests
//! 6. **Real OAuth tokens** are injected via Authorization headers by the proxy
//!
//! ### Why This is Vertex-Specific
//!
//! - Only Vertex AI uses oauth2.googleapis.com for OAuth token exchange
//! - The fake token in the intercepted response is never actually used
//! - Real tokens come from the token cache (VERTEX_ACCESS_TOKEN environment variable)
//! - This workaround is specific to Google Cloud / Vertex AI authentication flow
//!
//! ### Related Code
//!
//! - ADC credential creation: `lib.rs::create_fake_vertex_adc()`
//! - OAuth header injection: `l7/rest.rs::inject_oauth_header()`
//! - Token caching: `openshell-providers::token_cache::TokenCache`

use tracing::info;

/// Check if this request should be intercepted for Vertex AI OAuth workaround.
///
/// Returns `true` if:
/// - Method is POST
/// - Host is oauth2.googleapis.com
/// - Path is /token
///
/// This is called from both L7 (TLS-terminated) and L4 (forward proxy) paths.
pub fn should_intercept_oauth_request(method: &str, host: &str, path: &str) -> bool {
    method.to_ascii_uppercase() == "POST"
        && host.to_ascii_lowercase() == "oauth2.googleapis.com"
        && path == "/token"
}

/// Generate a fake OAuth success response for intercepted token exchange.
///
/// The access token in this response is a placeholder - it will never be used.
/// Real OAuth tokens are injected via Authorization headers by the proxy's
/// `inject_oauth_header()` function.
///
/// # L7 Path (TLS-terminated)
///
/// For requests processed via L7 inspection (rest.rs), we return a fake token
/// because Claude CLI needs *some* response to proceed. The actual token injection
/// happens later via `inject_oauth_header()`.
///
/// # L4 Path (forward proxy)
///
/// For requests that bypass L7 inspection (proxy.rs FORWARD path), we can optionally
/// inject the real cached token from VERTEX_ACCESS_TOKEN if available. This is
/// more correct but still a workaround - ideally all Vertex requests would go
/// through L7 inspection where OAuth header injection happens properly.
pub fn generate_fake_oauth_response(access_token: Option<&str>) -> Vec<u8> {
    let token = access_token.unwrap_or("fake-token-will-be-replaced-by-proxy");

    let response_body = format!(
        r#"{{"access_token":"{}","token_type":"Bearer","expires_in":3600}}"#,
        token
    );

    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        response_body.len(),
        response_body
    )
    .into_bytes()
}

/// Log that we're intercepting a Google OAuth token exchange.
///
/// This is called from both rest.rs and proxy.rs to provide consistent logging.
pub fn log_oauth_interception(context: &str) {
    info!(
        context = context,
        "Intercepting Google OAuth token exchange (Vertex AI workaround)"
    );
}
