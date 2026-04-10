// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

//! Provider-specific runtime behavior for the sandbox.
//!
//! This module contains provider-specific logic that runs within the sandbox
//! at request processing time. This is separate from the provider discovery
//! and credential management in the `openshell-providers` crate.

pub mod vertex;
