// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Generic secret storage interface.
//!
//! This module defines the storage layer for secrets/credentials.
//! Storage implementations are completely generic - they don't know about
//! provider-specific credential formats (ADC, API keys, etc.).
//!
//! The provider plugins (VertexProvider, AnthropicProvider, etc.) know how
//! to interpret the secrets retrieved from storage.

use async_trait::async_trait;

/// Result type for secret store operations
pub type SecretResult<T> = Result<T, SecretError>;

/// Errors that can occur during secret storage operations
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("secret not found: {0}")]
    NotFound(String),

    #[error("storage unavailable: {0}")]
    Unavailable(String),

    #[error("access denied: {0}")]
    AccessDenied(String),

    #[error("invalid format: {0}")]
    InvalidFormat(String),

    #[error("network error: {0}")]
    Network(String),
}

/// Generic secret storage interface
///
/// Implementations store and retrieve raw secret strings without interpreting them.
/// The provider plugins are responsible for interpreting the secret format.
#[async_trait]
pub trait SecretStore: Send + Sync {
    /// Retrieve a secret by key
    ///
    /// Returns the raw secret string without interpretation.
    async fn get(&self, key: &str) -> SecretResult<String>;

    /// Check if the storage backend is available
    ///
    /// This should be a lightweight check (e.g., can we connect to the storage service?)
    /// without actually retrieving secrets.
    async fn health_check(&self) -> SecretResult<()>;

    /// Get a human-readable name for this storage backend
    fn name(&self) -> &'static str;
}
