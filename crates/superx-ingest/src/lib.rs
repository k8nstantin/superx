/*
 * SuperX Universal Ingestor - Revision 42.14 (Hardened)
 * 
 * Copyright (c) 2026 Constantin Alexander <constantin@dedomena.io>
 */

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
        
        let root_uid = uuid::Uuid::now_v7().to_string();
        let root_thing = Thing::from(("entity".to_string(), root_uid.clone()));
        
        kernel.db.query("INSERT INTO entity { id: $id, tenant_id: $session_tenant, type: type_definition:node_code_root }")
            .bind(("id", root_thing.clone())).await?.check()?;

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

            let node_uid = uuid::Uuid::now_v7().to_string();
            let entity_thing = Thing::from(("entity".to_string(), node_uid.clone()));

            kernel.db.query("INSERT INTO entity { id: $id, tenant_id: $session_tenant, type: type_definition:node_code }")
                .bind(("id", entity_thing)).await?.check()?;

            path_to_id.insert(path.to_path_buf(), node_uid.clone());

            let parent_record_id = format!("entity:{parent_uid}");
            let entity_record_id = format!("entity:{node_uid}");
            kernel.create_structural_edge(&parent_record_id, &entity_record_id, "edge_owns").await?;

            if entry.file_type().is_file() {
                if let Ok(content) = std::fs::read_to_string(path) {
                    kernel.supersede_state(&entity_record_id, "attr_desc", serde_json::json!({"text": content}), Some(run_id.to_string())).await?;
                }
            }

            kernel.checkpoint_execution(run_id, "ingestion", Some(path_str.to_string()), None).await?;
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
        let json_uid = uuid::Uuid::now_v7().to_string();
        let json_thing = Thing::from(("entity".to_string(), json_uid.clone()));
        let json_record_id = format!("entity:{json_uid}");
        
        kernel.db.query("INSERT INTO entity { id: $id, tenant_id: $session_tenant, type: type_definition:node_source_external }")
            .bind(("id", json_thing)).await?.check()?;

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
