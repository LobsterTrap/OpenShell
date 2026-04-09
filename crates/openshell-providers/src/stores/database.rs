// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Gateway database secret store.
//!
//! Fetches credentials from the provider credentials HashMap stored in the gateway database.
//! This is the primary secret storage mechanism for OpenShell.
//!
//! The gateway stores Provider records with credentials in `Provider.credentials` HashMap.
//! This store provides a clean abstraction over that storage.

use crate::secret_store::{SecretError, SecretResult, SecretStore};
use async_trait::async_trait;
use std::collections::HashMap;

/// Gateway database secret store
///
/// Wraps a provider's credentials HashMap from the database.
/// This is a simple in-memory wrapper - the actual persistence is handled
/// by the gateway's database layer.
pub struct DatabaseStore {
    credentials: HashMap<String, String>,
}

impl DatabaseStore {
    /// Create a new database store from provider credentials
    #[must_use]
    pub fn new(credentials: HashMap<String, String>) -> Self {
        Self { credentials }
    }
}

#[async_trait]
impl SecretStore for DatabaseStore {
    async fn get(&self, key: &str) -> SecretResult<String> {
        tracing::debug!(key = key, "fetching secret from database store");

        self.credentials.get(key).cloned().ok_or_else(|| {
            SecretError::NotFound(format!("Credential '{}' not found in provider", key))
        })
    }

    async fn health_check(&self) -> SecretResult<()> {
        // Database store is always available (in-memory)
        Ok(())
    }

    fn name(&self) -> &'static str {
        "database"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_database_store_get() {
        let mut creds = HashMap::new();
        creds.insert("VERTEX_ADC".to_string(), "mock-adc-json".to_string());

        let store = DatabaseStore::new(creds);

        let result = store.get("VERTEX_ADC").await.unwrap();
        assert_eq!(result, "mock-adc-json");
    }

    #[tokio::test]
    async fn test_database_store_not_found() {
        let store = DatabaseStore::new(HashMap::new());

        let result = store.get("NONEXISTENT").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_database_store_health_check() {
        let store = DatabaseStore::new(HashMap::new());
        let result = store.health_check().await;
        assert!(result.is_ok());
    }
}
