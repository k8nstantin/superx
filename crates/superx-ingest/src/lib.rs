//! # superx-ingest — universal data-source ingestion
//!
//! Implements the **"data sources are compiled entities"** principle
//! (`ARCHITECTURE.md` §0c Principle #4). Every source — filesystem directory,
//! JSON payload, future SQL / Iceberg / RAG / remote-model connectors — is
//! reduced to a single trait, [`IngestionSource`], that produces a root
//! entity id and chained substrate state.
//!
//! ## Entry points
//!
//! - [`IngestionSource`] — the trait every connector implements.
//! - [`FileSource`] — walks a local directory; one `node_code_root` plus a
//!   `node_code` per file, linked by `edge_owns`. NASA-Rule-2-bounded by
//!   `max_ingestion_entries`.
//! - [`JsonSource`] — single-shot ingestion of a structured payload into a
//!   `node_source_external`.
//! - [`UniversalIngestor`] — thin orchestrator that calls
//!   `IngestionSource::ingest` on the boxed source.
//!
//! ## Design notes
//!
//! - **Bounded iteration.** The walkdir loop trips the
//!   `max_ingestion_entries` assertion before processing the (N+1)th entry —
//!   no unbounded ingestion is possible, per NASA Power-of-10 Rule 2.
//! - **`target/` and `.git/` paths are skipped** so a `graphify .` on a Rust
//!   project doesn't ingest its own build artifacts or git history.
//! - **Per-entry checkpoint.** Each file processed writes an
//!   `execution_cursor` row so a crashed ingest can resume from where it
//!   left off.
//!
//! Future connectors land here as new `IngestionSource` impls (Roadmap #4 in
//! `ARCHITECTURE.md` §8).
//!
//! Copyright (c) 2026 Constantin Alexander <constantin@dedomena.io>.
//! Licensed under the Apache License, Version 2.0.

#![deny(warnings)]
#![deny(clippy::pedantic)]

use superx_kernel::{Kernel, KernelError};
use std::path::Path;
use walkdir::WalkDir;
use surrealdb::sql::Thing;
use async_trait::async_trait;
use serde_json::Value;

/// `IngestionSource`: Interface for all 'Ingestion Blades' (Local, Web, SQL, Iceberg, Snowflake).
#[async_trait]
pub trait IngestionSource: Send + Sync {
    /// Ingests data from the source into the substrate.
    ///
    /// # Errors
    /// Returns `KernelError` if ingestion fails.
    async fn ingest(&self, kernel: &Kernel, run_id: &str) -> Result<String, KernelError>;
}

/// `FileSource`: Logic for local directory ingestion (The previous `RepoIngestor` logic).
pub struct FileSource {
    pub path: String,
}

#[async_trait]
impl IngestionSource for FileSource {
    /// Ingests a local file system directory.
    ///
    /// # Errors
    /// Returns `KernelError` if directory walk or substrate interaction fails.
    async fn ingest(&self, kernel: &Kernel, run_id: &str) -> Result<String, KernelError> {
        let root_path = Path::new(&self.path);
        assert!(root_path.exists(), "Root path must exist");

        // Resolve type FKs through the kernel's type_cache (single chokepoint
        // replacing the legacy `type_definition:<name>` named-id literals).
        let node_code_root = kernel.type_thing("node_code_root")?;
        let node_code = kernel.type_thing("node_code")?;
        // Tenant FK: substrate entity Thing read directly from the session.
        let tenant_thing = kernel.session_tenant_thing().await?;

        // Root entity for the ingestion (subject of the cursor chain).
        let root_uuid = uuid::Uuid::now_v7();
        let root_thing = Thing::from((
            "entity",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(root_uuid)),
        ));
        let root_uid = root_uuid.to_string();

        kernel.db.query(
            "CREATE entity CONTENT { \
                id: $id, \
                type: $type, \
                tenant: $tenant, \
                role: 'user' \
            }"
        )
            .bind(("id", root_thing.clone()))
            .bind(("type", node_code_root))
            .bind(("tenant", tenant_thing.clone()))
            .await?.check()?;

        let mut path_to_id = std::collections::HashMap::new();
        path_to_id.insert(root_path.to_path_buf(), root_uid.clone());

        let mut entry_count = 0;
        let max_entries: usize = kernel.get_parameter("max_ingestion_entries", 10_000).await;

        for entry_res in WalkDir::new(root_path) {
            entry_count += 1;
            assert!(entry_count <= max_entries, "NASA Safety Violation: Ingestion limit exceeded");

            let entry = match entry_res {
                Ok(e) => e,
                Err(err) => {
                    tracing::error!("walkdir error: {err}");
                    continue;
                }
            };
            let path = entry.path();
            if path == root_path { continue; }
            let path_str = path.to_string_lossy();
            if path_str.contains("target/") || path_str.contains(".git/") { continue; }

            // `WalkDir` never yields the filesystem root, so `path.parent()` is
            // always Some — but we don't rely on that implicit invariant: if it
            // ever returns None we fall back to the ingestion root rather than
            // panicking inside an async traversal.
            let parent = path.parent().unwrap_or(root_path);
            let parent_uid = path_to_id.get(parent).cloned().unwrap_or(root_uid.clone());

            let node_uuid = uuid::Uuid::now_v7();
            let entity_thing = Thing::from((
                "entity",
                surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(node_uuid)),
            ));
            let node_uid = node_uuid.to_string();

            kernel.db.query(
                "CREATE entity CONTENT { \
                    id: $id, \
                    type: $type, \
                    tenant: $tenant, \
                    role: 'user' \
                }"
            )
                .bind(("id", entity_thing))
                .bind(("type", node_code.clone()))
                .bind(("tenant", tenant_thing.clone()))
                .await?.check()?;

            path_to_id.insert(path.to_path_buf(), node_uid.clone());

            let parent_record_id = format!("entity:{parent_uid}");
            let entity_record_id = format!("entity:{node_uid}");
            kernel.create_structural_edge(&parent_record_id, &entity_record_id, "edge_owns").await?;

            if entry.file_type().is_file() {
                if let Ok(content) = std::fs::read_to_string(path) {
                    kernel.supersede_state(&entity_record_id, "attr_desc", serde_json::json!({"text": content}), Some(run_id.to_string())).await?;
                }
            }

            // Cursor subject = the ingestion root entity. The cursor chain
            // for this directory walk advances by file path as we traverse.
            kernel.write_cursor(
                root_thing.clone(),
                "ingestion",
                Some(path_str.to_string()),
                Some(serde_json::json!({"run_id": run_id})),
            ).await?;
        }

        Ok(format!("entity:{root_uid}"))
    }
}

/// `JsonSource`: Logic for ingesting structured JSON data.
pub struct JsonSource {
    pub data: Value,
}

#[async_trait]
impl IngestionSource for JsonSource {
    /// Ingests a structured JSON payload.
    ///
    /// # Errors
    /// Returns `KernelError` if substrate interaction fails.
    async fn ingest(&self, kernel: &Kernel, run_id: &str) -> Result<String, KernelError> {
        let node_source_external = kernel.type_thing("node_source_external")?;
        let tenant_thing = kernel.session_tenant_thing().await?;

        let json_uuid = uuid::Uuid::now_v7();
        let json_thing = Thing::from((
            "entity",
            surrealdb::sql::Id::Uuid(surrealdb::sql::Uuid::from(json_uuid)),
        ));
        let json_record_id = format!("entity:{json_uuid}");

        kernel.db.query(
            "CREATE entity CONTENT { \
                id: $id, \
                type: $type, \
                tenant: $tenant, \
                role: 'user' \
            }"
        )
            .bind(("id", json_thing))
            .bind(("type", node_source_external))
            .bind(("tenant", tenant_thing))
            .await?.check()?;

        kernel.supersede_state(&json_record_id, "attr_desc", self.data.clone(), Some(run_id.to_string())).await?;

        Ok(json_record_id)
    }
}

/// `UniversalIngestor`: Orchestrator for multi-source ingestion.
pub struct UniversalIngestor<'a> {
    pub kernel: &'a Kernel,
}

impl<'a> UniversalIngestor<'a> {
    /// Creates a new `UniversalIngestor`.
    #[must_use]
    pub fn new(kernel: &'a Kernel) -> Self {
        Self { kernel }
    }

    /// `ingest`: Distills an external source into the substrate.
    ///
    /// # Errors
    /// Returns `KernelError` if source ingestion fails.
    pub async fn ingest(&self, source: Box<dyn IngestionSource>, run_id: &str) -> Result<String, KernelError> {
        tracing::info!("Starting universal ingestion (Run ID: {run_id})");
        source.ingest(self.kernel, run_id).await
    }
}
