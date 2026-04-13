// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Runtime credential operations for providers.
//!
//! This module defines the runtime phase where providers fetch and exchange
//! credentials for access tokens during sandbox execution.

use std::collections::HashMap;

/// Standard response format for runtime token operations
#[derive(Debug, Clone)]
pub struct TokenResponse {
    /// The actual token/secret value
    pub access_token: String,

    /// Token type (e.g., "Bearer")
    pub token_type: String,

    /// Seconds until expiration (from now)
    pub expires_in: u64,

    /// Provider-specific metadata (e.g., project_id, region)
    pub metadata: HashMap<String, String>,
}

/// Result type for runtime operations
pub type RuntimeResult<T> = Result<T, RuntimeError>;

/// Errors that can occur during runtime credential operations
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("provider not configured: {0}")]
    NotConfigured(String),

    #[error("network error: {0}")]
    Network(String),

    #[error("authentication failed: {0}")]
    AuthFailed(String),

    #[error("token expired")]
    Expired,

    #[error("invalid response: {0}")]
    InvalidResponse(String),

    #[error("secret store error: {0}")]
    SecretStore(#[from] crate::secret_store::SecretError),
}

impl From<reqwest::Error> for RuntimeError {
    fn from(e: reqwest::Error) -> Self {
        RuntimeError::Network(e.to_string())
    }
}

impl From<serde_json::Error> for RuntimeError {
    fn from(e: serde_json::Error) -> Self {
        RuntimeError::InvalidResponse(e.to_string())
    }
}
